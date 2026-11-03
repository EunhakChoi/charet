//! charet-render: renders preview pages in Chromium for charet-server.
//!
//! - `GET  /health`
//! - `POST /v1/render`: page URL → image · element map · resources · problems (bearer token)
//!
//! Settings: `CHARET_RENDER_BIND` (default `127.0.0.1:5283`), `CHARET_RENDER_TOKEN` or `CHARET_RENDER_TOKEN_FILE` (required off
//! loopback), `CHARET_RENDER_JOBS` (pages at once, default 4), `CHARET_CHROMIUM` (browser binary),
//! `CHARET_RENDER_FORWARD` (`127.0.0.1:5281=server:5281`: reach a preview origin on another host
//! through loopback, see `forward.rs`).

mod chrome;
mod forward;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use tokio::sync::Semaphore;

use crate::chrome::{Chrome, ChromeOptions, RenderRequest};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Config {
    bind: SocketAddr,
    token: Option<String>,
    jobs: usize,
    chrome: PathBuf,
    forwards: Vec<forward::Forward>,
}

fn config(env: impl Fn(&str) -> Option<String>) -> Result<Config, String> {
    let text = env("CHARET_RENDER_BIND").unwrap_or_else(|| "127.0.0.1:5283".to_owned());
    let bind: SocketAddr = text.parse().map_err(|_| {
        format!("CHARET_RENDER_BIND {text:?} is not an address like 127.0.0.1:5283")
    })?;
    let token = match env("CHARET_RENDER_TOKEN").filter(|t| !t.is_empty()) {
        Some(token) => Some(token),
        None => match env("CHARET_RENDER_TOKEN_FILE") {
            Some(file) => Some(
                std::fs::read_to_string(&file)
                    .map_err(|e| format!("CHARET_RENDER_TOKEN_FILE {file:?}: {e}"))?
                    .trim()
                    .to_owned(),
            )
            .filter(|t| !t.is_empty()),
            None => None,
        },
    };
    if token.is_none() && !bind.ip().is_loopback() {
        return Err(format!(
            "{bind} is reachable from other machines; set CHARET_RENDER_TOKEN"
        ));
    }
    let jobs = match env("CHARET_RENDER_JOBS") {
        Some(text) => text
            .parse::<usize>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or("CHARET_RENDER_JOBS must be a positive integer")?,
        None => 4,
    };
    let chrome = env("CHARET_CHROMIUM").map_or_else(|| find_chromium(env("HOME")), PathBuf::from);
    let forwards = forward::parse(&env("CHARET_RENDER_FORWARD").unwrap_or_default())?;
    Ok(Config {
        bind,
        token,
        jobs,
        chrome,
        forwards,
    })
}

/// Playwright's headless shell if installed, else a browser on `PATH`.
fn find_chromium(home: Option<String>) -> PathBuf {
    let cache = PathBuf::from(home.unwrap_or_default()).join(".cache/ms-playwright");
    let shell = std::fs::read_dir(&cache)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("chromium_headless_shell"))
        .max();
    if let Some(dir) = shell {
        let binary = cache
            .join(dir)
            .join("chrome-headless-shell-linux64/chrome-headless-shell");
        if binary.exists() {
            return binary;
        }
    }
    for name in ["chromium", "chromium-browser", "google-chrome", "chrome"] {
        if let Some(path) = std::env::var_os("PATH")
            .into_iter()
            .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
            .map(|dir| dir.join(name))
            .find(|p| p.exists())
        {
            return path;
        }
    }
    PathBuf::from("chromium")
}

#[derive(Clone)]
struct AppState {
    chrome: Arc<Chrome>,
    slots: Arc<Semaphore>,
    token: Option<String>,
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { Json(json!({ "status": "ok" })) }))
        .route("/v1/render", post(render))
        .with_state(state)
}

