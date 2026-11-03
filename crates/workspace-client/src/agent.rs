//! `charet-agent`: starts web agent runs in containers and reports what they do.
//!
//! - `POST /v1/runs` starts one (`202`), `GET /v1/runs/{id}?after=N` reports events after `N`
//!   (`404` when it does not know the run), `POST /v1/runs/{id}/cancel` stops it.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use charet_engine::{
    AgentRunner, RunLogin, RunnerEvent, RunnerProgress, RunnerStart, WorkspaceError,
};
use charet_model::{MessageKind, RunState};
use serde::{Deserialize, Serialize};
use ureq::Agent;

/// A `charet-agent` at `url`, authenticated with `token`.
#[derive(Clone)]
pub struct AgentClient {
    url: String,
    token: Option<String>,
    agent: Agent,
}

impl std::fmt::Debug for AgentClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentClient")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl AgentClient {
    pub fn new(url: &str, token: Option<String>) -> Self {
        let agent = Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(3)))
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .new_agent();
        Self {
            url: url.trim_end_matches('/').to_owned(),
            token,
            agent,
        }
    }

    fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<(u16, String), WorkspaceError> {
        let url = format!("{}{path}", self.url);
        let auth = self.token.as_ref().map(|t| format!("Bearer {t}"));
        let unavailable =
            |e: ureq::Error| WorkspaceError::Unavailable(format!("charet-agent: {e}"));
        let mut response = match (method, body) {
            ("POST", Some(body)) => {
                let mut request = self.agent.post(url);
                if let Some(auth) = &auth {
                    request = request.header("authorization", auth);
                }
                request.send_json(body).map_err(unavailable)?
            }
            ("POST", None) => {
                let mut request = self.agent.post(url);
                if let Some(auth) = &auth {
                    request = request.header("authorization", auth);
                }
                request.send_empty().map_err(unavailable)?
            }
            _ => {
                let mut request = self.agent.get(url);
                if let Some(auth) = &auth {
                    request = request.header("authorization", auth);
                }
                request.call().map_err(unavailable)?
            }
        };
        let status = response.status().as_u16();
        let text = response.body_mut().read_to_string().map_err(unavailable)?;
        Ok((status, text))
    }
}

fn failed(status: u16, body: &str) -> WorkspaceError {
    WorkspaceError::Failed(format!(
        "status {status}: {}",
        body.chars().take(300).collect::<String>()
    ))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireStart<'a> {
    id: u64,
    agent: charet_model::AgentKind,
    model: &'a str,
    api_key: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    oauth: Option<WireLogin<'a>>,
    project: &'a str,
    branch: u32,
    token: &'a str,
    prompt: &'a str,
    context: &'a serde_json::Value,
    images: Vec<WireImage<'a>>,
    history: Vec<WireTurn<'a>>,
}

/// A subscription login in place of an API key (what `charet-agent` calls `RunOAuth`).
#[derive(Serialize)]
#[serde(
    tag = "provider",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
enum WireLogin<'a> {
    Anthropic {
        access_token: &'a str,
    },
    Openai {
        access_token: &'a str,
        id_token: &'a str,
        account_id: &'a str,
    },
}

