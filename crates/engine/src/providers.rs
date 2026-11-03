//! The accounts the web agent runs on: per provider an API key (the provider checks it before it is
//! saved) or a subscription login (Claude, `ChatGPT`) done in a new browser window.
//!
//! - Secrets are stored encrypted with the server key, like the agents' API keys always were. Only
//!   the last 4 characters of a key are ever shown.
//! - A login is the provider's own OAuth flow, driven by the server (the same flows the vendors'
//!   command line tools use, so no redirect has to reach this server): `ChatGPT` with a device code
//!   (the person types it at the provider's page), Claude with a code the provider's page shows
//!   (the person pastes it back).
//! - The refresh token never leaves the server. Before each run the server renews the access token
//!   when it is about to expire and hands the container only that.
//! - Network calls go through [`ProviderNet`]; this module decides, stores and refreshes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use charet_model::{Actor, AgentKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::agents::{kind_name, last4};
use crate::members::now;
use crate::policy::{Command, can};
use crate::{AgentConfig, Engine, EngineError};

/// A login nobody finishes is dropped after this many seconds.
const LOGIN_TIMEOUT: u64 = 15 * 60;
/// Extra seconds, beyond the longest a run may take, that a login must still have when a run
/// starts: its container cannot renew the token itself (it never sees the refresh token).
const RENEW_MARGIN: u64 = 5 * 60;

/// The page where the person types the `ChatGPT` device code.
pub const OPENAI_DEVICE_URL: &str = "https://auth.openai.com/codex/device";
/// Where the device flow ends the code exchange (not visited by anyone).
pub const OPENAI_DEVICE_REDIRECT: &str = "https://auth.openai.com/deviceauth/callback";

const ANTHROPIC_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const ANTHROPIC_AUTHORIZE: &str = "https://claude.ai/oauth/authorize";
/// The provider's page that shows the code to copy.
pub const ANTHROPIC_REDIRECT: &str = "https://platform.claude.com/oauth/code/callback";
const ANTHROPIC_SCOPES: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";

/// A company the web agent can run on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Anthropic,
    Openai,
    Google,
}

impl ProviderId {
    pub const ALL: [Self; 3] = [Self::Anthropic, Self::Openai, Self::Google];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Openai => "openai",
            Self::Google => "google",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name() == text)
    }

    fn of_agent(agent: AgentKind) -> Option<Self> {
        match agent {
            AgentKind::Claude => Some(Self::Anthropic),
            AgentKind::Codex => Some(Self::Openai),
            AgentKind::Script => None,
        }
    }

    fn has_sign_in(self) -> bool {
        !matches!(self, Self::Google)
    }

    /// Where a saved API key lives: the setting the agents' keys always used (`agent.key.claude`).
    fn key_setting(self) -> String {
        format!("agent.key.{}", self.env_name())
    }

    /// Its name in the environment keys of [`AgentConfig`] and in the agents' settings.
    fn env_name(self) -> &'static str {
        match self {
            Self::Anthropic => kind_name(AgentKind::Claude),
            Self::Openai => kind_name(AgentKind::Codex),
            Self::Google => "google",
        }
    }

    fn login_setting(self) -> String {
        format!("provider.login.{}", self.name())
    }

    fn use_setting(self) -> String {
        format!("provider.use.{}", self.name())
    }
}

/// Why a key does not work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyProblem {
    Invalid,
    Forbidden,
    RateLimited,
    Unreachable,
}

/// The provider's answer to "does this key work?".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyCheck {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<KeyProblem>,
    /// One line for people.
    pub message: String,
    /// How many models the key can use.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<u32>,
}

/// Where a sign-in stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignInState {
    SignedOut,
    Waiting,
    SignedIn,
    Failed,
}

/// How the person completes a sign-in that is waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignInMethod {
    /// Open the address and type the user code there; the server notices when it is done.
    Device,
    /// Open the address; the page it ends on shows a code to paste back.
    Code,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthStatus {
    pub state: SignInState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<SignInMethod>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl OAuthStatus {
    fn only(state: SignInState) -> Self {
        Self {
            state,
            method: None,
            url: None,
            user_code: None,
            account: None,
            error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyInfo {
    /// `env`, `settings` or `none`.
    pub source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last4: Option<String>,
}

/// A provider as the admins' page shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub id: ProviderId,
    pub key: KeyInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth: Option<OAuthStatus>,
    /// What the web agent uses now: `key`, `oauth` or `none`.
    #[serde(rename = "use")]
    pub using: &'static str,
}

/// The `ChatGPT` device code a person types at [`OPENAI_DEVICE_URL`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceAuth {
    pub device_auth_id: String,
    pub user_code: String,
    pub interval_secs: u64,
}

/// One look at whether the person finished the device flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DevicePoll {
    Pending,
    SlowDown,
    Done {
        authorization_code: String,
        code_verifier: String,
    },
    Failed(String),
}

/// What a token endpoint gives.
#[derive(Clone, PartialEq, Eq)]
pub struct Tokens {
    pub access: String,
    /// The new refresh token; `None` when the provider left it as it was (a renewal may).
    pub refresh: Option<String>,
    pub id_token: Option<String>,
    pub expires_in: u64,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens").finish_non_exhaustive()
    }
}

/// Why a call to a provider failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// The provider said no (a used or expired code, a revoked login).
    Rejected(String),
    /// It could not be reached or answered something unexpected.
    Unavailable(String),
}

/// The calls to the providers: HTTP lives behind this (`apps/charet-server`), the rules above it.
pub trait ProviderNet: Send + Sync {
    /// Asks the provider whether `key` works. Never an error: a failure is the answer.
    fn check_key(&self, provider: ProviderId, key: &str) -> KeyCheck;
    fn openai_device_start(&self) -> Result<DeviceAuth, ProviderError>;
    fn openai_device_poll(&self, device: &DeviceAuth) -> Result<DevicePoll, ProviderError>;
    fn openai_exchange(
        &self,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<Tokens, ProviderError>;
    fn openai_refresh(&self, refresh_token: &str) -> Result<Tokens, ProviderError>;
    fn anthropic_exchange(
        &self,
        code: &str,
        state: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<Tokens, ProviderError>;
    fn anthropic_refresh(&self, refresh_token: &str) -> Result<Tokens, ProviderError>;
}

/// What a run container is given instead of an API key: a subscription login's access token.
#[derive(Clone, PartialEq, Eq)]
pub enum RunLogin {
    Anthropic {
        access_token: String,
    },
    Openai {
        access_token: String,
        id_token: String,
        account_id: String,
    },
}

impl std::fmt::Debug for RunLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunLogin").finish_non_exhaustive()
    }
}

/// What a run starts with: an API key, or a login (then the key is empty).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Credential {
    pub api_key: String,
    pub login: Option<RunLogin>,
}

/// A saved login (sealed in the settings).
#[derive(Serialize, Deserialize)]
struct Login {
    access: String,
    refresh: String,
    #[serde(default)]
    id_token: String,
    /// Unix seconds.
    expires: u64,
    #[serde(default)]
    account: String,
    #[serde(default)]
    account_id: String,
}

enum Pending {
    Device {
        device: DeviceAuth,
        started: u64,
        next_poll: u64,
        generation: u64,
        /// The code has been given and is being exchanged for tokens: nothing else asks the
        /// provider about this attempt until that is done (the code is used up by the exchange).
        exchanging: bool,
    },
    Code {
        verifier: String,
        started: u64,
        generation: u64,
    },
    Failed(String),
}

