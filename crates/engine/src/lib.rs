//! Commands (merge, restore, comment.add, ...) and permission checks over the model; storage, database and rendering behind traits.

mod agents;
mod branches;
mod browse;
mod captures;
mod comments;
mod design;
mod error;
mod guards;
mod handoff;
mod members;
mod notices;
pub mod policy;
mod ports;
mod preview;
mod projects;
mod providers;
mod realtime;
mod sharing;
mod sheet;

use std::sync::{Arc, RwLock};

use sharing::Live;

pub use agents::{
    AgentAccess, AgentChoice, AgentConfig, AgentSettingsChange, AgentStatus, Costs, PersonCost,
    RunInput,
};
pub use branches::BranchDetail;
pub use captures::{CaptureStatus, RenderResult, Rendered, THEMES};
pub use charet_design::{Edit, Element as DesignElement, Moves, Value as DesignValue, Variant};
pub use comments::{At, CommentFilter};
pub use design::{DesignInfo, DesignTarget, EditCheck, EditState, Edited, HandEdit};
pub use error::EngineError;
pub use handoff::{Baseline, Export, HandoffStatus, charet_only};
pub use members::{
    MemberChange, SESSION_SECS, SessionState, SignedIn, new_visitor_id, visitor_name,
};
pub use notices::{Inbox, WEBHOOK_EVENTS, WebhookSecret};
pub use ports::{
    Accounts, AgentRecords, AgentRunner, Author, BranchEvent, Build, BuildState, BuildStep,
    BuiltFile, Capture, Captures, Change, ChangeKind, Comments, FileChange, ImageKind, Joined,
    Kick, Merged, NewBranch, NewComment, NewNotification, NewReply, NewRun, NewUser, NewVersion,
    NewWebhook, Notices, Outgoing, Peer, ProjectStore, Realtime, Records, RenderJob, Renderer,
    RunUpdate, RunnerEvent, RunnerProgress, RunnerStart, SessionRow, Settings, Shot, StoreError,
    TunnelMode, TunnelOptions, Tunnels, WebhookSender, Workspace, WorkspaceError,
};
pub use preview::{PREVIEW_PREFIX, Preview, PreviewSettings, preview_path};
pub use projects::{Checkout, Import};
pub use providers::{
    ANTHROPIC_REDIRECT, DeviceAuth, DevicePoll, KeyCheck, KeyInfo, KeyProblem, OAuthStatus,
    OPENAI_DEVICE_REDIRECT, OPENAI_DEVICE_URL, Provider, ProviderError, ProviderId, ProviderNet,
    RunLogin, SignInMethod, SignInState, Tokens, percent_encode,
};
pub use sharing::{ShareMode, ShareRequest, Sharing, SharingStatus};
pub use sheet::SheetItem;

/// Everything the engine needs from the outside world.
#[derive(Clone)]
pub struct Ports {
    pub settings: Arc<dyn Settings>,
    pub records: Arc<dyn Records>,
    pub store: Arc<dyn ProjectStore>,
    pub workspace: Arc<dyn Workspace>,
    pub renderer: Arc<dyn Renderer>,
    pub captures: Arc<dyn Captures>,
    pub comments: Arc<dyn Comments>,
    pub accounts: Arc<dyn Accounts>,
    pub realtime: Arc<dyn Realtime>,
    pub notices: Arc<dyn Notices>,
    pub webhooks: Arc<dyn WebhookSender>,
    pub agents: Arc<dyn AgentRecords>,
}

impl std::fmt::Debug for Ports {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ports").finish_non_exhaustive()
    }
}

/// Runs commands. One instance per server, shared by every request.
#[derive(Clone)]
pub struct Engine {
    ports: Ports,
    local_mode: bool,
    preview: Option<Arc<PreviewSettings>>,
    /// What can change while running: the address (sharing) and the owner.
    live: Arc<RwLock<Live>>,
    tunnels: Option<Arc<dyn Tunnels>>,
    secret: Option<Arc<[u8; 32]>>,
    /// Whether merges wait for the check of what they give (on by default, with previews).
    merge_check: bool,
    /// The web agent, when a runner is configured.
    agents: Option<agents::Agents>,
    /// Provider accounts (API keys and sign-ins), when the server can reach the providers.
    providers: Option<providers::Providers>,
    /// One bookkeeping pass over runs at a time.
    agent_turn: Arc<std::sync::Mutex<()>>,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("local_mode", &self.local_mode)
            .finish_non_exhaustive()
    }
}

impl Engine {
    pub fn new(ports: Ports, local_mode: bool) -> Self {
        Self {
            ports,
            local_mode,
            preview: None,
            live: Arc::default(),
            tunnels: None,
            secret: None,
            merge_check: true,
            agents: None,
            providers: None,
            agent_turn: Arc::default(),
        }
    }

