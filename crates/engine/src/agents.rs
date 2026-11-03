//! Web agent runs (story 04): a member asks in the web tool, a coding agent works on their draft in
//! a container run by `charet-agent`, and every save lands on the branch as a push.
//!
//! - One run at a time per branch; later ones wait (`queued`). A few run at once server-wide.
//! - The container reaches the server only through the agent door, with a token good for this
//!   run's project and branch while it runs ([`Engine::agent_door`]).
//! - A canceled or failed run keeps what it already saved (each save is a push on the branch).
//! - Vendor API keys come from the environment or the admins' settings (stored encrypted with the
//!   server key; only the last 4 characters are ever shown). Costs are recorded, never capped.

use std::collections::HashMap;
use std::sync::Arc;

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use charet_model::{
    Actor, AgentKind, AgentMessage, AgentRun, Attachment, BranchStatus, MessageKind, RunState,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::members::{hash, hex, random_hex};
use crate::policy::{Command, can};
use crate::providers::Credential;
use crate::{AgentRunner, Engine, EngineError, NewRun, RunUpdate, RunnerStart};

const DEFAULT_AGENT: &str = "agent.default";
const MAX_PROMPT: usize = 8_000;
const MAX_IMAGES: usize = 4;
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
/// Earlier turns handed to a run as context.
const HISTORY: usize = 20;

/// How the server runs agents (set at start).
#[derive(Clone)]
pub struct AgentConfig {
    /// Keys from the environment (`CHARET_ANTHROPIC_API_KEY`, `CHARET_OPENAI_API_KEY`); they win
    /// over the admins' settings.
    pub env_keys: HashMap<String, String>,
    /// Offer the scripted test agent (tests and e2e only).
    pub script: bool,
    /// Runs at once, server-wide.
    pub max_running: usize,
    /// The longest a run may take, in minutes (`CHARET_AGENT_TIMEOUT_MIN`, which `charet-agent`
    /// enforces; it defaults to 30). A login must outlast it, so it is renewed before a run when
    /// it would not.
    pub run_minutes: u64,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            env_keys: HashMap::new(),
            script: false,
            max_running: 0,
            run_minutes: 30,
        }
    }
}

impl AgentConfig {
    /// The longest a run may take, in seconds.
    pub(crate) fn run_limit_secs(&self) -> u64 {
        self.run_minutes * 60
    }
}

impl std::fmt::Debug for AgentConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentConfig")
            .field("script", &self.script)
            .field("max_running", &self.max_running)
            .finish_non_exhaustive()
    }
}

/// The runner and its settings, when the server has one.
#[derive(Clone)]
pub(crate) struct Agents {
    pub runner: Arc<dyn AgentRunner>,
    pub config: AgentConfig,
}

/// What someone asks the web agent.
#[derive(Debug, Clone, Default)]
pub struct RunInput {
    pub prompt: String,
    /// What they pointed at (screen, element, source), passed on as given.
    pub context: serde_json::Value,
    /// Their draft or a branch in review to continue; `None` starts a new draft.
    pub branch: Option<u32>,
    /// Defaults to the admins' choice.
    pub agent: Option<AgentKind>,
    /// Defaults to the admins' choice for that agent.
    pub model: Option<String>,
    pub images: Vec<Attachment>,
}

/// One agent as the admins' page shows it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentChoice {
    pub agent: AgentKind,
    /// A key is set (or none is needed).
    pub ready: bool,
    /// `env`, `settings` or `none`.
    pub key: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_last4: Option<String>,
    /// The model it runs unless a run asks for another; empty is the agent's own default.
    pub model: String,
}

/// The web agent as the server offers it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    /// A runner is configured and answers.
    pub enabled: bool,
    /// Why not, when it is not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub default: Option<AgentKind>,
    pub agents: Vec<AgentChoice>,
}

/// What admins may change.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSettingsChange {
    pub default: Option<AgentKind>,
    /// Model per agent; an empty string goes back to the agent's default.
    #[serde(default)]
    pub models: HashMap<AgentKind, String>,
    /// API key per agent; an empty string removes the saved one.
    #[serde(default)]
    pub keys: HashMap<AgentKind, String>,
}

/// Who spent what on the web agent.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Costs {
    pub people: Vec<PersonCost>,
    /// Newest first.
    pub runs: Vec<AgentRun>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonCost {
    pub id: String,
    pub name: String,
    pub runs: u32,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
    /// Runs whose model has no known price (not in `cost_usd`).
    pub unpriced: u32,
}

/// What a token at the agent door may touch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAccess {
    /// The member who asked, as the run acts (name marked as their agent).
    pub actor: Actor,
    pub project: String,
    pub branch: u32,
}