/// The network port and what is in flight.
#[derive(Clone)]
pub(crate) struct Providers {
    net: Arc<dyn ProviderNet>,
    pending: Arc<Mutex<HashMap<ProviderId, Pending>>>,
    /// Renewing a login replaces its refresh token: one at a time.
    renewing: Arc<Mutex<()>>,
    /// The newest sign-in action (a start or a cancel) asked for, per provider. It is noted before
    /// the action waits for anything, so an attempt that finishes meanwhile is already out of date.
    latest: Arc<Mutex<HashMap<ProviderId, u64>>>,
    next_generation: Arc<AtomicU64>,
}

impl Providers {
    pub(crate) fn new(net: Arc<dyn ProviderNet>) -> Self {
        Self {
            net,
            pending: Arc::default(),
            renewing: Arc::default(),
            latest: Arc::default(),
            next_generation: Arc::default(),
        }
    }

    /// What a sign-in of `provider` waits for, asked from the provider.
    fn new_attempt(&self, provider: ProviderId, generation: u64) -> Result<Pending, EngineError> {
        let started = now();
        match provider {
            ProviderId::Openai => {
                let device = self.net.openai_device_start().map_err(sign_in_error)?;
                Ok(Pending::Device {
                    next_poll: started + device.interval_secs.max(1),
                    device,
                    started,
                    generation,
                    exchanging: false,
                })
            }
            ProviderId::Anthropic => Ok(Pending::Code {
                verifier: random_verifier()?,
                started,
                generation,
            }),
            ProviderId::Google => Err(EngineError::Invalid("this provider has no sign-in".into())),
        }
    }

    /// Notes that a start or cancel was asked for: attempts of an earlier generation are not wanted
    /// from now on, even while this action still waits for the pending state.
    fn reserve(&self, provider: ProviderId) -> u64 {
        let generation = self.next_generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.latest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(provider, generation);
        generation
    }

    /// A start of this `generation` failed. If it is still the newest action, the attempt that was
    /// waiting (if any) is the one that counts again, as if the start had never been asked for.
    fn keep_waiting_attempt(&self, provider: ProviderId, generation: u64) {
        let mut pending = self.pending();
        if !self.is_latest(provider, generation) {
            return;
        }
        if let Some(Pending::Device { generation: g, .. } | Pending::Code { generation: g, .. }) =
            pending.get_mut(&provider)
        {
            *g = generation;
        }
    }

    /// Whether the attempt of this `generation` is still the one pending and no newer start or
    /// cancel was asked for.
    fn wanted(
        &self,
        pending: &HashMap<ProviderId, Pending>,
        provider: ProviderId,
        generation: u64,
    ) -> bool {
        let Some(Pending::Device { generation: g, .. } | Pending::Code { generation: g, .. }) =
            pending.get(&provider)
        else {
            return false;
        };
        *g == generation && self.is_latest(provider, generation)
    }

