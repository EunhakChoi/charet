//! The calls to the model providers behind `ProviderNet`: asking whether an API key works, and the
//! two sign-in flows of the vendors' own command line tools (`ChatGPT` device code, Claude pasted
//! code), with the token renewals. The rules live in `charet_engine`; this only speaks HTTP and
//! maps what comes back.

use std::time::Duration;

use charet_engine::{
    DeviceAuth, DevicePoll, KeyCheck, KeyProblem, ProviderError, ProviderId, ProviderNet, Tokens,
    percent_encode,
};
use serde_json::{Value, json};

const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const ANTHROPIC_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

/// Where each call goes (replaced in tests).
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub anthropic_api: String,
    pub openai_api: String,
    pub google_api: String,
    pub openai_auth: String,
    pub anthropic_token: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            anthropic_api: "https://api.anthropic.com".into(),
            openai_api: "https://api.openai.com".into(),
            google_api: "https://generativelanguage.googleapis.com".into(),
            openai_auth: "https://auth.openai.com".into(),
            anthropic_token: "https://platform.claude.com/v1/oauth/token".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderClient {
    agent: ureq::Agent,
    endpoints: Endpoints,
}

impl ProviderClient {
    pub fn new() -> Self {
        Self::with_endpoints(Endpoints::default())
    }

    pub fn with_endpoints(endpoints: Endpoints) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(5)))
            .timeout_global(Some(Duration::from_secs(20)))
            .http_status_as_error(false)
            .max_redirects(0)
            .user_agent(format!("charet-server/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .new_agent();
        Self { agent, endpoints }
    }

    fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<Reply, String> {
        let mut request = self.agent.get(url);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        reply(request.call())
    }

    fn post(&self, url: &str, content_type: &str, body: &str) -> Result<Reply, String> {
        reply(
            self.agent
                .post(url)
                .header("content-type", content_type)
                .header("accept", "application/json")
                .send(body),
        )
    }

    fn post_json(&self, url: &str, body: &Value) -> Result<Reply, String> {
        self.post(url, "application/json", &body.to_string())
    }

    fn post_form(&self, url: &str, fields: &[(&str, &str)]) -> Result<Reply, String> {
        self.post(url, "application/x-www-form-urlencoded", &form(fields))
    }

    fn token_url(&self) -> String {
        format!("{}/oauth/token", self.endpoints.openai_auth)
    }
}

impl Default for ProviderClient {
    fn default() -> Self {
        Self::new()
    }
}

struct Reply {
    status: u16,
    body: String,
}