    /// The server key: webhook signing secrets are derived from it.
    #[must_use]
    pub fn with_secret(mut self, key: [u8; 32]) -> Self {
        self.secret = Some(Arc::new(key));
        self
    }

    /// Enables previews: built commits are served from `settings.origin` with signed tokens.
    #[must_use]
    pub fn with_preview(mut self, settings: PreviewSettings) -> Self {
        self.preview = Some(Arc::new(settings));
        self
    }

    /// Lets sharing run tunnels (Tailscale) instead of only taking an address set up by hand.
    #[must_use]
    pub fn with_tunnels(mut self, tunnels: Arc<dyn Tunnels>) -> Self {
        self.tunnels = Some(tunnels);
        self
    }

    /// Enables the web agent: runs go to `runner` (the `charet-agent` service).
    #[must_use]
    pub fn with_agents(mut self, runner: Arc<dyn AgentRunner>, config: AgentConfig) -> Self {
        self.agents = Some(agents::Agents { runner, config });
        self
    }

    /// Lets admins check provider API keys and sign in to provider accounts: the calls to the
    /// providers go through `net`.
    #[must_use]
    pub fn with_providers(mut self, net: Arc<dyn ProviderNet>) -> Self {
        self.providers = Some(providers::Providers::new(net));
        self
    }

    /// Turns the merge check off (a server without the build service).
    #[must_use]
    pub fn with_merge_check(mut self, on: bool) -> Self {
        self.merge_check = on;
        self
    }

    /// Where people open the web tool, for links (`charet url`, notifications).
    #[must_use]
    pub fn with_tool_url(self, url: &str) -> Self {
        self.write_live(|live| live.home_url = Some(url.trim_end_matches('/').into()));
        self
    }

    pub(crate) fn read_live(&self) -> std::sync::RwLockReadGuard<'_, Live> {
        self.live
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn write_live(&self, change: impl FnOnce(&mut Live)) {
        let mut live = self
            .live
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        change(&mut live);
    }

    /// Versions, links and whether the internal services answer.
    pub fn system(&self) -> Result<System, StoreError> {
        let state = |result: Result<(), WorkspaceError>| match result {
            Ok(()) => ServiceState::Ok,
            Err(WorkspaceError::Unavailable(e) | WorkspaceError::Failed(e)) => {
                ServiceState::Failing(e)
            }
        };
        Ok(System {
            latest: self.latest_version()?,
            tool_url: self.tool_url(),
            preview_url: self.preview.as_ref().map(|p| p.origin.clone()),
            local_mode: self.local_mode,
            owner_signed_in: self.owner_signed_in(),
            workspace: state(self.ports.workspace.health()),
            render: state(self.ports.renderer.health()),
        })
    }

    /// True when previews are served from the tool's own address (one address for both).
    pub fn previews_on_tool(&self) -> bool {
        match (&self.preview, self.tool_url()) {
            (Some(preview), Some(tool)) => preview.origin.trim_end_matches('/') == tool,
            _ => false,
        }
    }

    /// Where people open the web tool, when known.
    pub fn tool_url(&self) -> Option<String> {
        let live = self.read_live();
        live.sharing
            .as_ref()
            .map(|s| s.url.clone())
            .or_else(|| live.home_url.as_deref().map(str::to_owned))
    }

    /// True for a local charet: the local door (this machine) acts as the owner.
    pub fn local_mode(&self) -> bool {
        self.local_mode
    }

    /// The latest released version, if an update check has recorded one.
    pub fn latest_version(&self) -> Result<Option<String>, StoreError> {
        self.ports.settings.get(LATEST_VERSION)
    }

    pub fn record_latest_version(&self, version: &str) -> Result<(), StoreError> {
        self.ports.settings.set(LATEST_VERSION, version)
    }
}

const LATEST_VERSION: &str = "update.latest";

/// What `Engine::system` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct System {
    pub latest: Option<String>,
    pub tool_url: Option<String>,
    pub preview_url: Option<String>,
    pub local_mode: bool,
    /// The owner has come in with their token at least once.
    pub owner_signed_in: bool,
    pub workspace: ServiceState,
    pub render: ServiceState,
}

/// Whether an internal service answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceState {
    Ok,
    Failing(String),
}

#[cfg(test)]
pub(crate) mod testing;

#[cfg(test)]
mod tests {
    use crate::testing::engine;

    #[test]
    fn latest_version_round_trips() {
        let engine = engine();
        assert_eq!(engine.latest_version(), Ok(None));
        engine.record_latest_version("1.2.3").unwrap();
        assert_eq!(engine.latest_version(), Ok(Some("1.2.3".into())));
    }
}