    /// Whether no start or cancel newer than this `generation` was asked for.
    fn is_latest(&self, provider: ProviderId, generation: u64) -> bool {
        self.latest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&provider)
            == Some(&generation)
    }

    /// The pending sign-ins. Lock order: `pending`, then `latest` (never the other way round).
    /// A decision about an attempt and its effect (storing a login) happen under this one lock, so
    /// that they cannot be separated by a start or a cancel.
    fn pending(&self) -> std::sync::MutexGuard<'_, HashMap<ProviderId, Pending>> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Engine {
    /// Every provider with its API key and sign-in. Admins only.
    pub fn providers(&self, actor: &Actor) -> Result<Vec<Provider>, EngineError> {
        admin_only(actor)?;
        ProviderId::ALL
            .into_iter()
            .map(|p| self.provider_view(&self.agent_config(), p))
            .collect()
    }

    /// Asks the provider whether `key` works. Admins only.
    pub fn check_provider_key(
        &self,
        actor: &Actor,
        provider: ProviderId,
        key: &str,
    ) -> Result<KeyCheck, EngineError> {
        admin_only(actor)?;
        let key = clean_key(key)?;
        Ok(self.provider_net()?.net.check_key(provider, key))
    }

    /// Saves an API key once the provider accepted it. Admins only.
    pub fn save_provider_key(
        &self,
        actor: &Actor,
        provider: ProviderId,
        key: &str,
    ) -> Result<Provider, EngineError> {
        admin_only(actor)?;
        let key = clean_key(key)?;
        let config = self.agent_config();
        if config.env_keys.contains_key(provider.env_name()) {
            return Err(EngineError::Invalid(
                "this key is set in the server's environment; change it there".into(),
            ));
        }
        let answer = self.provider_net()?.net.check_key(provider, key);
        if !answer.ok {
            return Err(EngineError::Invalid(answer.message));
        }
        let sealed = self.seal(key)?;
        self.ports.settings.set(&provider.key_setting(), &sealed)?;
        self.provider_view(&config, provider)
    }

    /// Removes the saved API key. Admins only.
    pub fn remove_provider_key(
        &self,
        actor: &Actor,
        provider: ProviderId,
    ) -> Result<(), EngineError> {
        admin_only(actor)?;
        self.ports.settings.set(&provider.key_setting(), "")?;
        Ok(())
    }

    /// Chooses whether the web agent uses the key or the signed-in account. Admins only.
    pub fn choose_provider_auth(
        &self,
        actor: &Actor,
        provider: ProviderId,
        using: &str,
    ) -> Result<Provider, EngineError> {
        admin_only(actor)?;
        let config = self.agent_config();
        let (has_key, signed_in) = self.provider_credentials(&config, provider)?;
        match using {
            "key" if has_key => {}
            "oauth" if signed_in => {}
            "key" => return Err(EngineError::Invalid("there is no API key".into())),
            "oauth" => return Err(EngineError::Invalid("nobody is signed in".into())),
            _ => return Err(EngineError::Invalid("use is `key` or `oauth`".into())),
        }
        self.ports.settings.set(&provider.use_setting(), using)?;
        self.provider_view(&config, provider)
    }

    /// Starts a sign-in: where to go, and what to type or paste there. Admins only.
    pub fn start_provider_sign_in(
        &self,
        actor: &Actor,
        provider: ProviderId,
    ) -> Result<OAuthStatus, EngineError> {
        admin_only(actor)?;
        let providers = self.provider_net()?;
        // Noted before the provider is asked anything, so an attempt that finishes meanwhile is not
        // accepted under the new one.
        let generation = providers.reserve(provider);
        let pending = providers
            .new_attempt(provider, generation)
            .inspect_err(|_| providers.keep_waiting_attempt(provider, generation))?;
        {
            let mut state = providers.pending();
            // A newer start or a cancel was asked for while the provider answered: this attempt
            // must not overwrite what they made (or bring back what a cancel removed).
            if !providers.is_latest(provider, generation) {
                return Err(EngineError::Invalid(
                    "the sign-in was cancelled or restarted while it started; try again".into(),
                ));
            }
            state.insert(provider, pending);
        }
        self.sign_in_view(provider)
    }

    /// Where a sign-in stands. A device sign-in asks the provider (at most as often as it allows).
    /// Admins only.
    pub fn provider_sign_in(
        &self,
        actor: &Actor,
        provider: ProviderId,
    ) -> Result<OAuthStatus, EngineError> {
        admin_only(actor)?;
        if provider.has_sign_in() {
            self.advance_sign_in(provider);
        }
        self.sign_in_view(provider)
    }

    /// Finishes a sign-in that shows a code: `code` is what the provider's page showed (`code#state`),
    /// or the address it ended on. Admins only.
    pub fn finish_provider_sign_in(
        &self,
        actor: &Actor,
        provider: ProviderId,
        code: &str,
    ) -> Result<OAuthStatus, EngineError> {
        admin_only(actor)?;
        let providers = self.provider_net()?;
        let (verifier, generation) = match providers.pending().get(&provider) {
            Some(Pending::Code {
                verifier,
                generation,
                ..
            }) => (verifier.clone(), *generation),
            _ => {
                return Err(EngineError::Invalid(
                    "no sign-in is waiting for a code; start one first".into(),
                ));
            }
        };
        let (code, state) = parse_authorization_input(code);
        let Some(code) = code else {
            return Err(EngineError::Invalid(
                "paste the code the sign-in page shows".into(),
            ));
        };
        if state.as_deref().is_some_and(|s| s != verifier) {
            return Err(EngineError::Invalid(
                "that code belongs to another sign-in; start again".into(),
            ));
        }
        let tokens = providers
            .net
            .anthropic_exchange(
                &code,
                state.as_deref().unwrap_or(&verifier),
                &verifier,
                ANTHROPIC_REDIRECT,
            )
            .map_err(|e| match e {
                ProviderError::Rejected(message) => EngineError::Invalid(format!(
                    "the provider did not accept the code ({message}); start again"
                )),
                ProviderError::Unavailable(message) => EngineError::Unavailable(message),
            })?;
        {
            let mut pending = providers.pending();
            // Cancelled or restarted while the code was exchanged: this login is not wanted now.
            if !providers.wanted(&pending, provider, generation) {
                return Err(EngineError::Invalid(
                    "that sign-in was cancelled or restarted; start again".into(),
                ));
            }
            self.store_login(provider, &tokens, None)?;
            pending.remove(&provider);
        }
        self.sign_in_view(provider)
    }

    /// Cancels a sign-in that waits (or dismisses one that failed); with none under way, signs the
    /// account out. Admins only.
    pub fn end_provider_sign_in(
        &self,
        actor: &Actor,
        provider: ProviderId,
    ) -> Result<(), EngineError> {
        admin_only(actor)?;
        if let Some(providers) = &self.providers {
            // Noted first: an attempt finishing at this very moment is not accepted behind it.
            providers.reserve(provider);
            if providers.pending().remove(&provider).is_some() {
                return Ok(());
            }
        }
        // A renewal in flight would store its tokens after this and bring the login back.
        let _renewal_done = self.providers.as_ref().map(|providers| {
            providers
                .renewing
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        });
        self.ports.settings.set(&provider.login_setting(), "")?;
        self.ports.settings.set(&provider.use_setting(), "")?;
        Ok(())
    }

    /// What a run of `agent` starts with, renewing the login when it is about to expire. `None`:
    /// there is neither a key nor a login.
    pub(crate) fn run_credential(
        &self,
        config: &AgentConfig,
        agent: AgentKind,
    ) -> Result<Option<Credential>, EngineError> {
        let Some(provider) = ProviderId::of_agent(agent) else {
            return Ok(None);
        };
        let (has_key, signed_in) = self.provider_credentials(config, provider)?;
        match self.using(provider, has_key, signed_in)? {
            "key" => Ok(self.api_key(config, agent)?.map(|api_key| Credential {
                api_key,
                login: None,
            })),
            "oauth" => {
                let login = self.fresh_login(config, provider)?;
                if provider == ProviderId::Openai
                    && login.as_ref().is_some_and(|l| l.id_token.is_empty())
                {
                    return Err(EngineError::Unavailable(
                        "the ChatGPT login has no id token; sign in again in Settings".into(),
                    ));
                }
                Ok(login.map(|login| Credential {
                    api_key: String::new(),
                    login: Some(run_login(provider, &login)),
                }))
            }
            _ => Ok(None),
        }
    }

    /// Whether an agent can run: a key or a login for its provider.
    pub(crate) fn agent_has_credential(
        &self,
        config: &AgentConfig,
        agent: AgentKind,
    ) -> Result<bool, EngineError> {
        let Some(provider) = ProviderId::of_agent(agent) else {
            return Ok(false);
        };
        let (has_key, signed_in) = self.provider_credentials(config, provider)?;
        Ok(has_key || signed_in)
    }

    fn agent_config(&self) -> AgentConfig {
        self.agents
            .as_ref()
            .map(|a| a.config.clone())
            .unwrap_or_default()
    }

    fn provider_net(&self) -> Result<&Providers, EngineError> {
        self.providers.as_ref().ok_or_else(|| {
            EngineError::Unavailable("this server cannot reach the providers".into())
        })
    }

    /// (an API key is set, someone is signed in)
    fn provider_credentials(
        &self,
        config: &AgentConfig,
        provider: ProviderId,
    ) -> Result<(bool, bool), EngineError> {
        let has_key = config.env_keys.contains_key(provider.env_name())
            || self.saved_secret(&provider.key_setting())?.is_some();
        let signed_in = provider.has_sign_in() && self.stored_login(provider)?.is_some();
        Ok((has_key, signed_in))
    }

    /// The choice, falling back to the key, then the login.
    fn using(
        &self,
        provider: ProviderId,
        has_key: bool,
        signed_in: bool,
    ) -> Result<&'static str, EngineError> {
        let chosen = self.ports.settings.get(&provider.use_setting())?;
        Ok(match (chosen.as_deref(), has_key, signed_in) {
            (Some("oauth"), _, true) | (_, false, true) => "oauth",
            (_, true, _) => "key",
            _ => "none",
        })
    }

    fn provider_view(
        &self,
        config: &AgentConfig,
        provider: ProviderId,
    ) -> Result<Provider, EngineError> {
        let (has_key, signed_in) = self.provider_credentials(config, provider)?;
        let key = if let Some(key) = config.env_keys.get(provider.env_name()) {
            KeyInfo {
                source: "env",
                last4: Some(last4(key)),
            }
        } else if let Some(key) = self.saved_secret(&provider.key_setting())? {
            KeyInfo {
                source: "settings",
                last4: Some(last4(&key)),
            }
        } else {
            KeyInfo {
                source: "none",
                last4: None,
            }
        };
        Ok(Provider {
            id: provider,
            key,
            oauth: if provider.has_sign_in() {
                Some(self.sign_in_view(provider)?)
            } else {
                None
            },
            using: self.using(provider, has_key, signed_in)?,
        })
    }

    fn sign_in_view(&self, provider: ProviderId) -> Result<OAuthStatus, EngineError> {
        if let Some(providers) = &self.providers
            && let Some(pending) = providers.pending().get(&provider)
        {
            return Ok(match pending {
                Pending::Device { device, .. } => OAuthStatus {
                    method: Some(SignInMethod::Device),
                    url: Some(OPENAI_DEVICE_URL.into()),
                    user_code: Some(device.user_code.clone()),
                    ..OAuthStatus::only(SignInState::Waiting)
                },
                Pending::Code { verifier, .. } => OAuthStatus {
                    method: Some(SignInMethod::Code),
                    url: Some(anthropic_authorize_url(verifier)),
                    ..OAuthStatus::only(SignInState::Waiting)
                },
                Pending::Failed(error) => OAuthStatus {
                    error: Some(error.clone()),
                    ..OAuthStatus::only(SignInState::Failed)
                },
            });
        }
        Ok(match self.stored_login(provider)? {
            Some(login) => OAuthStatus {
                account: (!login.account.is_empty()).then_some(login.account),
                ..OAuthStatus::only(SignInState::SignedIn)
            },
            None => OAuthStatus::only(SignInState::SignedOut),
        })
    }

    /// One step of a waiting sign-in: a device sign-in asks the provider (at most as often as it
    /// allows); one that waited too long is given up.
    fn advance_sign_in(&self, provider: ProviderId) {
        let Some(providers) = &self.providers else {
            return;
        };
        let (device, generation) = {
            let mut pending = providers.pending();
            let at = now();
            let too_long = match pending.get(&provider) {
                Some(Pending::Device {
                    exchanging: true, ..
                }) => return,
                Some(Pending::Device { started, .. } | Pending::Code { started, .. }) => {
                    at > started + LOGIN_TIMEOUT
                }
                _ => return,
            };
            if too_long {
                pending.insert(
                    provider,
                    Pending::Failed("the sign-in took too long; start again".into()),
                );
                return;
            }
            match pending.get_mut(&provider) {
                Some(Pending::Device {
                    device,
                    next_poll,
                    generation,
                    exchanging: false,
                    ..
                }) if at >= *next_poll => {
                    // Claim this look: a second page asking meanwhile waits for the next one.
                    *next_poll = at + device.interval_secs.max(1);
                    (device.clone(), *generation)
                }
                _ => return,
            }
        };
        let outcome = providers.net.openai_device_poll(&device);
        let mut pending = providers.pending();
        // Cancelled or restarted while the provider was asked: leave the newer state alone.
        if !providers.wanted(&pending, provider, generation) {
            return;
        }
        // Another look already got the code and is exchanging it: its outcome is the one that counts.
        if matches!(
            pending.get(&provider),
            Some(Pending::Device {
                exchanging: true,
                ..
            })
        ) {
            return;
        }
        match outcome {
            // Not yet, or the provider could not be asked this time: the next look tries again.
            Ok(DevicePoll::Pending) | Err(ProviderError::Unavailable(_)) => {}
            Ok(DevicePoll::SlowDown) => {
                if let Some(Pending::Device {
                    device, next_poll, ..
                }) = pending.get_mut(&provider)
                {
                    device.interval_secs += 5;
                    *next_poll = now() + device.interval_secs;
                }
            }
            Ok(DevicePoll::Failed(message)) | Err(ProviderError::Rejected(message)) => {
                pending.insert(provider, Pending::Failed(message));
            }
            Ok(DevicePoll::Done {
                authorization_code,
                code_verifier,
            }) => {
                if let Some(Pending::Device { exchanging, .. }) = pending.get_mut(&provider) {
                    *exchanging = true;
                }
                drop(pending);
                let exchanged = providers
                    .net
                    .openai_exchange(&authorization_code, &code_verifier, OPENAI_DEVICE_REDIRECT)
                    .map_err(sign_in_error);
                let mut pending = providers.pending();
                // Cancelled or restarted while the code was exchanged: this login is not wanted now.
                if !providers.wanted(&pending, provider, generation) {
                    return;
                }
                match exchanged.and_then(|tokens| self.store_login(provider, &tokens, None)) {
                    Ok(()) => {
                        pending.remove(&provider);
                    }
                    Err(error) => {
                        pending.insert(provider, Pending::Failed(error_text(&error)));
                    }
                }
            }
        }
    }

    /// Seals the tokens as the provider's login. A refresh that gives no new refresh token or
    /// id token keeps the old ones.
    fn store_login(
        &self,
        provider: ProviderId,
        tokens: &Tokens,
        old: Option<&Login>,
    ) -> Result<(), EngineError> {
        let id_token = tokens
            .id_token
            .clone()
            .or_else(|| old.map(|o| o.id_token.clone()))
            .unwrap_or_default();
        let (account, account_id) = if provider == ProviderId::Openai {
            let account_id = chatgpt_account_id(&tokens.access)
                .or_else(|| chatgpt_account_id(&id_token))
                .ok_or_else(|| {
                    EngineError::Unavailable(
                        "the ChatGPT token has no account id; sign in again".into(),
                    )
                })?;
            (email_of(&id_token).unwrap_or_default(), account_id)
        } else {
            (
                old.map(|o| o.account.clone()).unwrap_or_default(),
                String::new(),
            )
        };
        // The one place that decides: a login always has a refresh token, a new one or the old one.
        let Some(refresh) = tokens
            .refresh
            .clone()
            .filter(|r| !r.is_empty())
            .or_else(|| old.map(|o| o.refresh.clone()))
        else {
            return Err(EngineError::Unavailable(
                "the provider gave no refresh token, so the login could not be renewed".into(),
            ));
        };
        let login = Login {
            access: tokens.access.clone(),
            refresh,
            id_token,
            expires: now() + tokens.expires_in,
            account,
            account_id,
        };
        let json = serde_json::to_string(&login)
            .map_err(|_| EngineError::Unavailable("cannot keep the login".into()))?;
        let sealed = self.seal(&json)?;
        self.ports
            .settings
            .set(&provider.login_setting(), &sealed)?;
        Ok(())
    }

    fn stored_login(&self, provider: ProviderId) -> Result<Option<Login>, EngineError> {
        let Some(json) = self.saved_secret(&provider.login_setting())? else {
            return Ok(None);
        };
        serde_json::from_str(&json).map(Some).map_err(|_| {
            EngineError::Unavailable("a saved login cannot be read; sign in again".into())
        })
    }

    /// The saved login with an access token good for a while, renewed when it is not.
    fn fresh_login(
        &self,
        config: &AgentConfig,
        provider: ProviderId,
    ) -> Result<Option<Login>, EngineError> {
        let renew_within = config.run_limit_secs().saturating_add(RENEW_MARGIN);
        let Some(login) = self.stored_login(provider)? else {
            return Ok(None);
        };
        if login.expires > now().saturating_add(renew_within) {
            return Ok(Some(login));
        }
        let providers = self.provider_net()?;
        let _one_at_a_time = providers
            .renewing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Someone else may have renewed it while this waited.
        let Some(login) = self.stored_login(provider)? else {
            return Ok(None);
        };
        if login.expires > now().saturating_add(renew_within) {
            return Ok(Some(login));
        }
        let renewed = match provider {
            ProviderId::Openai => providers.net.openai_refresh(&login.refresh),
            ProviderId::Anthropic => providers.net.anthropic_refresh(&login.refresh),
            ProviderId::Google => return Ok(None),
        }
        .map_err(|e| match e {
            ProviderError::Rejected(message) => EngineError::Unavailable(format!(
                "the {} login expired ({message}); sign in again in Settings",
                provider.name()
            )),
            ProviderError::Unavailable(message) => EngineError::Unavailable(message),
        })?;
        self.store_login(provider, &renewed, Some(&login))?;
        self.stored_login(provider)
    }
}