impl Engine {
    /// Whether the web agent is on, and which agents it offers. Members only.
    pub fn agent_status(&self, actor: &Actor) -> Result<AgentStatus, EngineError> {
        if matches!(actor, Actor::Visitor { .. }) {
            return Err(EngineError::Forbidden);
        }
        let Some(agents) = &self.agents else {
            return Ok(AgentStatus {
                enabled: false,
                reason: Some("no agent runner is configured (CHARET_AGENT_URL)".into()),
                default: None,
                agents: Vec::new(),
            });
        };
        let reason = agents
            .runner
            .health()
            .err()
            .map(|e| format!("the agent runner does not answer: {e:?}"));
        let choices = self.agent_choices(&agents.config)?;
        let default = self.default_agent(&choices)?;
        Ok(AgentStatus {
            enabled: reason.is_none() && choices.iter().any(|c| c.ready),
            reason,
            default,
            agents: choices,
        })
    }

    /// Changes the default agent, models and keys. Admins only.
    pub fn set_agent_settings(
        &self,
        actor: &Actor,
        change: &AgentSettingsChange,
    ) -> Result<AgentStatus, EngineError> {
        if !can(actor, Command::Admin, None) {
            return Err(EngineError::Forbidden);
        }
        if let Some(agent) = change.default {
            self.ports.settings.set(DEFAULT_AGENT, kind_name(agent))?;
        }
        for (agent, model) in &change.models {
            let model = model.trim();
            if model.len() > 100 || model.chars().any(char::is_whitespace) {
                return Err(EngineError::Invalid(
                    "a model is one name, like claude-sonnet-5-5".into(),
                ));
            }
            self.ports.settings.set(&model_key(*agent), model)?;
        }
        for (agent, key) in &change.keys {
            let key = key.trim();
            let stored = if key.is_empty() {
                String::new()
            } else {
                self.seal(key)?
            };
            self.ports.settings.set(&key_key(*agent), &stored)?;
        }
        self.agent_status(actor)
    }

