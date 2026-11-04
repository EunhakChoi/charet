//! The accounts the web agent runs on: per provider an API key (the provider checks it before it is
//! saved) or a subscription login (Claude, `ChatGPT`) done in a new browser window.
//!
//! - Secrets are stored encrypted with the server key, like the agents' API keys always were. Only
//!   the last 4 characters of a key are ever shown.
//! - A login is the provider's own OAuth flow, driven by the server (the same flows the vendors'
//!   command line tools use, so no redirect has to reach this server): the person signs in at the
//!   provider's page in a new window and pastes back what it ends with (Claude: a code the page
//!   shows; `ChatGPT`: the address the browser was sent to, which nothing answers on purpose).
//! - The refresh token never leaves the server. Before each run the server renews the access token
//!   when it is about to expire and hands the container only that.
//! - Network calls go through [`ProviderNet`]; this module decides, stores and refreshes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

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

const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENAI_AUTHORIZE: &str = "https://auth.openai.com/oauth/authorize";
/// Where `ChatGPT`'s sign-in page sends the browser (one of the two addresses registered for the
/// client). Nothing listens there: the person copies the address the browser ends on.
pub const OPENAI_REDIRECT: &str = "http://localhost:1455/auth/callback";
/// What the provider's own tool asks for: sign-in, a refresh token, and the connector scopes a Codex
/// run's access token needs for connector-backed tools.
const OPENAI_SCOPES: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
/// `ChatGPT` may append this onboarding note to the otherwise exact `state` of the address.
const OPENAI_STATE_SUFFIX: &str = ".onboarding_entrypoint=life_sciences";
/// Who is asking, as the sign-in page is told.
const OPENAI_ORIGINATOR: &str = "charet";

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthStatus {
    pub state: SignInState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl OAuthStatus {
    fn only(state: SignInState) -> Self {
        Self {
            state,
            url: None,
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

/// A sign-in the person has not finished.
struct Attempt {
    /// The PKCE verifier, given to the provider when the code is exchanged.
    verifier: String,
    /// What the provider hands back with the code, to tell this sign-in from another.
    state: String,
    /// The provider's page to sign in at.
    url: String,
    started: u64,
}

impl Attempt {
    /// A new attempt for `provider` (which has a sign-in).
    fn new(provider: ProviderId) -> Result<Self, EngineError> {
        let verifier = random_verifier()?;
        let (state, url) = match provider {
            // Claude's page takes the verifier as `state`, as the vendor's tools do.
            ProviderId::Anthropic => (verifier.clone(), anthropic_authorize_url(&verifier)),
            ProviderId::Openai => {
                let state = random_verifier()?;
                let url = openai_authorize_url(&verifier, &state);
                (state, url)
            }
            ProviderId::Google => {
                return Err(EngineError::Invalid("this provider has no sign-in".into()));
            }
        };
        Ok(Self {
            verifier,
            state,
            url,
            started: now(),
        })
    }

    /// Started more than [`LOGIN_TIMEOUT`] ago and not finished.
    fn timed_out(&self, at: u64) -> bool {
        at > self.started + LOGIN_TIMEOUT
    }
}

/// One provider's sign-in and saved login. Whatever changes either (a start, a code exchange, a
/// renewal, a cancel, a sign-out) happens with [`Account::hold`] taken, network calls included: two
/// of them never overlap, so none acts on what another has just replaced. Looking at the state
/// never takes it, so it never waits for a call in flight.
#[derive(Default)]
struct Account {
    busy: Mutex<()>,
    /// The sign-in under way. Written only while `busy` is held; anyone may read it.
    pending: Mutex<Option<Attempt>>,
}

impl Account {
    fn hold(&self) -> MutexGuard<'_, ()> {
        self.busy.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn pending(&self) -> MutexGuard<'_, Option<Attempt>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The network port and the accounts.
#[derive(Clone)]
pub(crate) struct Providers {
    net: Arc<dyn ProviderNet>,
    accounts: Arc<HashMap<ProviderId, Account>>,
}

impl Providers {
    pub(crate) fn new(net: Arc<dyn ProviderNet>) -> Self {
        Self {
            net,
            accounts: Arc::new(
                ProviderId::ALL
                    .into_iter()
                    .map(|provider| (provider, Account::default()))
                    .collect(),
            ),
        }
    }

    fn account(&self, provider: ProviderId) -> &Account {
        &self.accounts[&provider]
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

    /// Starts a sign-in: the provider's page to open in a new window. One at a time per provider:
    /// while one waits it has to be cancelled first. An account that is signed in stays so until the
    /// new sign-in succeeds (which then replaces it). Admins only.
    pub fn start_provider_sign_in(
        &self,
        actor: &Actor,
        provider: ProviderId,
    ) -> Result<OAuthStatus, EngineError> {
        admin_only(actor)?;
        let providers = self.provider_net()?;
        let account = providers.account(provider);
        let _busy = account.hold();
        if account
            .pending()
            .as_ref()
            .is_some_and(|attempt| !attempt.timed_out(now()))
        {
            return Err(EngineError::Exists("a sign-in for this provider"));
        }
        *account.pending() = Some(Attempt::new(provider)?);
        self.sign_in_view(provider)
    }

    /// Where a sign-in stands. Admins only.
    pub fn provider_sign_in(
        &self,
        actor: &Actor,
        provider: ProviderId,
    ) -> Result<OAuthStatus, EngineError> {
        admin_only(actor)?;
        self.sign_in_view(provider)
    }

    /// Finishes the sign-in with what the person pasted: the code the provider's page showed
    /// (`code#state`), or the address the browser ended on. Admins only.
    pub fn finish_provider_sign_in(
        &self,
        actor: &Actor,
        provider: ProviderId,
        pasted: &str,
    ) -> Result<OAuthStatus, EngineError> {
        admin_only(actor)?;
        let providers = self.provider_net()?;
        let account = providers.account(provider);
        let _busy = account.hold();
        let (verifier, expected) = match account.pending().as_ref() {
            Some(attempt) if !attempt.timed_out(now()) => {
                (attempt.verifier.clone(), attempt.state.clone())
            }
            _ => {
                return Err(EngineError::Invalid(
                    "no sign-in is waiting for a code; start one first".into(),
                ));
            }
        };
        let (code, state) = parse_authorization_input(pasted);
        let Some(code) = code else {
            return Err(EngineError::Invalid(
                "paste the code or the address the sign-in ends with".into(),
            ));
        };
        if state
            .as_deref()
            .is_some_and(|s| !same_sign_in(provider, s, &expected))
        {
            return Err(EngineError::Invalid(
                "that code belongs to another sign-in; start again".into(),
            ));
        }
        let tokens = match provider {
            ProviderId::Openai => providers
                .net
                .openai_exchange(&code, &verifier, OPENAI_REDIRECT),
            ProviderId::Anthropic => providers.net.anthropic_exchange(
                &code,
                state.as_deref().unwrap_or(&expected),
                &verifier,
                ANTHROPIC_REDIRECT,
            ),
            ProviderId::Google => {
                return Err(EngineError::Invalid("this provider has no sign-in".into()));
            }
        }
        .map_err(|e| match e {
            ProviderError::Rejected(message) => EngineError::Invalid(format!(
                "the provider did not accept the code ({message}); start again"
            )),
            ProviderError::Unavailable(message) => EngineError::Unavailable(message),
        })?;
        self.store_login(provider, &tokens, None)?;
        *account.pending() = None;
        self.sign_in_view(provider)
    }

    /// Cancels the sign-in that waits (or dismisses one that failed). Nothing else changes: an
    /// account that is signed in stays so, and a sign-in that has already finished is not undone.
    /// Admins only.
    pub fn cancel_provider_sign_in(
        &self,
        actor: &Actor,
        provider: ProviderId,
    ) -> Result<(), EngineError> {
        admin_only(actor)?;
        if let Some(providers) = &self.providers {
            let account = providers.account(provider);
            let _busy = account.hold();
            *account.pending() = None;
        }
        Ok(())
    }

    /// Signs the account out: the saved login is forgotten here (the provider is not told). A
    /// sign-in that waits is left as it is. Admins only.
    pub fn sign_out_provider(
        &self,
        actor: &Actor,
        provider: ProviderId,
    ) -> Result<(), EngineError> {
        admin_only(actor)?;
        // A renewal in flight would store its tokens after this and bring the login back.
        let _busy = self
            .providers
            .as_ref()
            .map(|providers| providers.account(provider).hold());
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

    /// Where the sign-in stands. A saved login shows through a new sign-in: `account` is kept, and
    /// a sign-in that was given up is an `error` on a state that stays `SignedIn` (the account is
    /// still in use).
    fn sign_in_view(&self, provider: ProviderId) -> Result<OAuthStatus, EngineError> {
        let login = self.stored_login(provider)?;
        let attempt = self.providers.as_ref().and_then(|providers| {
            let pending = providers.account(provider).pending();
            let attempt = pending.as_ref()?;
            Some(if attempt.timed_out(now()) {
                OAuthStatus {
                    error: Some("the sign-in took too long; start again".into()),
                    ..OAuthStatus::only(SignInState::Failed)
                }
            } else {
                OAuthStatus {
                    url: Some(attempt.url.clone()),
                    ..OAuthStatus::only(SignInState::Waiting)
                }
            })
        });
        let Some(login) = login else {
            return Ok(attempt.unwrap_or_else(|| OAuthStatus::only(SignInState::SignedOut)));
        };
        let account = (!login.account.is_empty()).then_some(login.account);
        Ok(match attempt {
            Some(waiting) if waiting.state == SignInState::Waiting => {
                OAuthStatus { account, ..waiting }
            }
            failed => OAuthStatus {
                account,
                error: failed.and_then(|f| f.error),
                ..OAuthStatus::only(SignInState::SignedIn)
            },
        })
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
        let _busy = providers.account(provider).hold();
        // Someone else may have renewed it, or signed in again, while this waited.
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

/// `base?a=1&b=2`, percent-encoded.
fn with_query(base: &str, params: &[(&str, &str)]) -> String {
    let query: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{k}={}", percent_encode(v)))
        .collect();
    format!("{base}?{}", query.join("&"))
}

/// Claude's sign-in page. The PKCE verifier doubles as `state`, as the vendor's tools do.
fn anthropic_authorize_url(verifier: &str) -> String {
    with_query(
        ANTHROPIC_AUTHORIZE,
        &[
            ("code", "true"),
            ("client_id", ANTHROPIC_CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", ANTHROPIC_REDIRECT),
            ("scope", ANTHROPIC_SCOPES),
            ("code_challenge", &pkce_challenge(verifier)),
            ("code_challenge_method", "S256"),
            ("state", verifier),
        ],
    )
}

/// `ChatGPT`'s sign-in page. The browser ends on [`OPENAI_REDIRECT`] with `?code=…&state=…`.
fn openai_authorize_url(verifier: &str, state: &str) -> String {
    with_query(
        OPENAI_AUTHORIZE,
        &[
            ("response_type", "code"),
            ("client_id", OPENAI_CLIENT_ID),
            ("redirect_uri", OPENAI_REDIRECT),
            ("scope", OPENAI_SCOPES),
            ("code_challenge", &pkce_challenge(verifier)),
            ("code_challenge_method", "S256"),
            ("state", state),
            ("id_token_add_organizations", "true"),
            ("codex_cli_simplified_flow", "true"),
            ("originator", OPENAI_ORIGINATOR),
        ],
    )
}

/// Whether the `state` that came back is the one this sign-in was given. `ChatGPT` may add a note
/// to its own (exactly [`OPENAI_STATE_SUFFIX`]); every other difference is another sign-in.
fn same_sign_in(provider: ProviderId, got: &str, expected: &str) -> bool {
    got == expected
        || (provider == ProviderId::Openai
            && got.strip_suffix(OPENAI_STATE_SUFFIX) == Some(expected))
}

/// `%XX` escapes undone (what a browser's address bar shows for a value the provider encoded).
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = (bytes[i] == b'%')
            .then(|| bytes.get(i + 1..i + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
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
                Some(("code", v)) => code = Some(percent_decode(v)),
                Some(("state", v)) => state = Some(percent_decode(v)),
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

    /// The value of `name` in a sign-in page's address.
    fn param_of(url: &str, name: &str) -> String {
        url.split_once('?')
            .unwrap()
            .1
            .split('&')
            .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
            .unwrap()
            .to_owned()
    }

    /// The address `ChatGPT`'s page ends the browser on after signing in.
    fn chatgpt_callback(url: &str, code: &str) -> String {
        format!(
            "{OPENAI_REDIRECT}?code={code}&state={}",
            param_of(url, "state")
        )
    }

    /// Makes the waiting sign-in as old as one nobody finished for too long.
    fn age_attempt(engine: &Engine, provider: ProviderId) {
        let account = engine.providers.as_ref().unwrap().account(provider);
        if let Some(attempt) = account.pending().as_mut() {
            attempt.started = now() - LOGIN_TIMEOUT - 1;
        }
    }

    /// Signs in to `ChatGPT` as `account`: the page's address is pasted back.
    fn sign_in_chatgpt(engine: &Engine, fakes: &Fakes, admin: &Actor, account: &str) {
        fakes.net.issue(&chatgpt_token(account));
        let url = engine
            .start_provider_sign_in(admin, ProviderId::Openai)
            .unwrap()
            .url
            .unwrap();
        let done = engine
            .finish_provider_sign_in(admin, ProviderId::Openai, &chatgpt_callback(&url, "code"))
            .unwrap();
        assert_eq!(done.state, SignInState::SignedIn);
    }

    /// Signs in to Claude with a pasted code; `Fakes` hand out the same login each time.
    fn sign_in_claude(engine: &Engine, admin: &Actor) {
        let url = engine
            .start_provider_sign_in(admin, ProviderId::Anthropic)
            .unwrap()
            .url
            .unwrap();
        engine
            .finish_provider_sign_in(
                admin,
                ProviderId::Anthropic,
                &format!("code#{}", param_of(&url, "state")),
            )
            .unwrap();
    }

    fn state_of(engine: &Engine, admin: &Actor, provider: ProviderId) -> SignInState {
        engine.provider_sign_in(admin, provider).unwrap().state
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
    fn a_chatgpt_sign_in_takes_the_pasted_address_and_keeps_the_login_sealed() {
        let (engine, fakes, admin, _) = setup();
        fakes.net.issue(&chatgpt_token("acct-1"));
        let waiting = engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        assert_eq!(waiting.state, SignInState::Waiting);
        let url = waiting.url.unwrap();
        assert!(url.starts_with(
            "https://auth.openai.com/oauth/authorize?response_type=code&client_id=app_EMoamEEZ73f0CkXaXp7hrann"
        ));
        assert!(
            url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback")
                && url.contains("code_challenge_method=S256")
                && url.contains("codex_cli_simplified_flow=true"),
            "{url}"
        );
        assert!(
            url.contains("offline_access%20api.connectors.read%20api.connectors.invoke"),
            "the scopes the provider's own tool asks for: {url}"
        );

        // The address of another sign-in is refused, and the sign-in stays.
        let other = format!("{OPENAI_REDIRECT}?code=c&state=someone-elses");
        assert!(matches!(
            engine.finish_provider_sign_in(&admin, ProviderId::Openai, &other),
            Err(EngineError::Invalid(_))
        ));
        // So is a code the provider refuses.
        let refused = engine.finish_provider_sign_in(
            &admin,
            ProviderId::Openai,
            &chatgpt_callback(&url, "bad"),
        );
        assert!(
            matches!(&refused, Err(EngineError::Invalid(m)) if m.contains("invalid_grant")),
            "{refused:?}"
        );
        assert_eq!(
            state_of(&engine, &admin, ProviderId::Openai),
            SignInState::Waiting
        );

        // The address the browser ended on signs in, with the verifier of this sign-in.
        let done = engine
            .finish_provider_sign_in(
                &admin,
                ProviderId::Openai,
                &chatgpt_callback(&url, "good-code"),
            )
            .unwrap();
        assert_eq!(done.state, SignInState::SignedIn);
        assert_eq!(done.account.as_deref(), Some("dana@example.com"));
        let (code, verifier) = fakes.net.openai_exchanges().pop().unwrap();
        assert_eq!(code, "good-code");
        assert_eq!(pkce_challenge(&verifier), param_of(&url, "code_challenge"));
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
    fn a_cancelled_sign_in_leaves_nobody_signed_in_and_cannot_be_finished() {
        let (engine, fakes, admin, _) = setup();
        fakes.net.issue(&chatgpt_token("acct-1"));
        let url = engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap()
            .url
            .unwrap();
        engine
            .cancel_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        assert_eq!(
            state_of(&engine, &admin, ProviderId::Openai),
            SignInState::SignedOut
        );
        assert!(matches!(
            engine.finish_provider_sign_in(
                &admin,
                ProviderId::Openai,
                &chatgpt_callback(&url, "c")
            ),
            Err(EngineError::Invalid(_))
        ));
        assert_eq!(
            engine.stored_login(ProviderId::Openai).unwrap().map(|_| ()),
            None
        );
    }

    #[test]
    fn cancelling_only_cancels_and_signing_out_is_its_own_action() {
        let (engine, fakes, admin, _) = setup();
        sign_in_chatgpt(&engine, &fakes, &admin, "acct-1");

        // Cancelling with nothing waiting changes nothing.
        engine
            .cancel_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        assert_eq!(
            state_of(&engine, &admin, ProviderId::Openai),
            SignInState::SignedIn
        );

        // A second sign-in is started and cancelled: the account stays.
        engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        engine
            .cancel_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        assert_eq!(
            state_of(&engine, &admin, ProviderId::Openai),
            SignInState::SignedIn
        );

        // Signing out is what forgets it, and it leaves a waiting sign-in alone.
        engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        engine
            .sign_out_provider(&admin, ProviderId::Openai)
            .unwrap();
        assert_eq!(
            engine.stored_login(ProviderId::Openai).unwrap().map(|_| ()),
            None
        );
        assert_eq!(
            state_of(&engine, &admin, ProviderId::Openai),
            SignInState::Waiting
        );
        let none = engine.providers(&admin).unwrap();
        assert_eq!(none[1].using, "none");
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
        sign_in_chatgpt(&engine, &fakes, &admin, "acct-1");

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
        sign_in_chatgpt(&engine, &fakes, &admin, "acct-1");

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
        sign_in_chatgpt(&engine, &fakes, &admin, "acct-1");
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
    fn signing_out_waits_for_a_renewal_in_flight() {
        let (engine, fakes, admin, _) = setup();
        sign_in_chatgpt(&engine, &fakes, &admin, "acct-1");
        age_login(&engine, ProviderId::Openai, 10);

        // While the renewal is in flight an admin signs out (on another thread, as a request would).
        let signing_out = std::sync::Arc::new(Mutex::new(None));
        let (out, who, handle) = (engine.clone(), admin.clone(), signing_out.clone());
        fakes.net.during_refresh(move || {
            let (out, who) = (out.clone(), who.clone());
            *handle.lock().unwrap() = Some(std::thread::spawn(move || {
                out.sign_out_provider(&who, ProviderId::Openai).unwrap();
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
    fn chatgpt_may_add_its_onboarding_note_to_the_state_and_nothing_else_may_differ() {
        let sign_in = |engine: &Engine, admin: &Actor, provider: ProviderId, state_suffix: &str| {
            let url = engine
                .start_provider_sign_in(admin, provider)
                .unwrap()
                .url
                .unwrap();
            let state = param_of(&url, "state");
            let pasted = format!("{OPENAI_REDIRECT}?code=c&state={state}{state_suffix}");
            engine.finish_provider_sign_in(admin, provider, &pasted)
        };
        let note = ".onboarding_entrypoint=life_sciences";
        let encoded = ".onboarding_entrypoint%3Dlife_sciences";
        for suffix in [note, encoded] {
            let (engine, fakes, admin, _) = setup();
            fakes.net.issue(&chatgpt_token("acct-1"));
            let done = sign_in(&engine, &admin, ProviderId::Openai, suffix).unwrap();
            assert_eq!(done.state, SignInState::SignedIn, "{suffix}");
        }
        // Any other addition is another sign-in's, and the note is ChatGPT's alone.
        let (engine, fakes, admin, _) = setup();
        fakes.net.issue(&chatgpt_token("acct-1"));
        for other in [".other", "x", ".onboarding_entrypoint=other"] {
            let refused = sign_in(&engine, &admin, ProviderId::Openai, other);
            assert!(matches!(refused, Err(EngineError::Invalid(_))), "{other}");
            engine
                .cancel_provider_sign_in(&admin, ProviderId::Openai)
                .unwrap();
        }
        let claude = sign_in(&engine, &admin, ProviderId::Anthropic, note);
        assert!(matches!(claude, Err(EngineError::Invalid(_))));
    }

    #[test]
    fn a_provider_has_one_sign_in_at_a_time() {
        let (engine, fakes, admin, _) = setup();
        let url = engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap()
            .url
            .unwrap();
        assert_eq!(
            engine.start_provider_sign_in(&admin, ProviderId::Openai),
            Err(EngineError::Exists("a sign-in for this provider"))
        );
        // The one that waits is untouched and still finishes.
        assert_eq!(
            engine
                .provider_sign_in(&admin, ProviderId::Openai)
                .unwrap()
                .url,
            Some(url.clone())
        );
        // Another provider has its own.
        engine
            .start_provider_sign_in(&admin, ProviderId::Anthropic)
            .unwrap();
        fakes.net.issue(&chatgpt_token("acct-1"));
        engine
            .finish_provider_sign_in(&admin, ProviderId::Openai, &chatgpt_callback(&url, "c"))
            .unwrap();
        assert_eq!(
            state_of(&engine, &admin, ProviderId::Openai),
            SignInState::SignedIn
        );
    }

    #[test]
    fn a_sign_in_that_waited_too_long_is_given_up_and_can_be_started_again() {
        let (engine, fakes, admin, _) = setup();
        fakes.net.issue(&chatgpt_token("acct-1"));
        let url = engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap()
            .url
            .unwrap();
        age_attempt(&engine, ProviderId::Openai);
        let given_up = engine.provider_sign_in(&admin, ProviderId::Openai).unwrap();
        assert_eq!(given_up.state, SignInState::Failed);
        assert!(given_up.error.unwrap().contains("too long"));
        // What is pasted after that is not taken.
        assert!(matches!(
            engine.finish_provider_sign_in(
                &admin,
                ProviderId::Openai,
                &chatgpt_callback(&url, "c")
            ),
            Err(EngineError::Invalid(_))
        ));
        // Starting does not need a look at it first, and a new one replaces it.
        let restarted = engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        assert_eq!(restarted.state, SignInState::Waiting);
        assert_ne!(restarted.url, Some(url));
    }

    #[test]
    fn signing_in_again_replaces_the_account_only_when_it_succeeds() {
        let (engine, fakes, admin, _) = setup();
        sign_in_chatgpt(&engine, &fakes, &admin, "acct-1");
        let config = engine.agent_config();
        let account = |engine: &Engine| match engine
            .run_credential(&config, AgentKind::Codex)
            .unwrap()
            .unwrap()
            .login
        {
            Some(RunLogin::Openai { account_id, .. }) => account_id,
            other => panic!("{other:?}"),
        };

        // A sign-in given up leaves the account that is signed in; the failure is only told.
        engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap();
        assert_eq!(account(&engine), "acct-1", "still the one in use meanwhile");
        age_attempt(&engine, ProviderId::Openai);
        let failed = engine.provider_sign_in(&admin, ProviderId::Openai).unwrap();
        assert_eq!(
            (failed.state, failed.account.as_deref()),
            (SignInState::SignedIn, Some("dana@example.com"))
        );
        assert!(failed.error.unwrap().contains("too long"));
        assert_eq!(account(&engine), "acct-1");

        // So does a code the provider refuses.
        let url = engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap()
            .url
            .unwrap();
        assert!(
            engine
                .finish_provider_sign_in(&admin, ProviderId::Openai, &chatgpt_callback(&url, "bad"))
                .is_err()
        );
        assert_eq!(account(&engine), "acct-1");

        // One that succeeds replaces it.
        fakes.net.issue(&chatgpt_token("acct-2"));
        engine
            .finish_provider_sign_in(&admin, ProviderId::Openai, &chatgpt_callback(&url, "good"))
            .unwrap();
        assert_eq!(account(&engine), "acct-2");
    }

    #[test]
    fn a_renewal_in_flight_cannot_bring_back_the_account_a_new_sign_in_replaced() {
        let (engine, fakes, admin, _) = setup();
        sign_in_claude(&engine, &admin);
        // The test engine has a Claude key of its own; the login is what runs use here.
        engine
            .choose_provider_auth(&admin, ProviderId::Anthropic, "oauth")
            .unwrap();
        age_login(&engine, ProviderId::Anthropic, 10);
        let url = engine
            .start_provider_sign_in(&admin, ProviderId::Anthropic)
            .unwrap()
            .url
            .unwrap();
        let verifier = url.split("state=").nth(1).unwrap().to_owned();

        // The new account's code is pasted (on another thread, as a request would be) while the
        // old account's login is being renewed.
        let pasting = std::sync::Arc::new(Mutex::new(None));
        let (paste, who, handle) = (engine.clone(), admin.clone(), pasting.clone());
        fakes.net.during_refresh(move || {
            let (paste, who, verifier) = (paste.clone(), who.clone(), verifier.clone());
            *handle.lock().unwrap() = Some(std::thread::spawn(move || {
                paste
                    .finish_provider_sign_in(
                        &who,
                        ProviderId::Anthropic,
                        &format!("new-code#{verifier}"),
                    )
                    .unwrap();
            }));
            std::thread::sleep(std::time::Duration::from_millis(150));
        });
        engine
            .run_credential(&engine.agent_config(), AgentKind::Claude)
            .unwrap();
        pasting.lock().unwrap().take().unwrap().join().unwrap();

        // The renewal gave "sk-ant-oat-2"; the new sign-in came after it and is what is kept.
        assert_eq!(
            engine
                .stored_login(ProviderId::Anthropic)
                .unwrap()
                .unwrap()
                .access,
            "sk-ant-oat-1"
        );
        assert_eq!(fakes.net.refreshes(), 1);
    }

    #[test]
    fn looking_at_a_sign_in_does_not_wait_for_one_in_flight() {
        let (engine, fakes, admin, _) = setup();
        fakes.net.issue(&chatgpt_token("acct-1"));
        let url = engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap()
            .url
            .unwrap();

        // Another page looks while the code is being exchanged: it gets the state as it is.
        let seen = std::sync::Arc::new(Mutex::new(None));
        let (look, who, saw) = (engine.clone(), admin.clone(), seen.clone());
        fakes.net.during_exchange(move || {
            *saw.lock().unwrap() = Some(look.provider_sign_in(&who, ProviderId::Openai).unwrap());
        });
        let done = engine
            .finish_provider_sign_in(&admin, ProviderId::Openai, &chatgpt_callback(&url, "c"))
            .unwrap();
        assert_eq!(done.state, SignInState::SignedIn);
        let during = seen.lock().unwrap().take().unwrap();
        assert_eq!(during.state, SignInState::Waiting);
    }

    #[test]
    fn cancelling_does_not_undo_a_sign_in_that_is_finishing() {
        let (engine, fakes, admin, _) = setup();
        fakes.net.issue(&chatgpt_token("acct-1"));
        let url = engine
            .start_provider_sign_in(&admin, ProviderId::Openai)
            .unwrap()
            .url
            .unwrap();

        // The cancel arrives while the code is exchanged: it comes after the sign-in.
        let cancelling = std::sync::Arc::new(Mutex::new(None));
        let (cancel, who, handle) = (engine.clone(), admin.clone(), cancelling.clone());
        fakes.net.during_exchange(move || {
            let (cancel, who) = (cancel.clone(), who.clone());
            *handle.lock().unwrap() = Some(std::thread::spawn(move || {
                cancel
                    .cancel_provider_sign_in(&who, ProviderId::Openai)
                    .unwrap();
            }));
            std::thread::sleep(std::time::Duration::from_millis(150));
        });
        engine
            .finish_provider_sign_in(&admin, ProviderId::Openai, &chatgpt_callback(&url, "c"))
            .unwrap();
        cancelling.lock().unwrap().take().unwrap().join().unwrap();
        assert_eq!(
            state_of(&engine, &admin, ProviderId::Openai),
            SignInState::SignedIn
        );
    }

    #[test]
    fn the_choice_between_key_and_login_is_kept() {
        let (engine, fakes, admin, _) = setup();
        engine
            .save_provider_key(&admin, ProviderId::Openai, "sk-good-wxyz")
            .unwrap();
        sign_in_chatgpt(&engine, &fakes, &admin, "acct-1");
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
            parse_authorization_input(
                "http://localhost:1455/auth/callback?code=a%2Fb&state=s.x%3Dy#frag"
            ),
            some("a/b", Some("s.x=y")),
            "escapes in an address are undone"
        );
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