fn admin_only(actor: &Actor) -> Result<(), EngineError> {
    if can(actor, Command::Admin, None) {
        Ok(())
    } else {
        Err(EngineError::Forbidden)
    }
}

fn run_login(provider: ProviderId, login: &Login) -> RunLogin {
    match provider {
        ProviderId::Openai => RunLogin::Openai {
            access_token: login.access.clone(),
            id_token: login.id_token.clone(),
            account_id: login.account_id.clone(),
        },
        _ => RunLogin::Anthropic {
            access_token: login.access.clone(),
        },
    }
}

fn clean_key(key: &str) -> Result<&str, EngineError> {
    let key = key.trim();
    if key.is_empty() || key.len() > 500 || key.chars().any(char::is_whitespace) {
        return Err(EngineError::Invalid(
            "an API key is one piece of text without spaces".into(),
        ));
    }
    Ok(key)
}

fn sign_in_error(error: ProviderError) -> EngineError {
    match error {
        ProviderError::Rejected(message) | ProviderError::Unavailable(message) => {
            EngineError::Unavailable(message)
        }
    }
}

fn error_text(error: &EngineError) -> String {
    match error {
        EngineError::Unavailable(message) | EngineError::Invalid(message) => message.clone(),
        other => format!("{other:?}"),
    }
}