impl<'a> From<&'a RunLogin> for WireLogin<'a> {
    fn from(login: &'a RunLogin) -> Self {
        match login {
            RunLogin::Anthropic { access_token } => Self::Anthropic { access_token },
            RunLogin::Openai {
                access_token,
                id_token,
                account_id,
            } => Self::Openai {
                access_token,
                id_token,
                account_id,
            },
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireImage<'a> {
    media_type: &'a str,
    data: String,
}

#[derive(Serialize)]
struct WireTurn<'a> {
    role: &'a str,
    text: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireProgress {
    #[serde(default)]
    events: Vec<WireEvent>,
    next: u32,
    finished: Option<RunState>,
    error: Option<String>,
    #[serde(default)]
    tokens_in: u64,
    #[serde(default)]
    tokens_out: u64,
    cost_usd: Option<f64>,
}

#[derive(Deserialize)]
struct WireEvent {
    kind: MessageKind,
    text: String,
    tool: Option<String>,
}

fn parse_progress(body: &str) -> Result<RunnerProgress, WorkspaceError> {
    let wire: WireProgress = serde_json::from_str(body)
        .map_err(|e| WorkspaceError::Failed(format!("bad progress JSON: {e}")))?;
    Ok(RunnerProgress {
        events: wire
            .events
            .into_iter()
            .map(|e| RunnerEvent {
                kind: e.kind,
                text: e.text,
                tool: e.tool,
            })
            .collect(),
        next: wire.next,
        finished: wire.finished,
        error: wire.error,
        tokens_in: wire.tokens_in,
        tokens_out: wire.tokens_out,
        cost_usd: wire.cost_usd,
    })
}

/// The body of `POST /v1/runs`.
fn start_body(run: &RunnerStart) -> Result<serde_json::Value, WorkspaceError> {
    let body = WireStart {
        id: run.id,
        agent: run.agent,
        model: &run.model,
        api_key: &run.api_key,
        oauth: run.oauth.as_ref().map(WireLogin::from),
        project: &run.project,
        branch: run.branch,
        token: &run.token,
        prompt: &run.prompt,
        context: &run.context,
        images: run
            .images
            .iter()
            .map(|i| WireImage {
                media_type: &i.media_type,
                data: STANDARD.encode(&i.data),
            })
            .collect(),
        history: run
            .history
            .iter()
            .map(|(role, text)| WireTurn { role, text })
            .collect(),
    };
    serde_json::to_value(&body).map_err(|e| WorkspaceError::Failed(e.to_string()))
}

impl AgentRunner for AgentClient {
    fn start(&self, run: &RunnerStart) -> Result<(), WorkspaceError> {
        let body = start_body(run)?;
        match self.call("POST", "/v1/runs", Some(&body))? {
            (200..=202, _) => Ok(()),
            (status, body) => Err(failed(status, &body)),
        }
    }

    fn poll(&self, id: u64, after: u32) -> Result<Option<RunnerProgress>, WorkspaceError> {
        match self.call("GET", &format!("/v1/runs/{id}?after={after}"), None)? {
            (200, body) => parse_progress(&body).map(Some),
            (404, _) => Ok(None),
            (status, body) => Err(failed(status, &body)),
        }
    }

    fn cancel(&self, id: u64) -> Result<(), WorkspaceError> {
        match self.call("POST", &format!("/v1/runs/{id}/cancel"), None)? {
            (200 | 202 | 404, _) => Ok(()),
            (status, body) => Err(failed(status, &body)),
        }
    }

    fn health(&self) -> Result<(), WorkspaceError> {
        match self.call("GET", "/health", None)? {
            (200, _) => Ok(()),
            (status, body) => Err(failed(status, &body)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_progress() {
        let progress = parse_progress(
            r#"{"events":[{"kind":"tool","text":"Edit src/a.tsx","tool":"Edit"}],"next":3,
               "finished":"done","error":null,"tokensIn":5,"tokensOut":2,"costUsd":0.01}"#,
        )
        .unwrap();
        assert_eq!(progress.events[0].tool.as_deref(), Some("Edit"));
        assert_eq!(
            (progress.next, progress.finished),
            (3, Some(RunState::Done))
        );
        assert!(parse_progress(r#"{"events":[]}"#).is_err());
    }

    fn start(
        agent: charet_model::AgentKind,
        api_key: &str,
        oauth: Option<RunLogin>,
    ) -> RunnerStart {
        RunnerStart {
            id: 7,
            agent,
            model: String::new(),
            api_key: api_key.into(),
            oauth,
            project: "shop".into(),
            branch: 1,
            token: "t".into(),
            prompt: "go".into(),
            context: serde_json::Value::Null,
            images: Vec::new(),
            history: Vec::new(),
        }
    }

    #[test]
    fn a_run_carries_a_key_or_a_login_never_both() {
        use charet_model::AgentKind;
        let keyed = start_body(&start(AgentKind::Claude, "sk-1", None)).unwrap();
        assert_eq!(keyed["apiKey"], "sk-1");
        assert!(keyed.get("oauth").is_none(), "no login field with a key");

        let claude = start_body(&start(
            AgentKind::Claude,
            "",
            Some(RunLogin::Anthropic {
                access_token: "at".into(),
            }),
        ))
        .unwrap();
        assert_eq!(claude["apiKey"], "");
        assert_eq!(
            claude["oauth"],
            serde_json::json!({ "provider": "anthropic", "accessToken": "at" })
        );

        let codex = start_body(&start(
            AgentKind::Codex,
            "",
            Some(RunLogin::Openai {
                access_token: "at".into(),
                id_token: "it".into(),
                account_id: "acct".into(),
            }),
        ))
        .unwrap();
        assert_eq!(
            codex["oauth"],
            serde_json::json!({
                "provider": "openai", "accessToken": "at", "idToken": "it", "accountId": "acct"
            })
        );
    }

    #[test]
    fn a_missing_runner_is_unavailable() {
        let client = AgentClient::new("http://127.0.0.1:1", None);
        assert!(matches!(
            client.health(),
            Err(WorkspaceError::Unavailable(_))
        ));
    }
}