    /// Asks the web agent: records the run (queued) on the branch, then starts it when it can.
    pub fn start_agent_run(
        &self,
        actor: &Actor,
        key: &str,
        input: RunInput,
    ) -> Result<AgentRun, EngineError> {
        self.visible(actor, key, Command::BranchEdit)?;
        let agents = self.agents.as_ref().ok_or_else(|| {
            EngineError::Unavailable("the web agent is not set up on this server".into())
        })?;
        let prompt = input.prompt.trim();
        if prompt.is_empty() {
            return Err(EngineError::Invalid("say what to change".into()));
        }
        if prompt.len() > MAX_PROMPT {
            return Err(EngineError::Invalid(format!(
                "keep it under {MAX_PROMPT} characters"
            )));
        }
        if input.images.len() > MAX_IMAGES
            || input.images.iter().any(|i| i.data.len() > MAX_IMAGE_BYTES)
        {
            return Err(EngineError::Invalid(format!(
                "up to {MAX_IMAGES} images of 5 MB each"
            )));
        }
        if input.images.iter().any(|i| {
            !["image/png", "image/jpeg", "image/gif", "image/webp"].contains(&i.media_type.as_str())
        }) {
            return Err(EngineError::Invalid(
                "images are PNG, JPEG, GIF or WebP".into(),
            ));
        }
        let choices = self.agent_choices(&agents.config)?;
        let agent = match input.agent {
            Some(agent) => agent,
            None => self
                .default_agent(&choices)?
                .ok_or_else(|| EngineError::Unavailable("no agent has an API key yet".into()))?,
        };
        let choice = choices.iter().find(|c| c.agent == agent).ok_or_else(|| {
            EngineError::Invalid(format!("{} is not offered here", kind_name(agent)))
        })?;
        if !choice.ready {
            return Err(EngineError::Unavailable(format!(
                "{} has no API key; an admin can add one",
                kind_name(agent)
            )));
        }
        let model = input
            .model
            .map(|m| m.trim().to_owned())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| choice.model.clone());
        let mut context = input.context;
        let branch = if let Some(number) = input.branch {
            let branch = self.find_branch(key, number)?;
            Self::may_change(actor, &branch)?;
            number
        } else {
            {
                let title: String = prompt
                    .lines()
                    .next()
                    .unwrap_or(prompt)
                    .chars()
                    .take(60)
                    .collect();
                let branch = self.create_branch(actor, key, &title, None)?;
                if let serde_json::Value::Object(map) = &mut context {
                    map.insert("newBranch".into(), true.into());
                } else {
                    context = serde_json::json!({ "given": context, "newBranch": true });
                }
                branch.number
            }
        };
        let run = self.ports.agents.insert_run(&NewRun {
            project: key.to_owned(),
            branch,
            actor: actor.person(),
            agent,
            model,
            prompt: prompt.to_owned(),
            context,
            attachments: input.images,
        })?;
        self.agent_event(&run);
        self.agents_tick()?;
        Ok(self.ports.agents.run(run.id)?.unwrap_or(run))
    }

    /// A project's runs (one branch's when given), oldest first.
    pub fn agent_runs(
        &self,
        actor: &Actor,
        key: &str,
        branch: Option<u32>,
    ) -> Result<Vec<AgentRun>, EngineError> {
        self.visible(actor, key, Command::ProjectView)?;
        Ok(self.ports.agents.runs(key, branch)?)
    }

    /// A run's conversation after `after`.
    pub fn agent_messages(
        &self,
        actor: &Actor,
        key: &str,
        id: u64,
        after: u32,
    ) -> Result<Vec<AgentMessage>, EngineError> {
        self.visible(actor, key, Command::ProjectView)?;
        let run = self.project_run(key, id)?;
        Ok(self.ports.agents.messages(run.id, after)?)
    }

    /// An image attached to a run's prompt.
    pub fn agent_attachment(
        &self,
        actor: &Actor,
        key: &str,
        id: u64,
        seq: usize,
    ) -> Result<Attachment, EngineError> {
        self.visible(actor, key, Command::ProjectView)?;
        let run = self.project_run(key, id)?;
        self.ports
            .agents
            .attachments(run.id)?
            .into_iter()
            .nth(seq)
            .ok_or(EngineError::NotFound("attachment"))
    }

    /// Stops a run: its asker or an admin. What it saved stays on the branch.
    pub fn cancel_agent_run(
        &self,
        actor: &Actor,
        key: &str,
        id: u64,
    ) -> Result<AgentRun, EngineError> {
        self.visible(actor, key, Command::BranchEdit)?;
        let run = self.project_run(key, id)?;
        if actor.person().id != run.actor.id && !can(actor, Command::Admin, None) {
            return Err(EngineError::Forbidden);
        }
        if run.state.finished() {
            return Ok(run);
        }
        if run.state == RunState::Running
            && let Some(agents) = &self.agents
        {
            // Best effort: the run ends here either way.
            let _ = agents.runner.cancel(run.id);
        }
        self.finish_run(&run, RunState::Canceled, None, None)?;
        self.ports.agents.add_message(
            run.id,
            MessageKind::Status,
            &format!("Stopped by {}", actor.name()),
            None,
        )?;
        let run = self.project_run(key, id)?;
        self.agent_event(&run);
        self.agents_tick()?;
        Ok(run)
    }

    /// Every run and what each person spent. Admins only.
    pub fn agent_costs(&self, actor: &Actor) -> Result<Costs, EngineError> {
        if !can(actor, Command::Admin, None) {
            return Err(EngineError::Forbidden);
        }
        let runs = self.ports.agents.all_runs()?;
        let mut people: Vec<PersonCost> = Vec::new();
        for run in &runs {
            let index = if let Some(index) = people.iter().position(|p| p.id == run.actor.id) {
                index
            } else {
                {
                    people.push(PersonCost {
                        id: run.actor.id.clone(),
                        name: run.actor.name.clone(),
                        runs: 0,
                        tokens_in: 0,
                        tokens_out: 0,
                        cost_usd: 0.0,
                        unpriced: 0,
                    });
                    people.len() - 1
                }
            };
            let person = &mut people[index];
            person.runs += 1;
            person.tokens_in += run.tokens_in;
            person.tokens_out += run.tokens_out;
            match run.cost_usd {
                Some(cost) => person.cost_usd += cost,
                None if run.agent != AgentKind::Script && run.tokens_in > 0 => person.unpriced += 1,
                None => {}
            }
        }
        people.sort_by(|a, b| b.cost_usd.total_cmp(&a.cost_usd));
        Ok(Costs { people, runs })
    }

    /// Who a token at the agent door acts as, while its run runs.
    pub fn agent_door(&self, token: &str) -> Result<Option<AgentAccess>, EngineError> {
        let Some(run) = self.ports.agents.run_by_token(&hash(token.trim()))? else {
            return Ok(None);
        };
        if run.state != RunState::Running {
            return Ok(None);
        }
        let owner = self.ports.accounts.user(&run.actor.id)?;
        let actor = match owner {
            Some(user) => Actor::User {
                id: user.id,
                name: format!("{} · agent", user.name),
                role: user.role,
            },
            // The built-in admin (before an owner exists) or a member removed meanwhile.
            None if run.actor.id == "local" => Actor::Local,
            None => return Ok(None),
        };
        Ok(Some(AgentAccess {
            actor,
            project: run.project,
            branch: run.branch,
        }))
    }

    /// Moves runs along: takes in what running ones did, ends finished ones and starts waiting
    /// ones. The server calls this every second.
    pub fn agents_tick(&self) -> Result<(), EngineError> {
        let Some(agents) = self.agents.clone() else {
            return Ok(());
        };
        let _turn = self
            .agent_turn
            .lock()
            .map_err(|_| EngineError::Unavailable("agent bookkeeping is stuck".into()))?;
        let open = self.ports.agents.open_runs()?;
        for run in open.iter().filter(|r| r.state == RunState::Running) {
            self.follow_run(&agents, run)?;
        }
        let open = self.ports.agents.open_runs()?;
        let mut running: Vec<(String, u32)> = open
            .iter()
            .filter(|r| r.state == RunState::Running)
            .map(|r| (r.project.clone(), r.branch))
            .collect();
        for run in open.iter().filter(|r| r.state == RunState::Queued) {
            if running.len() >= agents.config.max_running.max(1) {
                break;
            }
            if running.contains(&(run.project.clone(), run.branch)) {
                continue;
            }
            self.launch_run(&agents, run)?;
            running.push((run.project.clone(), run.branch));
        }
        Ok(())
    }

    fn follow_run(&self, agents: &Agents, run: &AgentRun) -> Result<(), EngineError> {
        let progress = match agents.runner.poll(run.id, run.runner_seq) {
            Ok(Some(progress)) => progress,
            Ok(None) => {
                return self.end_with(
                    run,
                    RunState::Failed,
                    "the agent runner lost this run (restarted?)",
                );
            }
            // Try again on the next tick.
            Err(_) => return Ok(()),
        };
        for event in &progress.events {
            self.ports.agents.add_message(
                run.id,
                event.kind,
                &event.text,
                event.tool.as_deref(),
            )?;
        }
        let update = RunUpdate {
            tokens_in: Some(progress.tokens_in),
            tokens_out: Some(progress.tokens_out),
            cost_usd: progress.cost_usd,
            runner_seq: Some(progress.next),
            ..RunUpdate::default()
        };
        self.ports.agents.update_run(run.id, &update)?;
        if let Some(state) = progress.finished {
            let mut summary = match state {
                RunState::Done => "Done".to_owned(),
                RunState::Canceled => "Stopped".to_owned(),
                _ => format!(
                    "Failed: {}",
                    progress
                        .error
                        .clone()
                        .unwrap_or_else(|| "unknown error".into())
                ),
            };
            if let Some(cost) = progress.cost_usd.filter(|c| *c > 0.0) {
                summary = format!("{summary} · ${cost:.4}");
            }
            self.ports
                .agents
                .add_message(run.id, MessageKind::Status, &summary, None)?;
            self.finish_run(run, state, progress.error, progress.cost_usd)?;
        }
        if (!progress.events.is_empty() || progress.finished.is_some())
            && let Some(run) = self.ports.agents.run(run.id)?
        {
            self.agent_event(&run);
        }
        Ok(())
    }

    fn launch_run(&self, agents: &Agents, run: &AgentRun) -> Result<(), EngineError> {
        // The branch may have been merged or archived while the run waited.
        let branch = self.find_branch(&run.project, run.branch)?;
        if !matches!(branch.status, BranchStatus::Open | BranchStatus::InReview) {
            return self.end_with(
                run,
                RunState::Failed,
                &format!("branch {} is closed", run.branch),
            );
        }
        let credential = match run.agent {
            AgentKind::Script => Credential::default(),
            agent => match self.run_credential(&agents.config, agent) {
                Ok(Some(credential)) => credential,
                Ok(None) => {
                    return self.end_with(
                        run,
                        RunState::Failed,
                        "no API key or sign-in for this agent",
                    );
                }
                // A login that cannot be renewed ends the run with what to do about it.
                Err(EngineError::Unavailable(message)) => {
                    return self.end_with(run, RunState::Failed, &message);
                }
                Err(error) => return Err(error),
            },
        };
        let token = random_hex();
        let history = self.history(run)?;
        let images = self.ports.agents.attachments(run.id)?;
        self.ports.agents.update_run(
            run.id,
            &RunUpdate {
                state: Some(RunState::Running),
                token_hash: Some(Some(hash(&token))),
                started: true,
                ..RunUpdate::default()
            },
        )?;
        let start = RunnerStart {
            id: run.id,
            agent: run.agent,
            model: run.model.clone(),
            api_key: credential.api_key,
            oauth: credential.login,
            project: run.project.clone(),
            branch: run.branch,
            token,
            prompt: run.prompt.clone(),
            context: run.context.clone(),
            images,
            history,
        };
        if let Err(error) = agents.runner.start(&start) {
            return self.end_with(
                run,
                RunState::Failed,
                &format!("the agent runner refused it: {error:?}"),
            );
        }
        let label = if run.model.is_empty() {
            kind_name(run.agent).to_owned()
        } else {
            format!("{} · {}", kind_name(run.agent), run.model)
        };
        self.ports.agents.add_message(
            run.id,
            MessageKind::Status,
            &format!("Started ({label})"),
            None,
        )?;
        self.ports
            .realtime
            .working(&run.project, &agent_person(run), true);
        if let Some(run) = self.ports.agents.run(run.id)? {
            self.agent_event(&run);
        }
        Ok(())
    }

    fn end_with(&self, run: &AgentRun, state: RunState, error: &str) -> Result<(), EngineError> {
        self.ports.agents.add_message(
            run.id,
            MessageKind::Status,
            &format!("Failed: {error}"),
            None,
        )?;
        self.finish_run(run, state, Some(error.to_owned()), None)?;
        if let Some(run) = self.ports.agents.run(run.id)? {
            self.agent_event(&run);
        }
        Ok(())
    }

    /// Ends a run: its token stops working, and a new draft it left empty is archived.
    fn finish_run(
        &self,
        run: &AgentRun,
        state: RunState,
        error: Option<String>,
        cost: Option<f64>,
    ) -> Result<(), EngineError> {
        self.ports.agents.update_run(
            run.id,
            &RunUpdate {
                state: Some(state),
                error,
                cost_usd: cost,
                token_hash: Some(None),
                ended: true,
                ..RunUpdate::default()
            },
        )?;
        self.ports
            .realtime
            .working(&run.project, &agent_person(run), false);
        let new_branch = run.context.get("newBranch") == Some(&serde_json::Value::Bool(true));
        if new_branch && let Ok(branch) = self.find_branch(&run.project, run.branch) {
            {
                let later = self
                    .ports
                    .agents
                    .runs(&run.project, Some(run.branch))?
                    .into_iter()
                    .any(|r| r.id != run.id && !r.state.finished());
                if branch.status == BranchStatus::Open && branch.head == branch.base && !later {
                    let actor = Actor::User {
                        id: run.actor.id.clone(),
                        name: run.actor.name.clone(),
                        role: charet_model::Role::Admin,
                    };
                    // Nothing came of it: no empty draft left behind.
                    let _ = self.archive_branch(&actor, &run.project, run.branch);
                }
            }
        }
        Ok(())
    }

    /// Earlier turns on the run's branch: what was asked and what the agent answered.
    fn history(&self, run: &AgentRun) -> Result<Vec<(String, String)>, EngineError> {
        let mut turns = Vec::new();
        for earlier in self
            .ports
            .agents
            .runs(&run.project, Some(run.branch))?
            .into_iter()
            .filter(|r| r.id < run.id)
        {
            for message in self.ports.agents.messages(earlier.id, 0)? {
                let role = match message.kind {
                    MessageKind::User => "user",
                    MessageKind::Assistant => "assistant",
                    MessageKind::Tool | MessageKind::Status => continue,
                };
                turns.push((role.to_owned(), message.text));
            }
        }
        let skip = turns.len().saturating_sub(HISTORY);
        Ok(turns.split_off(skip))
    }

    fn project_run(&self, key: &str, id: u64) -> Result<AgentRun, EngineError> {
        self.ports
            .agents
            .run(id)?
            .filter(|r| r.project == key)
            .ok_or(EngineError::NotFound("run"))
    }

    fn agent_event(&self, run: &AgentRun) {
        self.publish(
            &run.project,
            "agent.updated",
            serde_json::json!({ "run": run.id, "branch": run.branch, "state": run.state }),
        );
    }

    fn agent_choices(&self, config: &AgentConfig) -> Result<Vec<AgentChoice>, EngineError> {
        let mut kinds = vec![AgentKind::Claude, AgentKind::Codex];
        if config.script {
            kinds.push(AgentKind::Script);
        }
        kinds
            .into_iter()
            .map(|agent| {
                let model = self
                    .ports
                    .settings
                    .get(&model_key(agent))?
                    .unwrap_or_default();
                if agent == AgentKind::Script {
                    return Ok(AgentChoice {
                        agent,
                        ready: true,
                        key: "none",
                        key_last4: None,
                        model,
                    });
                }
                let (key, last4) = if let Some(key) = config.env_keys.get(kind_name(agent)) {
                    ("env", Some(last4(key)))
                } else if let Some(key) = self.saved_key(agent)? {
                    ("settings", Some(last4(&key)))
                } else {
                    ("none", None)
                };
                Ok(AgentChoice {
                    agent,
                    // A key, or an account signed in to the agent's provider.
                    ready: self.agent_has_credential(config, agent)?,
                    key,
                    key_last4: last4,
                    model,
                })
            })
            .collect()
    }

    fn default_agent(&self, choices: &[AgentChoice]) -> Result<Option<AgentKind>, EngineError> {
        let saved = self.ports.settings.get(DEFAULT_AGENT)?;
        let chosen = choices
            .iter()
            .find(|c| c.ready && saved.as_deref() == Some(kind_name(c.agent)));
        Ok(chosen
            .or_else(|| {
                choices
                    .iter()
                    .find(|c| c.ready && c.agent != AgentKind::Script)
            })
            .or_else(|| choices.iter().find(|c| c.ready))
            .map(|c| c.agent))
    }

    pub(crate) fn api_key(
        &self,
        config: &AgentConfig,
        agent: AgentKind,
    ) -> Result<Option<String>, EngineError> {
        if let Some(key) = config.env_keys.get(kind_name(agent)) {
            return Ok(Some(key.clone()));
        }
        self.saved_key(agent)
    }

    fn saved_key(&self, agent: AgentKind) -> Result<Option<String>, EngineError> {
        self.saved_secret(&key_key(agent))
    }

    /// A secret kept in the settings, opened; `None` when nothing (or an empty value) is saved.
    pub(crate) fn saved_secret(&self, setting: &str) -> Result<Option<String>, EngineError> {
        match self.ports.settings.get(setting)? {
            Some(sealed) if !sealed.is_empty() => self.open(&sealed).map(Some),
            _ => Ok(None),
        }
    }

    /// Encrypts a secret for the settings table with a key derived from the server key.
    pub(crate) fn seal(&self, secret: &str) -> Result<String, EngineError> {
        let cipher = self.secrets_cipher()?;
        let mut nonce = [0u8; 12];
        getrandom::fill(&mut nonce).expect("the OS has randomness");
        let sealed = cipher
            .encrypt(&Nonce::from(nonce), secret.as_bytes())
            .map_err(|_| EngineError::Unavailable("cannot encrypt the key".into()))?;
        Ok(format!("v1:{}:{}", hex(&nonce), hex(&sealed)))
    }

    pub(crate) fn open(&self, sealed: &str) -> Result<String, EngineError> {
        let broken = || {
            EngineError::Unavailable(
                "a saved API key cannot be read (the server key changed?); save it again".into(),
            )
        };
        let mut parts = sealed.split(':');
        let (Some("v1"), Some(nonce), Some(body)) = (parts.next(), parts.next(), parts.next())
        else {
            return Err(broken());
        };
        let nonce: [u8; 12] = unhex(nonce)
            .and_then(|n| n.try_into().ok())
            .ok_or_else(broken)?;
        let body = unhex(body).ok_or_else(broken)?;
        let plain = self
            .secrets_cipher()?
            .decrypt(&Nonce::from(nonce), body.as_slice())
            .map_err(|_| broken())?;
        String::from_utf8(plain).map_err(|_| broken())
    }

    fn secrets_cipher(&self) -> Result<ChaCha20Poly1305, EngineError> {
        let key = self.secret.as_deref().ok_or_else(|| {
            EngineError::Unavailable("the server has no secret key to keep API keys with".into())
        })?;
        let mut mac = Hmac::<Sha256>::new_from_slice(key)
            .map_err(|_| EngineError::Unavailable("bad server key".into()))?;
        mac.update(b"charet:stored-secrets:v1");
        let derived = mac.finalize().into_bytes();
        ChaCha20Poly1305::new_from_slice(&derived)
            .map_err(|_| EngineError::Unavailable("bad derived key".into()))
    }
}