fn random_verifier() -> Result<String, EngineError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| EngineError::Unavailable("the system has no randomness".into()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Percent-encodes everything but unreserved characters (URL queries and form bodies).
#[must_use]
pub fn percent_encode(text: &str) -> String {
    use std::fmt::Write as _;
    text.bytes().fold(String::new(), |mut out, b| {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
        out
    })
}

/// Claude's sign-in page. The PKCE verifier doubles as `state`, as the vendor's tools do.
fn anthropic_authorize_url(verifier: &str) -> String {
    let params = [
        ("code", "true"),
        ("client_id", ANTHROPIC_CLIENT_ID),
        ("response_type", "code"),
        ("redirect_uri", ANTHROPIC_REDIRECT),
        ("scope", ANTHROPIC_SCOPES),
        ("code_challenge", &pkce_challenge(verifier)),
        ("code_challenge_method", "S256"),
        ("state", verifier),
    ];
    let query: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{k}={}", percent_encode(v)))
        .collect();
    format!("{ANTHROPIC_AUTHORIZE}?{}", query.join("&"))
}

/// What the person pasted: `code#state`, an address with `?code=…&state=…`, a query string, or
/// the bare code.
pub(crate) fn parse_authorization_input(input: &str) -> (Option<String>, Option<String>) {
    let value = input.trim();
    if value.is_empty() {
        return (None, None);
    }
    let query = |text: &str| {
        let mut code = None;
        let mut state = None;
        for pair in text.split('&') {
            match pair.split_once('=') {
                Some(("code", v)) => code = Some(v.to_owned()),
                Some(("state", v)) => state = Some(v.to_owned()),
                _ => {}
            }
        }
        (code, state)
    };
    if value.contains("://") {
        let after = value.split_once('?').map_or("", |(_, q)| q);
        return query(after.split('#').next().unwrap_or(""));
    }
    if let Some((code, state)) = value.split_once('#') {
        return (Some(code.to_owned()), Some(state.to_owned()));
    }
    if value.contains("code=") {
        return query(value);
    }
    (Some(value.to_owned()), None)
}

/// The claims of a JWT (not verified: the token came from the provider's own endpoint).
fn jwt_claims(token: &str) -> Option<serde_json::Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The `ChatGPT` account a Codex token belongs to.
fn chatgpt_account_id(token: &str) -> Option<String> {
    jwt_claims(token)?
        .get("https://api.openai.com/auth")?
        .get("chatgpt_account_id")?
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

fn email_of(id_token: &str) -> Option<String> {
    let claims = jwt_claims(id_token)?;
    claims
        .get("email")
        .or_else(|| claims.get("https://api.openai.com/profile")?.get("email"))?
        .as_str()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Fakes, engine_with_agents};
    use charet_model::Role;

    fn jwt(claims: &serde_json::Value) -> String {
        let part = |v: &serde_json::Value| URL_SAFE_NO_PAD.encode(v.to_string());
        format!(
            "{}.{}.sig",
            part(&serde_json::json!({"alg": "none"})),
            part(claims)
        )
    }

    fn chatgpt_token(account: &str) -> String {
        jwt(&serde_json::json!({
            "https://api.openai.com/auth": { "chatgpt_account_id": account },
            "email": "dana@example.com",
        }))
    }

    /// Lets the next look at a waiting device sign-in ask the provider now.
    fn expire_poll_wait(engine: &Engine, provider: ProviderId) {
        let mut pending = engine.providers.as_ref().unwrap().pending();
        if let Some(Pending::Device { next_poll, .. }) = pending.get_mut(&provider) {
            *next_poll = 0;
        }
    }

    /// Leaves the saved login `left` seconds to live.
    fn age_login(engine: &Engine, provider: ProviderId, left: u64) {
        let mut login = engine.stored_login(provider).unwrap().unwrap();
        login.expires = now() + left;
        let sealed = engine
            .seal(&serde_json::to_string(&login).unwrap())
            .unwrap();
        engine
            .ports
            .settings
            .set(&provider.login_setting(), &sealed)
            .unwrap();
    }

    fn setup() -> (Engine, Fakes, Actor, Actor) {
        let (engine, fakes) = engine_with_agents();
        engine.ensure_owner().unwrap();
        let admin = engine.local_actor();
        let ann = engine.add_member(&admin, "Ann", Role::Editor).unwrap();
        let ann = engine.token_actor(&ann.token).unwrap().unwrap();
        (engine, fakes, admin, ann)
    }

    #[test]
    fn only_admins_see_and_change_accounts() {
        let (engine, _, admin, ann) = setup();
        assert_eq!(engine.providers(&ann), Err(EngineError::Forbidden));
        assert_eq!(
            engine.start_provider_sign_in(&ann, ProviderId::Openai),
            Err(EngineError::Forbidden)
        );
        let providers = engine.providers(&admin).unwrap();
        let claude = &providers[0];
        // The test engine has a Claude key in its environment.
        assert_eq!(
            (claude.id, claude.key.source, claude.using),
            (ProviderId::Anthropic, "env", "key")
        );
        assert_eq!(providers[1].using, "none");
        assert!(providers[2].oauth.is_none(), "Google has no sign-in");
    }

    #[test]
    fn a_key_is_saved_only_after_the_provider_accepts_it() {
        let (engine, fakes, admin, _) = setup();
        fakes.net.refuse_keys("bad");
        let refused = engine
            .check_provider_key(&admin, ProviderId::Openai, "bad-key-1234")
            .unwrap();
        assert_eq!(
            (refused.ok, refused.reason),
            (false, Some(KeyProblem::Invalid))
        );
        assert!(matches!(
            engine.save_provider_key(&admin, ProviderId::Openai, "bad-key-1234"),
            Err(EngineError::Invalid(_))
        ));
        assert_eq!(engine.providers(&admin).unwrap()[1].key.source, "none");

        let saved = engine
            .save_provider_key(&admin, ProviderId::Openai, "sk-good-wxyz")
            .unwrap();
        assert_eq!(
            (saved.key.source, saved.key.last4.as_deref(), saved.using),
            ("settings", Some("wxyz"), "key")
        );
        let stored = engine
            .ports
            .settings
            .get("agent.key.codex")
            .unwrap()
            .unwrap();
        assert!(!stored.contains("wxyz"), "stored sealed");
        // The agents' own setting is the same key.
        assert_eq!(
            engine
                .api_key(&AgentConfig::default(), AgentKind::Codex)
                .unwrap()
                .as_deref(),
            Some("sk-good-wxyz")
        );
        // A key from the environment cannot be replaced here.
        assert!(matches!(
            engine.save_provider_key(&admin, ProviderId::Anthropic, "sk-ant-new"),
            Err(EngineError::Invalid(_))
        ));

        engine
            .remove_provider_key(&admin, ProviderId::Openai)
            .unwrap();
        assert_eq!(engine.providers(&admin).unwrap()[1].key.source, "none");
    }

    #[test]
    fn a_chatgpt_device_sign_in_waits_for_the_person_then_keeps_the_login_sealed() {
        let (engine, fakes, admin, _) = setup();
        let waiting = engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        assert_eq!(waiting.state, SignInState::Waiting);
        assert_eq!(waiting.method, Some(SignInMethod::Device));
        assert_eq!(waiting.url.as_deref(), Some(OPENAI_DEVICE_URL));
        assert_eq!(waiting.user_code.as_deref(), Some("ABCD-1234"));

        // Not asked again before the provider's interval has passed.
        let again = engine.provider_sign_in(&admin, ProviderId::Openai).unwrap();
        assert_eq!(again.state, SignInState::Waiting);
        assert_eq!(fakes.net.polls(), 0);

        fakes.net.finish_device(&chatgpt_token("acct-1"));
        expire_poll_wait(&engine, ProviderId::Openai);
        let done = engine.provider_sign_in(&admin, ProviderId::Openai).unwrap();
        assert_eq!(done.state, SignInState::SignedIn);
        assert_eq!(done.account.as_deref(), Some("dana@example.com"));
        let stored = engine
            .ports
            .settings
            .get("provider.login.openai")
            .unwrap()
            .unwrap();
        assert!(
            !stored.contains("acct-1") && !stored.contains("refresh-1"),
            "sealed"
        );
        assert_eq!(engine.providers(&admin).unwrap()[1].using, "oauth");
    }

    #[test]
    fn a_failed_or_cancelled_sign_in_leaves_nobody_signed_in() {
        let (engine, fakes, admin, _) = setup();
        engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        fakes.net.fail_device("the code expired");
        expire_poll_wait(&engine, ProviderId::Openai);
        let failed = engine.provider_sign_in(&admin, ProviderId::Openai).unwrap();
        assert_eq!(
            (failed.state, failed.error.as_deref()),
            (SignInState::Failed, Some("the code expired"))
        );

        engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        engine
            .end_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        assert_eq!(
            engine
                .provider_sign_in(&admin, ProviderId::Openai)
                .unwrap()
                .state,
            SignInState::SignedOut
        );
    }

    #[test]
    fn cancelling_a_new_sign_in_keeps_the_account_that_is_signed_in() {
        let (engine, fakes, admin, _) = setup();
        engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        fakes.net.finish_device(&chatgpt_token("acct-1"));
        expire_poll_wait(&engine, ProviderId::Openai);
        engine.provider_sign_in(&admin, ProviderId::Openai).unwrap();

        // A second sign-in is started and cancelled: the account stays.
        engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        engine
            .end_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        let state = |engine: &Engine| {
            engine
                .provider_sign_in(&admin, ProviderId::Openai)
                .unwrap()
                .state
        };
        assert_eq!(state(&engine), SignInState::SignedIn);
        // With none under way, the same call signs out.
        engine
            .end_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        assert_eq!(state(&engine), SignInState::SignedOut);
    }

    #[test]
    fn a_claude_sign_in_takes_the_pasted_code() {
        let (engine, fakes, admin, _) = setup();
        assert!(
            matches!(
                engine.finish_provider_sign_in(&admin, ProviderId::Anthropic, "abc#state"),
                Err(EngineError::Invalid(_))
            ),
            "nothing started yet"
        );
        let waiting = engine
            .start_provider_sign_in(&admin, ProviderId::Anthropic)
            .unwrap();
        assert_eq!(waiting.method, Some(SignInMethod::Code));
        let url = waiting.url.unwrap();
        assert!(url.starts_with("https://claude.ai/oauth/authorize?code=true&client_id=9d1c250a"));
        assert!(
            url.contains("code_challenge_method=S256")
                && url.contains(
                    "redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback"
                )
        );
        let verifier = url.split("state=").nth(1).unwrap().to_owned();

        assert!(
            matches!(
                engine.finish_provider_sign_in(
                    &admin,
                    ProviderId::Anthropic,
                    "abc#someone-elses-state"
                ),
                Err(EngineError::Invalid(_))
            ),
            "another sign-in's code"
        );
        let done = engine
            .finish_provider_sign_in(
                &admin,
                ProviderId::Anthropic,
                &format!("the-code#{verifier}"),
            )
            .unwrap();
        assert_eq!(done.state, SignInState::SignedIn);
        assert_eq!(
            fakes.net.anthropic_exchanges(),
            vec![("the-code".to_owned(), verifier)]
        );
    }

    #[test]
    fn a_run_gets_the_login_and_an_expiring_one_is_renewed_first() {
        let (engine, fakes, admin, _) = setup();
        engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        fakes.net.finish_device(&chatgpt_token("acct-1"));
        expire_poll_wait(&engine, ProviderId::Openai);
        engine.provider_sign_in(&admin, ProviderId::Openai).unwrap();

        let config = engine.agent_config();
        let credential = engine
            .run_credential(&config, AgentKind::Codex)
            .unwrap()
            .unwrap();
        assert_eq!(credential.api_key, "");
        assert!(
            matches!(&credential.login, Some(RunLogin::Openai { account_id, .. }) if account_id == "acct-1")
        );
        assert_eq!(fakes.net.refreshes(), 0, "a fresh token is used as it is");

        // Past the point where it should be renewed: the refresh token is used, the new tokens kept.
        age_login(&engine, ProviderId::Openai, 10);
        fakes.net.next_refresh(&chatgpt_token("acct-1"));
        let renewed = engine
            .run_credential(&config, AgentKind::Codex)
            .unwrap()
            .unwrap();
        assert_eq!(fakes.net.refreshes(), 1);
        assert!(matches!(renewed.login, Some(RunLogin::Openai { .. })));

        // A refused refresh stops the run with a message that says what to do.
        age_login(&engine, ProviderId::Openai, 10);
        fakes.net.refuse_refresh("invalid_grant");
        let error = engine
            .run_credential(&config, AgentKind::Codex)
            .unwrap_err();
        assert!(
            matches!(&error, EngineError::Unavailable(m) if m.contains("sign in again")),
            "{error:?}"
        );
    }

    #[test]
    fn a_renewal_that_gives_no_new_refresh_token_keeps_the_old_one() {
        let (engine, fakes, admin, _) = setup();
        engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        fakes.net.finish_device(&chatgpt_token("acct-1"));
        expire_poll_wait(&engine, ProviderId::Openai);
        engine.provider_sign_in(&admin, ProviderId::Openai).unwrap();

        let config = engine.agent_config();
        age_login(&engine, ProviderId::Openai, 10);
        fakes.net.keep_refresh_token();
        engine
            .run_credential(&config, AgentKind::Codex)
            .unwrap()
            .unwrap();
        // The login still renews with the refresh token it was given at sign-in.
        age_login(&engine, ProviderId::Openai, 10);
        engine
            .run_credential(&config, AgentKind::Codex)
            .unwrap()
            .unwrap();
        assert_eq!(fakes.net.refreshed_with(), ["refresh-1", "refresh-1"]);
    }

    #[test]
    fn a_login_the_provider_gives_no_refresh_token_for_is_not_kept() {
        let (engine, _, _, _) = setup();
        let tokens = Tokens {
            access: chatgpt_token("acct-1"),
            refresh: None,
            id_token: None,
            expires_in: 3600,
        };
        assert!(matches!(
            engine.store_login(ProviderId::Openai, &tokens, None),
            Err(EngineError::Unavailable(_))
        ));
        assert_eq!(
            engine.stored_login(ProviderId::Openai).unwrap().map(|_| ()),
            None
        );
    }

    #[test]
    fn a_sign_in_cancelled_while_its_code_is_exchanged_does_not_sign_in() {
        let (engine, fakes, admin, _) = setup();
        // ChatGPT: the device code is exchanged after the person finished.
        engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        fakes.net.finish_device(&chatgpt_token("acct-1"));
        expire_poll_wait(&engine, ProviderId::Openai);
        let (cancel, who) = (engine.clone(), admin.clone());
        fakes.net.during_exchange(move || {
            cancel
                .end_provider_sign_in(&who, ProviderId::Openai)
                .unwrap();
        });
        let state = engine
            .provider_sign_in(&admin, ProviderId::Openai)
            .unwrap()
            .state;
        assert_eq!(state, SignInState::SignedOut, "{state:?}");
        assert_eq!(
            engine.stored_login(ProviderId::Openai).unwrap().map(|_| ()),
            None
        );

        // A sign-in restarted meanwhile keeps its own state.
        engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        expire_poll_wait(&engine, ProviderId::Openai);
        let (restart, who) = (engine.clone(), admin.clone());
        fakes.net.during_exchange(move || {
            restart
                .start_provider_sign_in(&who, ProviderId::Openai)
                .unwrap();
        });
        let state = engine.provider_sign_in(&admin, ProviderId::Openai).unwrap();
        assert_eq!(
            state.state,
            SignInState::Waiting,
            "the newer sign-in is still waiting"
        );
        assert_eq!(
            engine.stored_login(ProviderId::Openai).unwrap().map(|_| ()),
            None
        );

        // Claude: the pasted code is exchanged while the sign-in is cancelled.
        let waiting = engine
            .start_provider_sign_in(&admin, ProviderId::Anthropic)
            .unwrap();
        let verifier = waiting
            .url
            .unwrap()
            .split("state=")
            .nth(1)
            .unwrap()
            .to_owned();
        let (cancel, who) = (engine.clone(), admin.clone());
        fakes.net.during_exchange(move || {
            cancel
                .end_provider_sign_in(&who, ProviderId::Anthropic)
                .unwrap();
        });
        let result =
            engine.finish_provider_sign_in(&admin, ProviderId::Anthropic, &format!("c#{verifier}"));
        assert!(matches!(result, Err(EngineError::Invalid(_))), "{result:?}");
        assert_eq!(
            engine
                .stored_login(ProviderId::Anthropic)
                .unwrap()
                .map(|_| ()),
            None
        );
    }

    #[test]
    fn a_login_is_renewed_to_outlast_the_longest_run() {
        let short = AgentConfig::default();
        assert_eq!(
            short.run_limit_secs(),
            30 * 60,
            "charet-agent's own default"
        );
        let long = AgentConfig {
            run_minutes: 90,
            ..AgentConfig::default()
        };
        let tiny = AgentConfig {
            run_minutes: 1,
            ..AgentConfig::default()
        };
        assert_eq!(tiny.run_limit_secs(), 60, "a shorter limit is honored too");

        let (engine, fakes, admin, _) = setup();
        engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        fakes.net.finish_device(&chatgpt_token("acct-1"));
        expire_poll_wait(&engine, ProviderId::Openai);
        engine.provider_sign_in(&admin, ProviderId::Openai).unwrap();
        let credential = |config: &AgentConfig| {
            engine
                .run_credential(config, AgentKind::Codex)
                .unwrap()
                .unwrap();
            fakes.net.refreshes()
        };

        // 40 minutes left: enough for 30-minute runs, not for 90-minute ones.
        age_login(&engine, ProviderId::Openai, 40 * 60);
        assert_eq!(credential(&short), 0);
        assert_eq!(credential(&long), 1);
        // 20 minutes left: enough for 1-minute runs, not for 30-minute ones.
        age_login(&engine, ProviderId::Openai, 20 * 60);
        assert_eq!(credential(&tiny), 1, "no refresh: it outlasts the run");
        assert_eq!(credential(&short), 2);

        // A limit too large to count (the agent service accepts any number) cannot overflow.
        let huge = AgentConfig {
            run_minutes: u64::MAX,
            ..AgentConfig::default()
        };
        assert_eq!(huge.run_limit_secs(), u64::MAX);
        assert_eq!(credential(&huge), 3, "it only means: always renew");
    }

    #[test]
    fn an_attempt_that_finishes_after_a_newer_start_or_cancel_was_asked_for_is_not_accepted() {
        let (engine, fakes, admin, _) = setup();
        let reserve = |engine: &Engine, provider| {
            // A restart or cancel that has asked for its place and waits for the pending state.
            engine.providers.as_ref().unwrap().reserve(provider);
        };

        engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        fakes.net.finish_device(&chatgpt_token("acct-1"));
        expire_poll_wait(&engine, ProviderId::Openai);
        let asked = engine.clone();
        fakes
            .net
            .during_exchange(move || reserve(&asked, ProviderId::Openai));
        engine.provider_sign_in(&admin, ProviderId::Openai).unwrap();
        assert_eq!(
            engine.stored_login(ProviderId::Openai).unwrap().map(|_| ()),
            None
        );

        let waiting = engine
            .start_provider_sign_in(&admin, ProviderId::Anthropic)
            .unwrap();
        let verifier = waiting
            .url
            .unwrap()
            .split("state=")
            .nth(1)
            .unwrap()
            .to_owned();
        let asked = engine.clone();
        fakes
            .net
            .during_exchange(move || reserve(&asked, ProviderId::Anthropic));
        let result =
            engine.finish_provider_sign_in(&admin, ProviderId::Anthropic, &format!("c#{verifier}"));
        assert!(matches!(result, Err(EngineError::Invalid(_))), "{result:?}");
        assert_eq!(
            engine
                .stored_login(ProviderId::Anthropic)
                .unwrap()
                .map(|_| ()),
            None
        );
    }

    #[test]
    fn a_start_that_answers_after_a_newer_start_or_a_cancel_does_not_overwrite_them() {
        let (engine, fakes, admin, _) = setup();

        // Two admins start a ChatGPT sign-in at once; the first one's answer comes back last.
        let second = engine.clone();
        let who = admin.clone();
        fakes.net.during_device_start(move || {
            let newer = second
                .start_provider_sign_in(&who, ProviderId::Openai)
                .unwrap();
            assert_eq!(newer.user_code.as_deref(), Some("ABCD-1235"));
        });
        let late = engine.start_provider_sign_in(&admin, ProviderId::Openai);
        assert!(matches!(late, Err(EngineError::Invalid(_))), "{late:?}");
        let shown = engine.provider_sign_in(&admin, ProviderId::Openai).unwrap();
        assert_eq!(
            (shown.state, shown.user_code.as_deref()),
            (SignInState::Waiting, Some("ABCD-1235")),
            "the newer start stays"
        );

        // A cancel comes in while the provider is asked for a code: nothing is brought back.
        engine
            .end_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        let cancel = engine.clone();
        let who = admin.clone();
        fakes.net.during_device_start(move || {
            cancel
                .end_provider_sign_in(&who, ProviderId::Openai)
                .unwrap();
        });
        let late = engine.start_provider_sign_in(&admin, ProviderId::Openai);
        assert!(matches!(late, Err(EngineError::Invalid(_))), "{late:?}");
        let shown = engine.provider_sign_in(&admin, ProviderId::Openai).unwrap();
        assert_eq!(
            shown.state,
            SignInState::SignedOut,
            "the cancelled start is not recreated"
        );
    }

    #[test]
    fn restarts_that_fail_in_any_order_leave_the_waiting_attempt_wanted() {
        let (engine, fakes, admin, _) = setup();
        engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        let providers = engine.providers.as_ref().unwrap();
        // Two restarts are in flight; the one asked for first fails first.
        let first = providers.reserve(ProviderId::Openai);
        let second = providers.reserve(ProviderId::Openai);
        providers.keep_waiting_attempt(ProviderId::Openai, first);
        providers.keep_waiting_attempt(ProviderId::Openai, second);

        fakes.net.finish_device(&chatgpt_token("acct-1"));
        expire_poll_wait(&engine, ProviderId::Openai);
        let state = engine
            .provider_sign_in(&admin, ProviderId::Openai)
            .unwrap()
            .state;
        assert_eq!(
            state,
            SignInState::SignedIn,
            "the attempt that was waiting still counts"
        );
    }

    #[test]
    fn a_second_look_while_the_code_is_exchanged_does_not_spoil_the_sign_in() {
        let (engine, fakes, admin, _) = setup();
        engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        fakes.net.finish_device(&chatgpt_token("acct-1"));
        expire_poll_wait(&engine, ProviderId::Openai);
        // The exchange is slow; another page asks meanwhile, and the code is used up by now.
        let other_page = engine.clone();
        let who = admin.clone();
        let net = fakes.net.clone();
        fakes.net.during_exchange(move || {
            net.fail_device("the code was already used");
            expire_poll_wait(&other_page, ProviderId::Openai);
            let seen = other_page
                .provider_sign_in(&who, ProviderId::Openai)
                .unwrap();
            assert_eq!(seen.state, SignInState::Waiting);
        });
        let state = engine
            .provider_sign_in(&admin, ProviderId::Openai)
            .unwrap()
            .state;
        assert_eq!(state, SignInState::SignedIn);
        assert_eq!(
            fakes.net.polls(),
            1,
            "nobody asked the provider again while exchanging"
        );
    }

    #[test]
    fn signing_out_waits_for_a_renewal_in_flight() {
        let (engine, fakes, admin, _) = setup();
        engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        fakes.net.finish_device(&chatgpt_token("acct-1"));
        expire_poll_wait(&engine, ProviderId::Openai);
        engine.provider_sign_in(&admin, ProviderId::Openai).unwrap();
        age_login(&engine, ProviderId::Openai, 10);

        // While the renewal is in flight an admin signs out (on another thread, as a request would).
        let signing_out = std::sync::Arc::new(Mutex::new(None));
        let (out, who, handle) = (engine.clone(), admin.clone(), signing_out.clone());
        fakes.net.during_refresh(move || {
            let (out, who) = (out.clone(), who.clone());
            *handle.lock().unwrap() = Some(std::thread::spawn(move || {
                out.end_provider_sign_in(&who, ProviderId::Openai).unwrap();
            }));
            std::thread::sleep(std::time::Duration::from_millis(150));
        });
        let _ = engine.run_credential(&engine.agent_config(), AgentKind::Codex);
        signing_out.lock().unwrap().take().unwrap().join().unwrap();

        // The renewal's tokens were not stored after the sign-out wiped the login.
        assert_eq!(
            engine.stored_login(ProviderId::Openai).unwrap().map(|_| ()),
            None
        );
        let state = engine
            .provider_sign_in(&admin, ProviderId::Openai)
            .unwrap()
            .state;
        assert_eq!(state, SignInState::SignedOut);
    }

    #[test]
    fn a_restart_that_fails_leaves_the_waiting_attempt_alone() {
        let (engine, fakes, admin, _) = setup();
        engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        fakes.net.refuse_device_start("no network");
        let restarted = engine.start_provider_sign_in(&admin, ProviderId::Openai);
        assert!(
            matches!(restarted, Err(EngineError::Unavailable(_))),
            "{restarted:?}"
        );

        // The person finishes the first attempt after all: it still counts.
        fakes.net.finish_device(&chatgpt_token("acct-1"));
        expire_poll_wait(&engine, ProviderId::Openai);
        let state = engine
            .provider_sign_in(&admin, ProviderId::Openai)
            .unwrap()
            .state;
        assert_eq!(state, SignInState::SignedIn);
    }

    #[test]
    fn the_choice_between_key_and_login_is_kept() {
        let (engine, fakes, admin, _) = setup();
        engine
            .save_provider_key(&admin, ProviderId::Openai, "sk-good-wxyz")
            .unwrap();
        engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        fakes.net.finish_device(&chatgpt_token("acct-1"));
        expire_poll_wait(&engine, ProviderId::Openai);
        engine.provider_sign_in(&admin, ProviderId::Openai).unwrap();
        // Both are there: the key until the admins choose the login.
        assert_eq!(engine.providers(&admin).unwrap()[1].using, "key");
        assert_eq!(
            engine
                .choose_provider_auth(&admin, ProviderId::Openai, "oauth")
                .unwrap()
                .using,
            "oauth"
        );
        assert!(
            matches!(
                engine.choose_provider_auth(&admin, ProviderId::Anthropic, "oauth"),
                Err(EngineError::Invalid(_))
            ),
            "nobody is signed in to Claude"
        );
    }

    #[test]
    fn pasted_codes_come_in_several_shapes() {
        let some = |c: &str, s: Option<&str>| (Some(c.to_owned()), s.map(str::to_owned));
        assert_eq!(
            parse_authorization_input(" abc#xyz "),
            some("abc", Some("xyz"))
        );
        assert_eq!(parse_authorization_input("abc"), some("abc", None));
        assert_eq!(
            parse_authorization_input("code=abc&state=xyz"),
            some("abc", Some("xyz"))
        );
        assert_eq!(
            parse_authorization_input("http://localhost:1455/auth/callback?code=abc&state=xyz"),
            some("abc", Some("xyz"))
        );
        assert_eq!(parse_authorization_input("  "), (None, None));
    }

    #[test]
    fn pkce_challenge_is_the_vendors_s256() {
        // The example of RFC 7636 appendix B.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