async fn render(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RenderRequest>,
) -> Response {
    if let Some(token) = &state.token {
        let given = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        if !given.is_some_and(|g| constant_eq(g.as_bytes(), token.as_bytes())) {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "code": "unauthorized", "message": "missing or wrong token" })),
            )
                .into_response();
        }
    }
    if !(request.url.starts_with("http://") || request.url.starts_with("https://"))
        || !(1..=8192).contains(&request.width)
        || !(1..=8192).contains(&request.height)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "code": "invalid", "message": "need an http(s) url and a size up to 8192" })),
        )
            .into_response();
    }
    let Ok(_slot) = state.slots.acquire().await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    Json(state.chrome.render(&request).await).into_response()
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[tokio::main]
async fn main() -> ExitCode {
    let config = match config(|key| std::env::var(key).ok()) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("charet-render: {message}");
            return ExitCode::FAILURE;
        }
    };
    let chrome = Arc::new(Chrome::new(ChromeOptions {
        executable: config.chrome.clone(),
    }));
    for forward in config.forwards.clone() {
        if let Err(message) = forward::start(forward).await {
            eprintln!("charet-render: {message}");
            return ExitCode::FAILURE;
        }
    }
    let state = AppState {
        chrome: chrome.clone(),
        slots: Arc::new(Semaphore::new(config.jobs)),
        token: config.token.clone(),
    };
    let listener = match tokio::net::TcpListener::bind(config.bind).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("charet-render: cannot listen on {}: {error}", config.bind);
            return ExitCode::FAILURE;
        }
    };
    println!(
        "charet-render {} listening on {} (Chromium: {})",
        env!("CARGO_PKG_VERSION"),
        config.bind,
        config.chrome.display()
    );
    let served = axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown())
        .await;
    chrome.close().await;
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("charet-render: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn shutdown() {
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending().await,
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        () = term => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use std::collections::HashMap;
    use tower::ServiceExt;

    fn cfg(env: &[(&str, &str)]) -> Result<Config, String> {
        let env: HashMap<String, String> = env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        config(|key| env.get(key).cloned())
    }

    #[test]
    fn config_defaults_and_checks() {
        let c = cfg(&[("CHARET_CHROMIUM", "/bin/chrome")]).unwrap();
        assert_eq!(c.bind, "127.0.0.1:5283".parse().unwrap());
        assert_eq!(c.jobs, 4);
        assert_eq!(c.chrome, PathBuf::from("/bin/chrome"));
        assert!(cfg(&[("CHARET_RENDER_BIND", "0.0.0.0:5283")]).is_err());
        let open = cfg(&[
            ("CHARET_RENDER_BIND", "0.0.0.0:5283"),
            ("CHARET_RENDER_TOKEN", "t"),
            ("CHARET_RENDER_FORWARD", "127.0.0.1:5281=server:5281"),
        ])
        .unwrap();
        assert_eq!(open.forwards[0].target, "server:5281");
        assert!(cfg(&[("CHARET_RENDER_JOBS", "0")]).is_err());

        let file = std::env::temp_dir().join(format!("charet-render-token-{}", std::process::id()));
        std::fs::write(&file, "from-file\n").unwrap();
        let from_file = cfg(&[
            ("CHARET_RENDER_BIND", "0.0.0.0:5283"),
            ("CHARET_RENDER_TOKEN_FILE", file.to_str().unwrap()),
        ])
        .unwrap();
        assert_eq!(from_file.token.as_deref(), Some("from-file"));
        std::fs::remove_file(file).unwrap();
    }

    fn app(token: Option<&str>) -> Router {
        router(AppState {
            chrome: Arc::new(Chrome::new(ChromeOptions {
                executable: PathBuf::from("/nonexistent/chrome"),
            })),
            slots: Arc::new(Semaphore::new(1)),
            token: token.map(str::to_owned),
        })
    }

    async fn post(app: Router, auth: Option<&str>, body: &str) -> (StatusCode, serde_json::Value) {
        let mut request = Request::post("/v1/render").header("content-type", "application/json");
        if let Some(auth) = auth {
            request = request.header("authorization", auth);
        }
        let response = app
            .oneshot(request.body(Body::from(body.to_owned())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or_default())
    }

    /// Pages for the real-browser test: a ready page with a component, a console error and a
    /// missing mock; a page that never gets ready.
    async fn page_server() -> SocketAddr {
        let ready = r#"<!doctype html><html><body><main data-src="src/a.tsx:1:1" style="width:300px;height:100px">
            <button data-slot="button">Save</button></main>
            <script>
              console.error("boom");
              console.warn("[charet] missing mock GET /api/x");
              document.documentElement.dataset.charetReady = "true";
            </script></body></html>"#;
        let never = "<!doctype html><html><body>loading</body></html>";
        // Ready once its `fetch` is answered, as a client that never saw the mocks would call it.
        let bypass = r#"<!doctype html><html><body><script>
              fetch("/api/auth/me").catch(() => {}).finally(() => {
                document.documentElement.dataset.charetReady = "true";
              });
            </script></body></html>"#;
        // The frame's missing-mock notice, then the request it let through: one issue, not two.
        let missing = r#"<!doctype html><html><body><script>
              console.warn("[charet] missing mock GET /api/y");
              fetch("/api/y").catch(() => {}).finally(() => {
                document.documentElement.dataset.charetReady = "true";
              });
            </script></body></html>"#;
        let app = Router::new()
            .route(
                "/bypass",
                get(move || async move { axum::response::Html(bypass) }),
            )
            .route(
                "/missing",
                get(move || async move { axum::response::Html(missing) }),
            )
            .route(
                "/ready",
                get(move || async move { axum::response::Html(ready) }),
            )
            .route(
                "/never",
                get(move || async move { axum::response::Html(never) }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
    }

    #[tokio::test]
    async fn renders_in_a_real_browser() {
        let executable = std::env::var_os("CHARET_CHROMIUM")
            .map_or_else(|| find_chromium(std::env::var("HOME").ok()), PathBuf::from);
        if !executable.exists() {
            eprintln!("skipped: no Chromium ({})", executable.display());
            return;
        }
        let chrome = Chrome::new(ChromeOptions { executable });
        let addr = page_server().await;
        let request = |path: &str, ready_ms: u64| RenderRequest {
            url: format!("http://{addr}{path}"),
            width: 640,
            height: 480,
            image: Some(chrome::ImageFormat::Png),
            quality: None,
            map: true,
            ready_ms,
            settle_ms: 0,
        };
        let shot = chrome.render(&request("/ready", 10_000)).await;
        assert_eq!(shot.ready.as_deref(), Some("true"), "{:?}", shot.issues);
        assert_eq!(
            shot.issues,
            ["error: boom", "[charet] missing mock GET /api/x"],
            "page problems in source order"
        );
        let png = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            shot.image.unwrap(),
        )
        .unwrap();
        assert_eq!(&png[1..4], b"PNG");
        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 640);
        let resources = shot.resources.unwrap();
        assert!(resources[0].ends_with("/ready"));

        let bypass = chrome.render(&request("/bypass", 10_000)).await;
        assert_eq!(bypass.ready.as_deref(), Some("true"), "{:?}", bypass.issues);
        assert_eq!(bypass.issues.len(), 1, "{:?}", bypass.issues);
        assert!(
            bypass.issues[0]
                .starts_with("[charet] unmocked request reached the network: GET /api/auth/me"),
            "{:?}",
            bypass.issues
        );
        let missing = chrome.render(&request("/missing", 10_000)).await;
        assert_eq!(
            missing.issues,
            ["[charet] missing mock GET /api/y"],
            "the same request is not reported twice"
        );

        let never = chrome.render(&request("/never", 500)).await;
        assert_eq!(never.ready, None);
        assert!(
            never.issues[0].starts_with("not ready after"),
            "{:?}",
            never.issues
        );
        chrome.close().await;
    }

    #[tokio::test]
    async fn checks_token_and_input_and_reports_a_missing_browser() {
        let body = r#"{"url":"http://127.0.0.1:1/x","width":800,"height":600}"#;
        assert_eq!(
            post(app(Some("t")), None, body).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            post(app(Some("t")), Some("Bearer x"), body).await.0,
            StatusCode::UNAUTHORIZED
        );
        let bad = r#"{"url":"file:///etc/passwd","width":800,"height":600}"#;
        assert_eq!(post(app(None), None, bad).await.0, StatusCode::BAD_REQUEST);
        let (status, shot) = post(app(Some("t")), Some("Bearer t"), body).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            shot["issues"][0]
                .as_str()
                .unwrap()
                .contains("cannot start Chromium"),
            "{shot}"
        );
    }
}