/// How the run shows in presence: the asker's agent.
fn agent_person(run: &AgentRun) -> charet_model::Person {
    charet_model::Person {
        id: run.actor.id.clone(),
        name: format!("{} · agent", run.actor.name),
    }
}

pub(crate) fn kind_name(agent: AgentKind) -> &'static str {
    match agent {
        AgentKind::Claude => "claude",
        AgentKind::Codex => "codex",
        AgentKind::Script => "script",
    }
}

fn model_key(agent: AgentKind) -> String {
    format!("agent.model.{}", kind_name(agent))
}

fn key_key(agent: AgentKind) -> String {
    format!("agent.key.{}", kind_name(agent))
}

pub(crate) fn last4(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    chars[chars.len().saturating_sub(4)..].iter().collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Fakes, engine, engine_with_agents, file};
    use charet_model::Role;

    fn setup() -> (Engine, Fakes, Actor, Actor) {
        let (engine, fakes) = engine_with_agents();
        engine.ensure_owner().unwrap();
        let admin = engine.local_actor();
        engine.create_project(&admin, "shop", "", None).unwrap();
        let ann = engine.add_member(&admin, "Ann", Role::Editor).unwrap();
        let ann = engine.token_actor(&ann.token).unwrap().unwrap();
        (engine, fakes, admin, ann)
    }

    fn ask(prompt: &str) -> RunInput {
        RunInput {
            prompt: prompt.into(),
            context: serde_json::json!({ "screen": "home" }),
            ..RunInput::default()
        }
    }

    #[test]
    fn a_run_starts_on_a_new_draft_and_its_token_opens_only_while_it_runs() {
        let (engine, fakes, _, ann) = setup();
        let run = engine
            .start_agent_run(&ann, "shop", ask("Make the title bigger\nand bold"))
            .unwrap();
        assert_eq!(run.state, RunState::Running);
        assert_eq!(run.agent, AgentKind::Claude, "the first agent with a key");
        let branch = engine.find_branch("shop", run.branch).unwrap();
        assert_eq!(branch.title, "Make the title bigger");
        assert_eq!(branch.author.id, ann.person().id);

        let start = fakes.runner.last_start();
        assert_eq!(start.api_key, "sk-test-abcd");
        assert_eq!((start.project.as_str(), start.branch), ("shop", run.branch));
        let access = engine
            .agent_door(&start.token)
            .unwrap()
            .expect("open while running");
        assert_eq!(access.actor.name(), "Ann · agent");
        assert_eq!(access.actor.person().id, ann.person().id);
        assert_eq!(access.branch, run.branch);
        assert_eq!(engine.agent_door("nope").unwrap(), None);

        // The agent saves under the asker's draft, as their agent.
        let base = branch.head.clone();
        engine
            .push_branch(
                &access.actor,
                "shop",
                run.branch,
                &base,
                "bigger title",
                &[file("a.tsx", "x")],
            )
            .unwrap();
        fakes
            .runner
            .say(run.id, MessageKind::Assistant, "Made it bigger.");
        fakes
            .runner
            .finish(run.id, RunState::Done, None, Some(0.0123));
        engine.agents_tick().unwrap();

        let run = engine.ports.agents.run(run.id).unwrap().unwrap();
        assert_eq!(run.state, RunState::Done);
        assert_eq!(
            (run.tokens_in, run.tokens_out, run.cost_usd),
            (100, 20, Some(0.0123))
        );
        assert_eq!(
            engine.agent_door(&start.token).unwrap(),
            None,
            "closed when done"
        );
        let texts: Vec<_> = engine
            .agent_messages(&ann, "shop", run.id, 0)
            .unwrap()
            .into_iter()
            .map(|m| (m.kind, m.text))
            .collect();
        assert_eq!(
            texts[0],
            (MessageKind::User, "Make the title bigger\nand bold".into())
        );
        assert_eq!(texts[2], (MessageKind::Assistant, "Made it bigger.".into()));
        assert_eq!(texts[3], (MessageKind::Status, "Done · $0.0123".into()));
        assert_eq!(
            engine.find_branch("shop", run.branch).unwrap().status,
            BranchStatus::Open,
            "a draft with changes stays"
        );
    }

    #[test]
    fn runs_on_one_branch_wait_their_turn_and_carry_the_conversation() {
        let (engine, fakes, _, ann) = setup();
        let first = engine.start_agent_run(&ann, "shop", ask("one")).unwrap();
        let second = engine
            .start_agent_run(
                &ann,
                "shop",
                RunInput {
                    branch: Some(first.branch),
                    ..ask("two")
                },
            )
            .unwrap();
        assert_eq!(second.state, RunState::Queued);
        assert_eq!(fakes.runner.started.lock().unwrap().len(), 1);

        fakes
            .runner
            .say(first.id, MessageKind::Assistant, "did one");
        fakes.runner.finish(first.id, RunState::Done, None, None);
        engine.agents_tick().unwrap();
        engine.agents_tick().unwrap();
        let start = fakes.runner.last_start();
        assert_eq!(start.id, second.id);
        assert_eq!(
            start.history,
            [
                ("user".to_owned(), "one".to_owned()),
                ("assistant".to_owned(), "did one".to_owned())
            ]
        );
    }

    #[test]
    fn a_canceled_run_with_nothing_saved_leaves_no_empty_draft() {
        let (engine, fakes, _, ann) = setup();
        let run = engine.start_agent_run(&ann, "shop", ask("try")).unwrap();
        let token = fakes.runner.last_start().token;
        let run = engine.cancel_agent_run(&ann, "shop", run.id).unwrap();
        assert_eq!(run.state, RunState::Canceled);
        assert_eq!(*fakes.runner.canceled.lock().unwrap(), [run.id]);
        assert_eq!(engine.agent_door(&token).unwrap(), None);
        assert_eq!(
            engine.find_branch("shop", run.branch).unwrap().status,
            BranchStatus::Archived
        );
    }

    #[test]
    fn only_the_author_runs_on_a_draft_and_only_the_asker_or_an_admin_stops_it() {
        let (engine, _, admin, ann) = setup();
        let bo = engine.add_member(&admin, "Bo", Role::Editor).unwrap();
        let bo = engine.token_actor(&bo.token).unwrap().unwrap();
        let run = engine.start_agent_run(&ann, "shop", ask("mine")).unwrap();
        assert_eq!(
            engine.start_agent_run(
                &bo,
                "shop",
                RunInput {
                    branch: Some(run.branch),
                    ..ask("x")
                }
            ),
            Err(EngineError::Forbidden)
        );
        assert_eq!(
            engine.cancel_agent_run(&bo, "shop", run.id),
            Err(EngineError::Forbidden)
        );
        engine.cancel_agent_run(&admin, "shop", run.id).unwrap();

        let viewer = engine.add_member(&admin, "Vi", Role::Viewer).unwrap();
        let viewer = engine.token_actor(&viewer.token).unwrap().unwrap();
        assert_eq!(
            engine.start_agent_run(&viewer, "shop", ask("x")),
            Err(EngineError::Forbidden)
        );
    }

    #[test]
    fn keys_are_kept_sealed_and_shown_by_their_last_four() {
        let (engine, _, admin, ann) = setup();
        let change = AgentSettingsChange {
            default: Some(AgentKind::Codex),
            models: HashMap::from([(AgentKind::Codex, "gpt-5.5".to_owned())]),
            keys: HashMap::from([(AgentKind::Codex, "sk-openai-wxyz".to_owned())]),
        };
        assert_eq!(
            engine.set_agent_settings(&ann, &change),
            Err(EngineError::Forbidden)
        );
        let status = engine.set_agent_settings(&admin, &change).unwrap();
        assert!(status.enabled);
        assert_eq!(status.default, Some(AgentKind::Codex));
        let codex = status
            .agents
            .iter()
            .find(|a| a.agent == AgentKind::Codex)
            .unwrap();
        assert_eq!(
            (codex.key, codex.key_last4.as_deref(), codex.model.as_str()),
            ("settings", Some("wxyz"), "gpt-5.5")
        );
        let claude = status
            .agents
            .iter()
            .find(|a| a.agent == AgentKind::Claude)
            .unwrap();
        assert_eq!(claude.key, "env");
        let stored = engine
            .ports
            .settings
            .get("agent.key.codex")
            .unwrap()
            .unwrap();
        assert!(!stored.contains("wxyz"), "stored sealed");
        assert_eq!(
            engine
                .api_key(&AgentConfig::default(), AgentKind::Codex)
                .unwrap()
                .as_deref(),
            Some("sk-openai-wxyz")
        );

        let run = engine.start_agent_run(&ann, "shop", ask("go")).unwrap();
        assert_eq!(
            (run.agent, run.model.as_str()),
            (AgentKind::Codex, "gpt-5.5")
        );
    }

    #[test]
    fn costs_add_up_per_person() {
        let (engine, fakes, admin, ann) = setup();
        let run = engine.start_agent_run(&ann, "shop", ask("a")).unwrap();
        fakes.runner.finish(run.id, RunState::Done, None, Some(0.5));
        engine.agents_tick().unwrap();
        let run = engine
            .start_agent_run(
                &ann,
                "shop",
                RunInput {
                    agent: Some(AgentKind::Script),
                    ..ask("b")
                },
            )
            .unwrap();
        fakes
            .runner
            .finish(run.id, RunState::Failed, Some("boom"), None);
        engine.agents_tick().unwrap();
        assert_eq!(engine.agent_costs(&ann), Err(EngineError::Forbidden));
        let costs = engine.agent_costs(&admin).unwrap();
        assert_eq!(costs.runs.len(), 2);
        let person = &costs.people[0];
        assert_eq!(
            (
                person.name.as_str(),
                person.runs,
                person.cost_usd,
                person.unpriced
            ),
            ("Ann", 2, 0.5, 0)
        );
        assert_eq!(costs.runs[0].error.as_deref(), Some("boom"));
    }

    #[test]
    fn without_a_runner_the_agent_is_off() {
        let engine = engine();
        let status = engine.agent_status(&Actor::Local).unwrap();
        assert!(!status.enabled);
        engine
            .create_project(&Actor::Local, "shop", "", None)
            .unwrap();
        assert!(matches!(
            engine.start_agent_run(&Actor::Local, "shop", ask("x")),
            Err(EngineError::Unavailable(_))
        ));
    }
}