fn reply(result: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Reply, String> {
    let mut response = result.map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    Ok(Reply { status, body })
}

/// `a=1&b=2`, percent-encoded.
fn form(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

impl ProviderNet for ProviderClient {
    fn check_key(&self, provider: ProviderId, key: &str) -> KeyCheck {
        let result = match provider {
            ProviderId::Anthropic => self.get(
                &format!("{}/v1/models?limit=1000", self.endpoints.anthropic_api),
                &[("x-api-key", key), ("anthropic-version", "2023-06-01")],
            ),
            ProviderId::Openai => self.get(
                &format!("{}/v1/models", self.endpoints.openai_api),
                &[("authorization", &format!("Bearer {key}"))],
            ),
            // The key goes in a header, not the query: addresses end up in logs.
            ProviderId::Google => self.get(
                &format!("{}/v1beta/models?pageSize=1000", self.endpoints.google_api),
                &[("x-goog-api-key", key)],
            ),
        };
        match result {
            Ok(reply) => key_check(provider, reply.status, &reply.body),
            Err(_) => KeyCheck {
                ok: false,
                reason: Some(KeyProblem::Unreachable),
                message: "The provider could not be reached.".into(),
                models: None,
            },
        }
    }

    fn openai_device_start(&self) -> Result<DeviceAuth, ProviderError> {
        let url = format!(
            "{}/api/accounts/deviceauth/usercode",
            self.endpoints.openai_auth
        );
        let reply = self
            .post_json(&url, &json!({ "client_id": OPENAI_CLIENT_ID }))
            .map_err(ProviderError::Unavailable)?;
        parse_device_start(reply.status, &reply.body)
    }

    fn openai_device_poll(&self, device: &DeviceAuth) -> Result<DevicePoll, ProviderError> {
        let url = format!(
            "{}/api/accounts/deviceauth/token",
            self.endpoints.openai_auth
        );
        let reply = self
            .post_json(
                &url,
                &json!({
                    "device_auth_id": device.device_auth_id,
                    "user_code": device.user_code,
                }),
            )
            .map_err(ProviderError::Unavailable)?;
        Ok(parse_device_poll(reply.status, &reply.body))
    }

    fn openai_exchange(
        &self,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<Tokens, ProviderError> {
        let reply = self
            .post_form(
                &self.token_url(),
                &[
                    ("grant_type", "authorization_code"),
                    ("client_id", OPENAI_CLIENT_ID),
                    ("code", code),
                    ("code_verifier", verifier),
                    ("redirect_uri", redirect_uri),
                ],
            )
            .map_err(ProviderError::Unavailable)?;
        parse_tokens(reply.status, &reply.body, Refresh::Required)
    }

    fn openai_refresh(&self, refresh_token: &str) -> Result<Tokens, ProviderError> {
        let reply = self
            .post_form(
                &self.token_url(),
                &[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh_token),
                    ("client_id", OPENAI_CLIENT_ID),
                ],
            )
            .map_err(ProviderError::Unavailable)?;
        parse_tokens(reply.status, &reply.body, Refresh::Optional)
    }

    fn anthropic_exchange(
        &self,
        code: &str,
        state: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<Tokens, ProviderError> {
        let reply = self
            .post_json(
                &self.endpoints.anthropic_token,
                &json!({
                    "grant_type": "authorization_code",
                    "client_id": ANTHROPIC_CLIENT_ID,
                    "code": code,
                    "state": state,
                    "redirect_uri": redirect_uri,
                    "code_verifier": verifier,
                }),
            )
            .map_err(ProviderError::Unavailable)?;
        parse_tokens(reply.status, &reply.body, Refresh::Required)
    }

    fn anthropic_refresh(&self, refresh_token: &str) -> Result<Tokens, ProviderError> {
        let reply = self
            .post_json(
                &self.endpoints.anthropic_token,
                &json!({
                    "grant_type": "refresh_token",
                    "client_id": ANTHROPIC_CLIENT_ID,
                    "refresh_token": refresh_token,
                }),
            )
            .map_err(ProviderError::Unavailable)?;
        parse_tokens(reply.status, &reply.body, Refresh::Optional)
    }
}

/// What the models list answered: does the key work?
fn key_check(provider: ProviderId, status: u16, body: &str) -> KeyCheck {
    let no = |reason, message: &str| KeyCheck {
        ok: false,
        reason: Some(reason),
        message: message.into(),
        models: None,
    };
    match status {
        200 => {
            let list: Value = serde_json::from_str(body).unwrap_or(Value::Null);
            let field = if provider == ProviderId::Google {
                "models"
            } else {
                "data"
            };
            let models = list
                .get(field)
                .and_then(Value::as_array)
                .and_then(|models| u32::try_from(models.len()).ok());
            KeyCheck {
                ok: true,
                reason: None,
                message: "The key works.".into(),
                models,
            }
        }
        401 => no(
            KeyProblem::Invalid,
            "The provider says this key is not valid.",
        ),
        // Google answers a key it does not know with 400.
        400 if provider == ProviderId::Google => no(
            KeyProblem::Invalid,
            "The provider says this key is not valid.",
        ),
        403 => no(
            KeyProblem::Forbidden,
            "The key is valid but not allowed to list models.",
        ),
        429 => no(
            KeyProblem::RateLimited,
            "The provider is limiting this key right now.",
        ),
        _ => no(
            KeyProblem::Unreachable,
            &format!("The provider answered with status {status}."),
        ),
    }
}

/// Whether a token answer has to carry a refresh token.
#[derive(Clone, Copy)]
enum Refresh {
    /// A new login: without one it could never be renewed.
    Required,
    /// A renewal: the provider may leave the refresh token as it was, and the answer then has none.
    Optional,
}

/// What a token endpoint answered. An answer without a refresh token (a renewal that keeps the
/// old one) gives an empty `refresh`.
fn parse_tokens(status: u16, body: &str, refresh: Refresh) -> Result<Tokens, ProviderError> {
    if !(200..300).contains(&status) {
        let message = format!("status {status}: {}", error_text(body));
        // The provider said no to this code or login; anything else is worth trying again.
        return Err(if (400..500).contains(&status) && status != 429 {
            ProviderError::Rejected(message)
        } else {
            ProviderError::Unavailable(message)
        });
    }
    let json: Value = serde_json::from_str(body)
        .map_err(|_| ProviderError::Unavailable("the provider's answer is not JSON".into()))?;
    let text = |name: &str| {
        json.get(name)
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
    };
    let new_refresh = text("refresh_token");
    let (Some(access), Some(refresh), Some(expires_in)) = (
        text("access_token"),
        match refresh {
            Refresh::Required => new_refresh,
            Refresh::Optional => Some(new_refresh.unwrap_or_default()),
        },
        json.get("expires_in").and_then(Value::as_u64),
    ) else {
        return Err(ProviderError::Unavailable(
            "the provider's token answer is missing fields".into(),
        ));
    };
    Ok(Tokens {
        access,
        refresh,
        id_token: text("id_token"),
        expires_in,
    })
}

fn parse_device_start(status: u16, body: &str) -> Result<DeviceAuth, ProviderError> {
    if status == 404 {
        return Err(ProviderError::Rejected(
            "ChatGPT device sign-in is not available".into(),
        ));
    }
    if !(200..300).contains(&status) {
        return Err(ProviderError::Unavailable(format!(
            "status {status}: {}",
            error_text(body)
        )));
    }
    let json: Value = serde_json::from_str(body)
        .map_err(|_| ProviderError::Unavailable("the provider's answer is not JSON".into()))?;
    let text = |name: &str| {
        json.get(name)
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
    };
    let (Some(id), Some(code)) = (text("device_auth_id"), text("user_code")) else {
        return Err(ProviderError::Unavailable(
            "the provider's device code answer is missing fields".into(),
        ));
    };
    // The interval comes as a number or as text.
    let interval = match json.get("interval") {
        Some(Value::Number(n)) => n.as_u64(),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    };
    Ok(DeviceAuth {
        device_auth_id: id.to_owned(),
        user_code: code.to_owned(),
        interval_secs: interval.unwrap_or(5),
    })
}

/// One look at the device sign-in: not yet (403/404 or a pending code), slow down, done, failed.
fn parse_device_poll(status: u16, body: &str) -> DevicePoll {
    if (200..300).contains(&status) {
        let json: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let text = |name: &str| {
            json.get(name)
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
        };
        return match (text("authorization_code"), text("code_verifier")) {
            (Some(code), Some(verifier)) => DevicePoll::Done {
                authorization_code: code.to_owned(),
                code_verifier: verifier.to_owned(),
            },
            _ => DevicePoll::Failed("the provider's sign-in answer is missing fields".into()),
        };
    }
    if status == 403 || status == 404 {
        return DevicePoll::Pending;
    }
    let json: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let code = match json.get("error") {
        Some(Value::String(code)) => Some(code.as_str()),
        Some(error) => error.get("code").and_then(Value::as_str),
        None => None,
    };
    match code {
        Some("deviceauth_authorization_pending" | "authorization_pending") => DevicePoll::Pending,
        Some("slow_down") => DevicePoll::SlowDown,
        _ => DevicePoll::Failed(format!("status {status}: {}", error_text(body))),
    }
}

/// A short line from an error answer: its description or message, else the start of the body.
fn error_text(body: &str) -> String {
    let json: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let described = json
        .get("error_description")
        .and_then(Value::as_str)
        .or_else(|| json.pointer("/error/message").and_then(Value::as_str))
        .or_else(|| json.get("error").and_then(Value::as_str))
        .or_else(|| json.get("message").and_then(Value::as_str));
    let text = described.unwrap_or(body).trim();
    let text: String = text.chars().take(200).collect();
    if text.is_empty() {
        "no details".into()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use super::*;

    /// Answers one request with `status` and `body`; gives back what it received.
    fn serve_once(status: u16, body: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut received = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let n = stream.read(&mut buffer).unwrap();
                received.extend_from_slice(&buffer[..n]);
                let text = String::from_utf8_lossy(&received).to_string();
                if let Some((head, rest)) = text.split_once("\r\n\r\n") {
                    let length = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if rest.len() >= length {
                        break;
                    }
                }
            }
            let answer = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(answer.as_bytes()).unwrap();
            String::from_utf8_lossy(&received).to_string()
        });
        (url, handle)
    }

    fn client(url: &str) -> ProviderClient {
        ProviderClient::with_endpoints(Endpoints {
            anthropic_api: url.into(),
            openai_api: url.into(),
            google_api: url.into(),
            openai_auth: url.into(),
            anthropic_token: format!("{url}/v1/oauth/token"),
        })
    }

    #[test]
    fn a_key_is_checked_against_the_models_list_without_leaving_the_headers() {
        let (url, request) = serve_once(200, r#"{"data":[{"id":"a"},{"id":"b"}]}"#);
        let check = client(&url).check_key(ProviderId::Anthropic, "sk-ant-secret");
        assert_eq!((check.ok, check.models), (true, Some(2)));
        let request = request.join().unwrap().to_ascii_lowercase();
        assert!(
            request.starts_with("get /v1/models?limit=1000 "),
            "{request}"
        );
        assert!(request.contains("x-api-key: sk-ant-secret"));
        assert!(request.contains("anthropic-version: 2023-06-01"));

        let (url, request) = serve_once(200, r#"{"models":[{}]}"#);
        let check = client(&url).check_key(ProviderId::Google, "g-secret");
        assert_eq!((check.ok, check.models), (true, Some(1)));
        let request = request.join().unwrap().to_ascii_lowercase();
        assert!(request.contains("x-goog-api-key: g-secret"));
        assert!(
            !request.lines().next().unwrap().contains("g-secret"),
            "not in the address"
        );
    }

    #[test]
    fn a_key_that_does_not_work_says_why() {
        let why = |provider, status| key_check(provider, status, "{}").reason;
        assert_eq!(why(ProviderId::Openai, 401), Some(KeyProblem::Invalid));
        assert_eq!(why(ProviderId::Google, 400), Some(KeyProblem::Invalid));
        assert_eq!(why(ProviderId::Openai, 403), Some(KeyProblem::Forbidden));
        assert_eq!(why(ProviderId::Openai, 429), Some(KeyProblem::RateLimited));
        assert_eq!(
            why(ProviderId::Anthropic, 529),
            Some(KeyProblem::Unreachable)
        );
        // Nobody listens: the provider cannot be reached.
        let check = client("http://127.0.0.1:1").check_key(ProviderId::Openai, "k");
        assert_eq!(check.reason, Some(KeyProblem::Unreachable));
    }

    #[test]
    fn token_answers_are_read_and_refusals_told_from_outages() {
        let tokens = parse_tokens(
            200,
            r#"{"access_token":"a","refresh_token":"r","id_token":"i","expires_in":3600}"#,
            Refresh::Required,
        )
        .unwrap();
        assert_eq!(
            (
                tokens.access.as_str(),
                tokens.refresh.as_str(),
                tokens.id_token.as_deref(),
                tokens.expires_in
            ),
            ("a", "r", Some("i"), 3600)
        );
        assert!(matches!(
            parse_tokens(400, r#"{"error":"invalid_grant","error_description":"used"}"#, Refresh::Required),
            Err(ProviderError::Rejected(m)) if m.contains("used")
        ));
        assert!(matches!(
            parse_tokens(503, "down", Refresh::Required),
            Err(ProviderError::Unavailable(_))
        ));
        assert!(matches!(
            parse_tokens(429, "slow", Refresh::Required),
            Err(ProviderError::Unavailable(_))
        ));
        assert!(matches!(
            parse_tokens(200, r#"{"access_token":"a"}"#, Refresh::Required),
            Err(ProviderError::Unavailable(_))
        ));
    }

    #[test]
    fn a_renewal_may_leave_the_refresh_token_as_it_was() {
        let answer = r#"{"access_token":"a2","expires_in":3600}"#;
        let kept = parse_tokens(200, answer, Refresh::Optional).unwrap();
        assert_eq!((kept.access.as_str(), kept.refresh.as_str()), ("a2", ""));
        // A new login without one could never be renewed.
        assert!(matches!(
            parse_tokens(200, answer, Refresh::Required),
            Err(ProviderError::Unavailable(_))
        ));

        let (url, _) = serve_once(200, r#"{"access_token":"a2","expires_in":3600}"#);
        let renewed = client(&url).anthropic_refresh("r1").unwrap();
        assert_eq!(
            (renewed.access.as_str(), renewed.refresh.as_str()),
            ("a2", "")
        );
    }

    #[test]
    fn a_refresh_sends_the_form_the_provider_expects() {
        let (url, request) = serve_once(
            200,
            r#"{"access_token":"a2","refresh_token":"r2","expires_in":3600}"#,
        );
        let tokens = client(&url).openai_refresh("r 1&x").unwrap();
        assert_eq!(tokens.access, "a2");
        let request = request.join().unwrap();
        assert!(request.starts_with("POST /oauth/token "), "{request}");
        assert!(
            request
                .to_ascii_lowercase()
                .contains("content-type: application/x-www-form-urlencoded")
        );
        assert!(
            request.ends_with(
                "grant_type=refresh_token&refresh_token=r%201%26x&client_id=app_EMoamEEZ73f0CkXaXp7hrann"
            ),
            "{request}"
        );

        let (url, request) = serve_once(
            200,
            r#"{"access_token":"a","refresh_token":"r","expires_in":28800}"#,
        );
        client(&url)
            .anthropic_exchange("c", "s", "v", "https://x/cb")
            .unwrap();
        let request = request.join().unwrap();
        assert!(request.starts_with("POST /v1/oauth/token "), "{request}");
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(
            body,
            json!({
                "grant_type": "authorization_code",
                "client_id": ANTHROPIC_CLIENT_ID,
                "code": "c",
                "state": "s",
                "redirect_uri": "https://x/cb",
                "code_verifier": "v",
            })
        );
    }

    #[test]
    fn the_device_sign_in_is_read_step_by_step() {
        let start = parse_device_start(
            200,
            r#"{"device_auth_id":"d","user_code":"AB-12","interval":"7"}"#,
        )
        .unwrap();
        assert_eq!(
            (
                start.device_auth_id.as_str(),
                start.user_code.as_str(),
                start.interval_secs
            ),
            ("d", "AB-12", 7)
        );
        assert_eq!(
            parse_device_start(200, r#"{"device_auth_id":"d","user_code":"x"}"#)
                .unwrap()
                .interval_secs,
            5,
            "a missing interval is 5 seconds"
        );
        assert!(matches!(
            parse_device_start(404, ""),
            Err(ProviderError::Rejected(_))
        ));

        assert_eq!(parse_device_poll(403, ""), DevicePoll::Pending);
        assert_eq!(
            parse_device_poll(
                400,
                r#"{"error":{"code":"deviceauth_authorization_pending"}}"#
            ),
            DevicePoll::Pending
        );
        assert_eq!(
            parse_device_poll(400, r#"{"error":"slow_down"}"#),
            DevicePoll::SlowDown
        );
        assert!(
            matches!(parse_device_poll(400, r#"{"error":"expired"}"#), DevicePoll::Failed(m) if m.contains("expired"))
        );
        assert_eq!(
            parse_device_poll(200, r#"{"authorization_code":"c","code_verifier":"v"}"#),
            DevicePoll::Done {
                authorization_code: "c".into(),
                code_verifier: "v".into()
            }
        );
        assert!(matches!(
            parse_device_poll(200, "{}"),
            DevicePoll::Failed(_)
        ));
    }
}
