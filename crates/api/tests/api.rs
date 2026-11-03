//! Black-box tests of the router, plus the contract check.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use charet_db::{CaptureStore, CommentStore, Db};
use charet_engine::{
    Build, BuildState, BuiltFile, Engine, Ports, PreviewSettings, RenderJob, Renderer, Shot,
    Workspace, WorkspaceError,
};
use charet_store::GitStore;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

fn engine(local_mode: bool) -> Engine {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let root = std::env::temp_dir().join(format!(
        "charet-api-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    let db = Arc::new(Db::memory().unwrap());
    Engine::new(
        Ports {
            settings: db.clone(),
            records: db.clone(),
            store: Arc::new(GitStore::new(root.join("projects"))),
            workspace: Arc::new(ReadyWorkspace),
            renderer: Arc::new(FakeRenderer),
            captures: Arc::new(CaptureStore::new(db.clone(), root.join("captures"))),
            comments: Arc::new(CommentStore::new(db.clone(), root.join("comments"))),
            accounts: db.clone(),
            agents: db.clone(),
            notices: db,
            webhooks: Arc::new(TeapotSender),
            realtime: Arc::new(charet_collab::Hub::new()),
        },
        local_mode,
    )
    .with_secret([3; 32])
}

/// Every webhook receiver answers 418, so tests can tell a delivery happened.
struct TeapotSender;

impl charet_engine::WebhookSender for TeapotSender {
    fn post(&self, _: &str, _: &[(&str, String)], _: &[u8]) -> Result<u16, String> {
        Ok(418)
    }
}

/// Every build is ready at once and has one page.
struct ReadyWorkspace;

impl Workspace for ReadyWorkspace {
    fn build(&self, _: &str, _: &str) -> Result<Option<Build>, WorkspaceError> {
        Ok(None)
    }
    fn start_build(
        &self,
        _: &str,
        _: &str,
        base: &str,
        source: Vec<u8>,
        _: bool,
    ) -> Result<Build, WorkspaceError> {
        assert_eq!(base, "./", "preview builds are relative");
        assert!(source.starts_with(&[0x1f, 0x8b]), "gzip source");
        Ok(Build {
            base: base.to_owned(),
            state: BuildState::Ready,
            steps: Vec::new(),
            error: String::new(),
        })
    }
    fn file(&self, _: &str, _: &str, path: &str) -> Result<Option<BuiltFile>, WorkspaceError> {
        let body: &[u8] = match path {
            ".charet/frame.html" => b"<!doctype html>",
            ".charet/manifest.json" => {
                br#"{"manifestVersion":1,"screens":[{"id":"home","size":{"w":800,"h":600}}],"stories":[]}"#
            }
            _ => return Ok(None),
        };
        Ok(Some(BuiltFile {
            content_type: "text/html; charset=utf-8".into(),
            cache_control: "no-cache".into(),
            body: body.to_vec(),
        }))
    }
}

/// Every page is ready; dark pages report a missing mock.
struct FakeRenderer;

impl Renderer for FakeRenderer {
    fn render(&self, job: &RenderJob) -> Result<Shot, WorkspaceError> {
        let issues = if job.url.contains("theme=dark") {
            vec!["[charet] missing mock GET /api/x".to_owned()]
        } else {
            Vec::new()
        };
        Ok(Shot {
            ready: Some("true".into()),
            issues,
            image: job.image.map(|_| b"\xff\xd8jpeg".to_vec()),
            elements: job
                .map
                .then(|| json!([{ "x": 0, "y": 0, "w": 10, "h": 10, "chain": ["Home"], "sel": "body", "tag": "main", "text": "", "src": "src/home.tsx:1:1" }])),
        })
    }
}

fn preview_settings() -> PreviewSettings {
    PreviewSettings {
        origin: "http://127.0.0.1:5281".into(),
        render_origin: "http://127.0.0.1:5281".into(),
        secret: [3; 32],
        ttl_secs: 600,
    }
}

/// The owner's token on a local engine: nobody is trusted for their address, so tests act as the
/// owner by bringing it, as the admin link does.
fn owner_token(engine: &Engine) -> Option<String> {
    if !engine.local_mode() {
        return None;
    }
    engine.ensure_owner().unwrap();
    engine.owner_token().unwrap()
}

async fn send(engine: Engine, mut request: Request<Body>) -> (StatusCode, Vec<u8>, Option<String>) {
    let headers = request.headers();
    if !headers.contains_key("authorization")
        && !headers.contains_key("cookie")
        && let Some(token) = owner_token(&engine)
    {
        request
            .headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
    }
    let response = charet_api::router(engine).oneshot(request).await.unwrap();
    let status = response.status();
    let commit = response
        .headers()
        .get("x-charet-commit")
        .map(|v| v.to_str().unwrap().to_owned());
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, bytes.to_vec(), commit)
}

async fn post(engine: Engine, path: &str, body: &Value) -> (StatusCode, Value) {
    let request = Request::post(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, bytes, _) = send(engine, request).await;
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// A `POST` with no body that answers with none (`204`), or with an error body.
async fn post_empty(engine: Engine, path: &str) -> StatusCode {
    send(engine, Request::post(path).body(Body::empty()).unwrap())
        .await
        .0
}

fn b64(text: &str) -> String {
    STANDARD.encode(text)
}

async fn call(engine: Engine, path: &str) -> (StatusCode, Value) {
    let (status, bytes, _) = send(engine, Request::get(path).body(Body::empty()).unwrap()).await;
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn import_checkout_push_versions_diff() {
    let engine = engine(true);
    let import = json!({
        "key": "web",
        "title": "Web",
        "import": { "source": "test", "files": [
            { "path": "src/a.ts", "content": b64("export const a = 1;\n") },
            { "path": "run.sh", "content": b64("#!/bin/sh\n"), "executable": true }
        ]}
    });
    let (status, created) = post(engine.clone(), "/api/projects", &import).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["project"]["origin"], "import");
    assert_eq!(created["version"]["number"], 1);
    let v1 = created["version"]["commit"].as_str().unwrap().to_owned();

    let (status, again) = post(engine.clone(), "/api/projects", &json!({ "key": "web" })).await;
    assert_eq!(
        (status, again["code"].as_str()),
        (StatusCode::CONFLICT, Some("exists"))
    );

    let (status, bytes, commit) = send(
        engine.clone(),
        Request::get("/api/projects/web/checkout")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(commit.as_deref(), Some(v1.as_str()));
    assert_eq!(&bytes[..2], &[0x1f, 0x8b]);

    // Changes reach main only through a branch.
    let proposal = json!({ "title": "edit", "from": v1, "files": [
        { "path": "src/a.ts", "content": b64("export const a = 2;\n") },
        { "path": "run.sh" }
    ]});
    let (status, branch) = post(engine.clone(), "/api/projects/web/branches", &proposal).await;
    assert_eq!(status, StatusCode::CREATED, "{branch}");
    let (status, v2) = post(
        engine.clone(),
        "/api/projects/web/branches/1/merge",
        &json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{v2}");
    assert_eq!(
        (v2["number"].as_u64(), v2["kind"].as_str()),
        (Some(2), Some("merge"))
    );

    let (_, versions) = call(engine.clone(), "/api/projects/web/versions").await;
    assert_eq!(versions["versions"].as_array().unwrap().len(), 2);
    let (_, diff) = call(engine.clone(), "/api/projects/web/diff?from=v1").await;
    assert_eq!(
        diff,
        json!({ "changes": [
            { "path": "run.sh", "kind": "deleted" },
            { "path": "src/a.ts", "kind": "modified" }
        ]})
    );
    let (_, list) = call(engine.clone(), "/api/projects").await;
    assert_eq!(list["projects"][0]["key"], "web");
}

#[tokio::test]
async fn branches_propose_review_merge_restore() {
    let engine = engine(true);
    let import = json!({ "key": "web", "import": { "source": "test", "files": [
        { "path": "a.txt", "content": b64("1\n") }
    ]}});
    let (_, created) = post(engine.clone(), "/api/projects", &import).await;
    let v1 = created["version"]["commit"].as_str().unwrap().to_owned();

    let proposal = json!({ "title": "Two", "from": v1, "files": [
        { "path": "a.txt", "content": b64("2\n") }
    ]});
    let (status, branch) = post(engine.clone(), "/api/projects/web/branches", &proposal).await;
    assert_eq!(status, StatusCode::CREATED, "{branch}");
    assert_eq!(
        (branch["number"].as_u64(), branch["status"].as_str()),
        (Some(1), Some("open"))
    );

    let push =
        json!({ "base": branch["head"], "files": [{ "path": "b.txt", "content": b64("b") }] });
    let (status, pushed) = post(engine.clone(), "/api/projects/web/branches/1/push", &push).await;
    assert_eq!(status, StatusCode::OK, "{pushed}");
    let (status, stale) = post(engine.clone(), "/api/projects/web/branches/1/push", &push).await;
    assert_eq!(
        (status, stale["code"].as_str()),
        (StatusCode::CONFLICT, Some("conflict"))
    );

    let (status, reviewed) = post(
        engine.clone(),
        "/api/projects/web/branches/1/request-review",
        &json!({}),
    )
    .await;
    assert_eq!(
        (status, reviewed["status"].as_str()),
        (StatusCode::OK, Some("in_review"))
    );
    let (_, detail) = call(engine.clone(), "/api/projects/web/branches/1").await;
    assert_eq!(
        detail["changes"],
        json!([{ "path": "a.txt", "kind": "modified" }, { "path": "b.txt", "kind": "added" }])
    );
    assert_eq!(detail["behind"], false);
    assert_eq!(detail["events"].as_array().unwrap().len(), 4);

    let (status, v2) = post(
        engine.clone(),
        "/api/projects/web/branches/1/merge",
        &json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{v2}");
    assert_eq!(
        (v2["kind"].as_str(), v2["branch"].as_u64()),
        (Some("merge"), Some(1))
    );
    let (_, list) = call(engine.clone(), "/api/projects/web/branches").await;
    assert_eq!(list["branches"][0]["status"], "merged");

    let (status, v3) = post(
        engine.clone(),
        "/api/projects/web/versions/2/restore",
        &json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{v3}");
    assert_eq!(
        (v3["kind"].as_str(), v3["restored"].as_u64()),
        (Some("restore"), Some(2))
    );
    let (_, diff) = call(engine.clone(), "/api/projects/web/diff?from=v1").await;
    assert_eq!(diff, json!({ "changes": [] }));

    // Two drafts touching the same line: the second clashes after the first is merged.
    for (n, text) in [(2, "x\n"), (3, "y\n")] {
        let body = json!({ "title": format!("t{n}"), "files": [{ "path": "a.txt", "content": b64(text) }] });
        let (status, _) = post(engine.clone(), "/api/projects/web/branches", &body).await;
        assert_eq!(status, StatusCode::CREATED);
    }
    let (status, _) = post(
        engine.clone(),
        "/api/projects/web/branches/2/merge",
        &json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, clash) = post(
        engine.clone(),
        "/api/projects/web/branches/3/merge",
        &json!({}),
    )
    .await;
    assert_eq!(
        (status, clash["code"].as_str()),
        (StatusCode::CONFLICT, Some("clash"))
    );
    assert_eq!(clash["detail"]["files"], json!(["a.txt"]));
}

#[tokio::test]
async fn delete_project() {
    let engine = engine(true);
    let (status, _) = post(engine.clone(), "/api/projects", &json!({ "key": "web" })).await;
    assert_eq!(status, StatusCode::CREATED);
    let delete = || {
        Request::delete("/api/projects/web")
            .body(Body::empty())
            .unwrap()
    };
    let (status, _, _) = send(engine.clone(), delete()).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(engine.clone(), "/api/projects/web").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = send(engine, delete()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bad_requests() {
    let engine = engine(true);
    let (status, body) = post(engine.clone(), "/api/projects", &json!({ "key": "Bad" })).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid"))
    );
    let traversal = json!({ "key": "x", "import": { "source": "t", "files": [
        { "path": "../etc/passwd", "content": b64("no") }
    ]}});
    let (status, _) = post(engine.clone(), "/api/projects", &traversal).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(engine.clone(), "/api/projects/missing").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(engine, "/api/projects/missing/checkout?rev=v1").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn health() {
    assert_eq!(
        call(engine(true), "/api/health").await,
        (StatusCode::OK, json!({ "ok": true }))
    );
}

#[tokio::test]
async fn version_reports_latest_when_known() {
    let engine = engine(true);
    let (_, body) = call(engine.clone(), "/api/version").await;
    assert_eq!(body["latest"], Value::Null);
    engine.record_latest_version("9.9.9").unwrap();
    let (_, body) = call(engine, "/api/version").await;
    assert_eq!(body["latest"], "9.9.9");
    assert_eq!(body["version"], charet_model::VERSION);
}

#[tokio::test]
async fn this_machine_s_address_grants_nothing_without_the_owner_s_token() {
    let engine = engine(true);
    engine.ensure_owner().unwrap();
    let response = charet_api::router(engine.clone())
        .oneshot(Request::get("/api/auth/me").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let body: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(
        (&body["kind"], &body["role"]),
        (&json!("visitor"), &json!("viewer"))
    );
    let response = charet_api::router(engine.clone())
        .oneshot(
            Request::get("/api/admin/users")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    // The admin link's token makes the browser or CLI the owner.
    let (status, body) = call(engine, "/api/auth/me").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({ "kind": "user", "id": body["id"], "name": "Admin", "role": "admin", "localMode": true })
    );
}

#[tokio::test]
async fn others_need_a_token_or_are_visitors() {
    let (status, body) = call(engine(false), "/api/projects").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "unauthorized");
    let (status, body) = call(engine(false), "/api/auth/me").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (&body["kind"], &body["role"]),
        (&json!("visitor"), &json!("viewer"))
    );
}

#[tokio::test]
async fn unknown_routes_use_the_error_shape() {
    let (status, body) = call(engine(true), "/api/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
}

/// Every route in the router is in the contract and every contract operation is routed.
#[test]
fn routes_match_the_contract() {
    let contract = include_str!("../../../contracts/openapi.yaml");
    let mut documented = BTreeSet::new();
    let mut path = None;
    for line in contract.lines() {
        if let Some(rest) = line.strip_prefix("  /") {
            path = rest.strip_suffix(':').map(|p| format!("/{p}"));
        } else if let (Some(p), Some(method)) = (&path, line.strip_prefix("    ")) {
            let method = method.trim_end_matches(':');
            if ["get", "post", "put", "patch", "delete"].contains(&method) {
                documented.insert((method.to_owned(), p.clone()));
            }
        } else if !line.starts_with(' ') {
            path = None;
        }
    }
    let routed: BTreeSet<_> = charet_api::ROUTES
        .iter()
        .map(|(m, p)| ((*m).to_owned(), (*p).to_owned()))
        .collect();
    assert_eq!(routed, documented);
}

#[tokio::test]
async fn preview_builds_and_serves_with_a_scoped_token() {
    let engine = engine(true).with_preview(preview_settings());
    let (status, _) = post(engine.clone(), "/api/projects", &json!({ "key": "web" })).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, preview) = call(engine.clone(), "/api/projects/web/preview").await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["state"], "ready");
    let commit = preview["commit"].as_str().unwrap().to_owned();
    let token = preview["token"].as_str().unwrap().to_owned();
    assert_eq!(
        preview["baseUrl"],
        format!("http://127.0.0.1:5281/_preview/{token}/web/{commit}/")
    );

    let origin = charet_api::PreviewOrigin {
        frame_ancestors: vec!["http://localhost:5280".into()],
        sandbox: false,
    };
    let open = |path: String| {
        let router = charet_api::preview_router(engine.clone(), &origin);
        router.oneshot(Request::get(path).body(Body::empty()).unwrap())
    };

    let page = open(format!("/_preview/{token}/web/{commit}/.charet/frame.html"))
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let headers = page.headers();
    assert!(
        headers.get("set-cookie").is_none(),
        "the token is in the path"
    );
    assert_eq!(
        headers["content-security-policy"],
        "frame-ancestors http://localhost:5280"
    );
    assert_eq!(headers["referrer-policy"], "no-referrer");
    let missing = open(format!("/_preview/{token}/web/{commit}/nope.js"))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    // A bad token, a token for another commit, or the API on the preview origin: refused.
    let bad = open(format!("/_preview/x.y/web/{commit}/.charet/frame.html"))
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::FORBIDDEN);
    let other = "0".repeat(40);
    let wrong = open(format!("/_preview/{token}/web/{other}/.charet/frame.html"))
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::FORBIDDEN);
    let api = open("/api/projects".into()).await.unwrap();
    assert_eq!(api.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn preview_without_settings_is_unavailable() {
    let engine = engine(true);
    post(engine.clone(), "/api/projects", &json!({ "key": "web" })).await;
    let (status, body) = call(engine, "/api/projects/web/preview").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "unavailable");
}

#[tokio::test]
async fn check_capture_and_read_back() {
    let engine = engine(true).with_preview(preview_settings());
    post(engine.clone(), "/api/projects", &json!({ "key": "web" })).await;

    let (status, checked) = post(
        engine.clone(),
        "/api/projects/web/check",
        &json!({ "themes": ["light", "dark"] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{checked}");
    assert_eq!(checked["results"][0]["issues"], json!([]));
    assert_eq!(
        checked["results"][1]["issues"][0],
        "[charet] missing mock GET /api/x"
    );

    let (status, captured) = post(engine.clone(), "/api/projects/web/captures", &json!({})).await;
    assert_eq!(status, StatusCode::OK, "{captured}");
    assert_eq!(captured["results"][0]["captured"], true);
    let (_, again) = post(engine.clone(), "/api/projects/web/captures", &json!({})).await;
    assert_eq!(again["results"][0]["captured"], false);

    let (status, list) = call(engine.clone(), "/api/projects/web/captures").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["captures"][0]["target"], "home");
    assert_eq!(list["captures"][0]["stale"], false);
    assert_eq!(list["captures"][0]["sources"], json!(["src/home.tsx"]));

    let (status, image, _) = send(
        engine.clone(),
        Request::get("/api/projects/web/captures/screen/home/light/image")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(image.starts_with(&[0xff, 0xd8]));
    let (status, elements) = call(
        engine.clone(),
        "/api/projects/web/captures/screen/home/light/elements",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(elements[0]["chain"], json!(["Home"]));
    let (status, _) = call(
        engine.clone(),
        "/api/projects/web/captures/screen/home/dark/image",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body) = post(
        engine,
        "/api/projects/web/check",
        &json!({ "targets": ["ghost"] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test]
async fn system_reports_services() {
    let engine = engine(true).with_tool_url("http://localhost:5280/");
    let (status, body) = call(engine, "/api/system").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["toolUrl"], "http://localhost:5280");
    assert_eq!(body["localMode"], true);
    assert_eq!(body["services"]["workspace"]["ok"], true);
    assert_eq!(body["services"]["render"]["ok"], true);
}

#[tokio::test]
async fn the_first_run_shows_at_this_machine_until_the_owner_comes_in() {
    let engine = engine(true);
    let token = engine.ensure_owner().unwrap().expect("made now");
    // A browser without a token (`send` would bring the owner's).
    let visitor = |engine: Engine| async move {
        let response = charet_api::router(engine)
            .oneshot(Request::get("/api/system").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice::<Value>(&bytes).unwrap()
    };
    assert_eq!(visitor(engine.clone()).await["firstRun"], true);
    let (_, shared) = remote(
        &engine,
        Request::get("/api/system").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(shared["firstRun"], false, "not through the remote door");
    let (status, _, _) = send(
        engine.clone(),
        Request::get("/api/auth/me")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(visitor(engine).await["firstRun"], false);
}

#[tokio::test]
async fn sheet_of_placeholders() {
    let engine = engine(true).with_preview(preview_settings());
    post(engine.clone(), "/api/projects", &json!({ "key": "web" })).await;
    let (status, image, _) = send(
        engine.clone(),
        Request::get("/api/projects/web/sheet?items=planned,screen:ghost&theme=dark")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(image.starts_with(&[0xff, 0xd8]));
    let (status, body) = call(engine, "/api/projects/web/sheet?items=page:x").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test]
async fn comment_thread() {
    let engine = engine(true).with_preview(preview_settings());
    post(engine.clone(), "/api/projects", &json!({ "key": "web" })).await;
    post(engine.clone(), "/api/projects/web/captures", &json!({})).await;

    let (status, comment) = post(
        engine.clone(),
        "/api/projects/web/comments",
        &json!({ "kind": "screen", "target": "home", "at": { "x": 5, "y": 5 }, "body": "Hi" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{comment}");
    assert_eq!(comment["anchor"]["src"], "src/home.tsx:1:1");
    assert_eq!(comment["status"], "open");
    let id = comment["id"].as_u64().unwrap();

    let (status, replied) = post(
        engine.clone(),
        &format!("/api/projects/web/comments/{id}/replies"),
        &json!({ "rev": 1, "body": "Done?" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replied}");
    let (status, stale) = post(
        engine.clone(),
        &format!("/api/projects/web/comments/{id}/resolve"),
        &json!({ "rev": 1 }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(stale["code"], "changed");
    assert_eq!(stale["detail"]["rev"], 2);
    let (status, resolved) = post(
        engine.clone(),
        &format!("/api/projects/web/comments/{id}/resolve"),
        &json!({ "rev": 2, "body": "fixed in v2" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(resolved["status"], "resolved");

    let (_, open) = call(engine.clone(), "/api/projects/web/comments?status=open").await;
    assert_eq!(open["comments"], json!([]));
    let (_, all) = call(engine.clone(), "/api/projects/web/comments").await;
    assert_eq!(all["comments"][0]["replies"].as_array().unwrap().len(), 2);
    let (status, image, _) = send(
        engine.clone(),
        Request::get(format!("/api/projects/web/comments/{id}/snapshot"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(image.starts_with(&[0xff, 0xd8]));
    let (status, _) = call(engine, "/api/projects/web/comments?status=nope").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn manifest_files_and_web() {
    let engine = engine(true).with_preview(preview_settings());
    let import = json!({
        "key": "web",
        "import": { "source": "t", "files": [{ "path": "src/a.tsx", "content": b64("export {}\n") }] }
    });
    post(engine.clone(), "/api/projects", &import).await;
    let (status, manifest) = call(engine.clone(), "/api/projects/web/manifest").await;
    assert_eq!(status, StatusCode::OK, "{manifest}");
    assert_eq!(manifest["manifest"]["screens"][0]["id"], "home");
    let (_, files) = call(engine.clone(), "/api/projects/web/files").await;
    assert_eq!(files["paths"], json!(["src/a.tsx"]));
    let (status, body, commit) = send(
        engine.clone(),
        Request::get("/api/projects/web/file?path=src/a.tsx")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"export {}\n");
    assert_eq!(commit.as_deref(), files["commit"].as_str());

    // The app: API paths stay JSON 404s, other paths fall back to the web tool.
    let web = std::env::temp_dir().join(format!("charet-web-{}", std::process::id()));
    std::fs::create_dir_all(web.join("assets")).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html>app").unwrap();
    std::fs::write(web.join("assets/a.js"), "x").unwrap();
    let app = |path: &str| {
        charet_api::app(engine.clone(), Some(web.clone()))
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
    };
    let page = app("/p/web/screens").await.unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(page.headers()["cache-control"], "no-cache");
    let asset = app("/assets/a.js").await.unwrap();
    assert_eq!(
        asset.headers()["content-type"],
        "text/javascript; charset=utf-8"
    );
    assert!(
        asset.headers()["cache-control"]
            .to_str()
            .unwrap()
            .contains("immutable")
    );
    let api = app("/api/nope").await.unwrap();
    assert_eq!(api.status(), StatusCode::NOT_FOUND);
    std::fs::remove_dir_all(&web).unwrap();
}

/// A public charet (no local door) with its owner signed in; returns the owner's cookie.
async fn public_engine() -> (Engine, String) {
    let engine = engine(false).with_tool_url("https://charet.test");
    let token = engine.ensure_owner().unwrap().unwrap();
    let cookie = browser_token(&engine, &token).await;
    (engine, cookie)
}

/// Signs a browser in with `token`; returns the session cookie.
async fn browser_token(engine: &Engine, token: &str) -> String {
    let response = charet_api::router(engine.clone())
        .oneshot(
            Request::post("/api/auth/token")
                .header("content-type", "application/json")
                .header("x-charet-client", "test")
                .body(Body::from(json!({ "token": token }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let text = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        text.contains("HttpOnly") && text.contains("Secure"),
        "{text}"
    );
    text.split(';').next().unwrap().to_owned()
}

/// An admin adds a member with `role`; returns their token and a browser cookie.
async fn member(engine: &Engine, admin: &str, name: &str, role: &str) -> (String, String) {
    let (status, made) = json_of(
        engine,
        as_member(
            "POST",
            "/api/admin/users",
            admin,
            Some(json!({ "name": name, "role": role })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let token = made["token"].as_str().unwrap().to_owned();
    let cookie = browser_token(engine, &token).await;
    (token, cookie)
}

/// A JSON request with the session cookie and the client header.
fn as_member(method: &str, path: &str, cookie: &str, body: Option<Value>) -> Request<Body> {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("cookie", cookie)
        .header("x-charet-client", "test")
        .header("content-type", "application/json");
    request
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap()
}

fn with_bearer(path: &str, token: &str) -> Request<Body> {
    Request::get(path)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

async fn json_of(engine: &Engine, request: Request<Body>) -> (StatusCode, Value) {
    let (status, body, _) = send(engine.clone(), request).await;
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[tokio::test]
async fn token_sign_in_session_csrf_and_logout() {
    let (engine, boss) = public_engine().await;
    let sign_in = |token: &str, header: bool| {
        let mut request =
            Request::post("/api/auth/token").header("content-type", "application/json");
        if header {
            request = request.header("x-charet-client", "test");
        }
        request
            .body(Body::from(json!({ "token": token }).to_string()))
            .unwrap()
    };
    let (status, _) = json_of(&engine, sign_in("x", false)).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the client header is required"
    );
    let (status, body) = json_of(&engine, sign_in("wrong", true)).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("invalid_token"))
    );

    let (_, me) = json_of(&engine, as_member("GET", "/api/auth/me", &boss, None)).await;
    assert_eq!(
        (
            me["kind"].as_str(),
            me["name"].as_str(),
            me["role"].as_str()
        ),
        (Some("user"), Some("Admin"), Some("admin"))
    );

    // Changing requests need the client header.
    let create = |with_header: bool| {
        let mut request = Request::post("/api/projects")
            .header("cookie", &boss)
            .header("content-type", "application/json");
        if with_header {
            request = request.header("x-charet-client", "web");
        }
        request
            .body(Body::from(r#"{"key":"web","title":"Web"}"#))
            .unwrap()
    };
    let (status, _) = json_of(&engine, create(false)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = json_of(&engine, create(true)).await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _) = json_of(&engine, as_member("POST", "/api/auth/logout", &boss, None)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = json_of(&engine, as_member("GET", "/api/projects", &boss, None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_second_browser_replaces_the_first_but_not_the_cli() {
    let (engine, boss) = public_engine().await;
    let (token, first) = member(&engine, &boss, "Ann", "editor").await;
    let second = browser_token(&engine, &token).await;
    let (status, body) = json_of(&engine, as_member("GET", "/api/projects", &first, None)).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("replaced"))
    );
    let (status, _) = json_of(&engine, as_member("GET", "/api/projects", &second, None)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, me) = json_of(&engine, with_bearer("/api/auth/me", &token)).await;
    assert_eq!((status, me["name"].as_str()), (StatusCode::OK, Some("Ann")));
    let (status, body) = json_of(&engine, with_bearer("/api/projects", "nope")).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("invalid_token"))
    );
}

#[tokio::test]
async fn visitors_do_what_the_server_and_project_let_them() {
    let (engine, boss) = public_engine().await;
    json_of(
        &engine,
        as_member("POST", "/api/projects", &boss, Some(json!({"key": "web"}))),
    )
    .await;
    let response = charet_api::router(engine.clone())
        .oneshot(Request::get("/api/auth/me").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let visitor = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert!(visitor.starts_with("charet_visitor="), "{visitor}");
    let id = visitor.trim_start_matches("charet_visitor=");
    let (_, me) = json_of(&engine, as_member("GET", "/api/auth/me", &visitor, None)).await;
    assert_eq!(me["id"], id, "the same visitor next time");
    assert_eq!(me["name"], charet_engine::visitor_name(id));
    let (status, _) = json_of(
        &engine,
        as_member("GET", "/api/projects/web", &visitor, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = json_of(
        &engine,
        as_member(
            "POST",
            "/api/projects/web/comments",
            &visitor,
            Some(json!({"kind": "screen", "target": "home", "at": {"x": 1, "y": 1}, "body": "Hi"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "visitors look by default");
    for who in [&visitor, &boss] {
        let (status, _) = json_of(
            &engine,
            as_member(
                "PUT",
                "/api/admin/access",
                who,
                Some(json!({"visitors": "none"})),
            ),
        )
        .await;
        if who == &visitor {
            assert_eq!(status, StatusCode::FORBIDDEN);
        } else {
            assert_eq!(status, StatusCode::OK);
        }
    }
    let (status, _) = json_of(&engine, as_member("GET", "/api/projects", &visitor, None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body) = call(engine.clone(), "/api/auth/me").await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("token_required"))
    );
    let (_, access) = json_of(&engine, as_member("GET", "/api/admin/access", &boss, None)).await;
    assert_eq!(access["visitors"], "none");
}

#[tokio::test]
async fn a_project_lets_visitors_comment_but_never_check_out() {
    let (engine, boss) = public_engine().await;
    json_of(
        &engine,
        as_member("POST", "/api/projects", &boss, Some(json!({"key": "web"}))),
    )
    .await;
    let response = charet_api::router(engine.clone())
        .oneshot(Request::get("/api/auth/me").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let visitor = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let (status, _) = json_of(
        &engine,
        as_member("GET", "/api/projects/web/checkout", &visitor, None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "no checkout for visitors");

    // This project lets them comment.
    let (status, project) = json_of(
        &engine,
        as_member(
            "PUT",
            "/api/projects/web/visitors",
            &boss,
            Some(json!({"visitors": "comment"})),
        ),
    )
    .await;
    assert_eq!(
        (
            status,
            project["visitors"].as_str(),
            project["visitorAccess"].as_str()
        ),
        (StatusCode::OK, Some("comment"), Some("comment"))
    );
    let (_, seen) = json_of(
        &engine,
        as_member("GET", "/api/projects/web", &visitor, None),
    )
    .await;
    assert_eq!(seen["visitorAccess"], "comment");
    let (status, _) = json_of(
        &engine,
        as_member(
            "POST",
            "/api/projects/web/comments",
            &visitor,
            Some(json!({"kind": "screen", "target": "home", "at": {"x": 1, "y": 1}, "body": "Hi"})),
        ),
    )
    .await;
    assert_ne!(status, StatusCode::FORBIDDEN, "commenting is allowed now");
    json_of(
        &engine,
        as_member(
            "PUT",
            "/api/projects/web/visitors",
            &boss,
            Some(json!({"visitors": null})),
        ),
    )
    .await;
}

#[tokio::test]
async fn admins_manage_members_and_their_tokens() {
    let (engine, boss) = public_engine().await;
    let (ann_token, ann) = member(&engine, &boss, "Ann", "editor").await;
    let (status, _) = json_of(&engine, as_member("GET", "/api/admin/users", &ann, None)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, list) = json_of(&engine, as_member("GET", "/api/admin/users", &boss, None)).await;
    assert_eq!(status, StatusCode::OK);
    let users = list["users"].as_array().unwrap();
    assert_eq!(users.len(), 2);
    assert_eq!(
        (&users[0]["name"], &users[0]["owner"]),
        (&json!("Admin"), &json!(true))
    );
    assert_eq!(users[1]["token"], ann_token.as_str());
    assert!(users[1]["lastUsedAt"].is_string());
    let id = users[1]["id"].as_str().unwrap().to_owned();

    let (status, changed) = json_of(
        &engine,
        as_member(
            "PATCH",
            &format!("/api/admin/users/{id}"),
            &boss,
            Some(json!({"name": "Ann K", "role": "commenter"})),
        ),
    )
    .await;
    assert_eq!(
        (status, changed["name"].as_str(), changed["role"].as_str()),
        (StatusCode::OK, Some("Ann K"), Some("commenter"))
    );
    let (status, reissued) = json_of(
        &engine,
        as_member("POST", &format!("/api/admin/users/{id}/token"), &boss, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(reissued["token"], ann_token.as_str());
    let (status, _) = json_of(&engine, with_bearer("/api/projects", &ann_token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the old token stopped");
    let (status, _) = json_of(&engine, as_member("GET", "/api/projects", &ann, None)).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the browser was signed out"
    );

    let owner = users[0]["id"].as_str().unwrap();
    let (status, _) = json_of(
        &engine,
        as_member("DELETE", &format!("/api/admin/users/{owner}"), &boss, None),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "the owner stays");
    let (status, _) = json_of(
        &engine,
        as_member("DELETE", &format!("/api/admin/users/{id}"), &boss, None),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn system_answers_without_a_token_without_details() {
    let (status, system) = call(engine(false), "/api/system").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(system["localMode"], false);
    assert!(system["services"]["workspace"].get("error").is_none());
    assert!(system.get("signIn").is_none());
}

#[tokio::test]
async fn private_projects_hide_from_non_members() {
    let (engine, boss) = public_engine().await;
    let ann = member(&engine, &boss, "ann", "editor").await.1;
    json_of(
        &engine,
        as_member(
            "POST",
            "/api/projects",
            &boss,
            Some(json!({"key": "secret", "title": "Secret"})),
        ),
    )
    .await;
    let (status, project) = json_of(
        &engine,
        as_member(
            "PATCH",
            "/api/projects/secret",
            &boss,
            Some(json!({"visibility": "private"})),
        ),
    )
    .await;
    assert_eq!(
        (status, project["visibility"].as_str()),
        (StatusCode::OK, Some("private"))
    );
    let (status, _) = json_of(
        &engine,
        as_member("GET", "/api/projects/secret", &ann, None),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (_, me) = json_of(&engine, as_member("GET", "/api/auth/me", &ann, None)).await;
    let ann_id = me["id"].as_str().unwrap();
    let (status, _, _) = send(
        engine.clone(),
        as_member(
            "PUT",
            &format!("/api/projects/secret/members/{ann_id}"),
            &boss,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, members) = json_of(
        &engine,
        as_member("GET", "/api/projects/secret/members", &boss, None),
    )
    .await;
    assert_eq!(members["members"], json!([ann_id]));
    let (status, _) = json_of(
        &engine,
        as_member("GET", "/api/projects/secret", &ann, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = json_of(
        &engine,
        as_member(
            "PATCH",
            "/api/projects/secret",
            &ann,
            Some(json!({"title": "Mine"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn realtime_room_relays_presence_and_events() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    async fn read(
        socket: &mut (
                 impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin
             ),
    ) -> Value {
        loop {
            if let Message::Text(text) = socket.next().await.unwrap().unwrap() {
                return serde_json::from_str(&text).unwrap();
            }
        }
    }

    let engine = engine(true);
    engine
        .create_project(&charet_model::Actor::Local, "web", "Web", None)
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, charet_api::router(engine.clone())).into_future());
    let url = format!("ws://{addr}/api/ws?project=web");
    let token = owner_token(&engine).unwrap();
    let request = || {
        let mut request = url.as_str().into_client_request().unwrap();
        request
            .headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        request
    };
    let (mut ann, _) = tokio_tungstenite::connect_async(request()).await.unwrap();
    let (mut bob, _) = tokio_tungstenite::connect_async(request()).await.unwrap();
    assert_eq!(read(&mut ann).await["type"], "hello");
    assert_eq!(read(&mut bob).await["peers"].as_array().unwrap().len(), 1);
    assert_eq!(read(&mut ann).await["type"], "join");

    bob.send(Message::Text(
        r#"{"type":"presence","data":{"cursor":[3,4]}}"#.into(),
    ))
    .await
    .unwrap();
    let presence = read(&mut ann).await;
    assert_eq!(presence["data"]["cursor"], json!([3, 4]));

    // A command elsewhere reaches both as an event.
    engine
        .update_project(&charet_model::Actor::Local, "web", Some("Web 2"), None)
        .unwrap();
    let event = read(&mut ann).await;
    assert_eq!(
        (event["kind"].as_str(), event["seq"].as_u64()),
        (Some("project.updated"), Some(2)),
        "version 1 was event 1"
    );
    assert_eq!(read(&mut bob).await["kind"], "project.updated");

    // Pages on other sites cannot open the channel with the visitor's cookies.
    let mut request = url.as_str().into_client_request().unwrap();
    request
        .headers_mut()
        .insert("origin", "https://evil.test".parse().unwrap());
    assert!(tokio_tungstenite::connect_async(request).await.is_err());
}

/// Boss (the owner) and Ann (editor) signed in, and a project `web`; returns their cookies.
async fn boss_and_ann() -> (Engine, String, String) {
    let (engine, boss) = public_engine().await;
    let ann = member(&engine, &boss, "ann", "editor").await.1;
    json_of(
        &engine,
        as_member(
            "POST",
            "/api/projects",
            &boss,
            Some(json!({"key": "web", "title": "Web"})),
        ),
    )
    .await;
    (engine, boss, ann)
}

#[tokio::test]
async fn inbox_gets_comments_and_marks_read() {
    let (engine, boss, ann) = boss_and_ann().await;
    let (status, _) = json_of(
        &engine,
        as_member(
            "POST",
            "/api/projects/web/comments",
            &boss,
            Some(
                json!({"kind": "screen", "target": "home", "at": {"x": 1, "y": 1}, "body": "Look"}),
            ),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, inbox) = json_of(
        &engine,
        as_member("GET", "/api/me/notifications?unread=true", &ann, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(inbox["unread"], 1);
    assert_eq!(inbox["notifications"][0]["kind"], "comment.added");
    assert_eq!(
        inbox["notifications"][0]["text"],
        "Admin commented on screen home: Look"
    );
    let (_, read) = json_of(
        &engine,
        as_member("POST", "/api/me/notifications/read", &ann, Some(json!({}))),
    )
    .await;
    assert_eq!(read["unread"], 0);
}

#[tokio::test]
async fn webhooks_are_admin_settings_with_a_secret_shown_once() {
    let (engine, boss, ann) = boss_and_ann().await;
    // Webhooks are for admins; the secret comes back once.
    let (status, _) = json_of(&engine, as_member("GET", "/api/admin/webhooks", &ann, None)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, created) = json_of(
        &engine,
        as_member(
            "POST",
            "/api/admin/webhooks",
            &boss,
            Some(json!({"url": "https://hooks.test/x", "events": ["comment.added"]})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let secret = created["secret"].as_str().unwrap();
    let id = created["webhook"]["id"].as_str().unwrap().to_owned();
    assert!(secret.ends_with(created["webhook"]["secretLast4"].as_str().unwrap()));
    let (status, tested) = json_of(
        &engine,
        as_member(
            "POST",
            &format!("/api/admin/webhooks/{id}/test"),
            &boss,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tested["lastStatus"], 418);
    let (_, list) = json_of(
        &engine,
        as_member("GET", "/api/admin/webhooks", &boss, None),
    )
    .await;
    assert_eq!(list["webhooks"].as_array().unwrap().len(), 1);
    assert!(list["webhooks"][0].get("secret").is_none());
    assert!(
        list["events"]
            .as_array()
            .unwrap()
            .contains(&json!("version.created"))
    );
    let (status, _) = json_of(
        &engine,
        as_member("DELETE", &format!("/api/admin/webhooks/{id}"), &boss, None),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn one_address_serves_previews_sandboxed_from_the_tool() {
    let mut settings = preview_settings();
    settings.origin = "https://charet.test".into();
    let engine = engine(true)
        .with_tool_url("https://charet.test")
        .with_preview(settings);
    assert!(engine.previews_on_tool());
    let (status, _) = post(engine.clone(), "/api/projects", &json!({ "key": "web" })).await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, preview) = call(engine.clone(), "/api/projects/web/preview").await;
    let commit = preview["commit"].as_str().unwrap().to_owned();
    let token = preview["token"].as_str().unwrap();
    assert_eq!(
        preview["baseUrl"],
        format!("https://charet.test/_preview/{token}/web/{commit}/")
    );
    let page = charet_api::app(engine.clone(), None)
        .oneshot(
            Request::get(format!("/_preview/{token}/web/{commit}/.charet/frame.html"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let csp = page.headers()["content-security-policy"].to_str().unwrap();
    assert!(csp.starts_with("sandbox allow-scripts"), "{csp}");
    assert!(!csp.contains("allow-same-origin"), "{csp}");
    assert!(csp.ends_with("frame-ancestors 'self'"), "{csp}");
    assert_eq!(page.headers()["access-control-allow-origin"], "*");
    // The API still answers next to it.
    let (status, _) = call(engine, "/api/health").await;
    assert_eq!(status, StatusCode::OK);
}

async fn remote(engine: &Engine, request: Request<Body>) -> (StatusCode, Value) {
    let response = charet_api::remote_app(engine.clone(), None)
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A browser signs in through the remote door; returns its session cookie.
async fn remote_sign_in(engine: &Engine, token: &str) -> String {
    let response = charet_api::remote_app(engine.clone(), None)
        .oneshot(
            Request::post("/api/auth/token")
                .header("content-type", "application/json")
                .header("x-charet-client", "test")
                .body(Body::from(json!({ "token": token }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn the_remote_door_and_this_machine_s_address_follow_the_same_rules() {
    let engine = engine(true)
        .with_tool_url("http://localhost:5280")
        .with_preview(preview_settings());
    let owner_token = engine.ensure_owner().unwrap().unwrap();
    let me = || Request::get("/api/auth/me").body(Body::empty()).unwrap();
    let (_, local) = call(engine.clone(), "/api/auth/me").await;
    assert_eq!(
        (&local["kind"], &local["name"], &local["localMode"]),
        (&json!("user"), &json!("Admin"), &json!(true))
    );
    // Through the remote door nobody is the owner: without a token, a visitor.
    let (status, body) = remote(&engine, me()).await;
    assert_eq!(
        (status, body["kind"].as_str()),
        (StatusCode::OK, Some("visitor"))
    );

    let share = json!({ "mode": "address", "url": "https://b.example.com/" });
    let request = Request::put("/api/admin/sharing")
        .header("content-type", "application/json")
        .body(Body::from(share.to_string()))
        .unwrap();
    let (status, bytes, _) = send(engine.clone(), request).await;
    let shared: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status, StatusCode::OK, "{shared}");
    assert_eq!(shared["url"], "https://b.example.com");
    let (_, system) = remote(
        &engine,
        Request::get("/api/system").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(system["localMode"], false);
    assert_eq!(system["toolUrl"], "https://b.example.com");
    assert_eq!(system["previewUrl"], "https://b.example.com");

    // An editor who came in remotely sees previews on the shared address but cannot move it.
    let (_, made) = post(
        engine.clone(),
        "/api/admin/users",
        &json!({ "name": "Eve", "role": "editor" }),
    )
    .await;
    let eve = remote_sign_in(&engine, made["token"].as_str().unwrap()).await;
    let (status, _) = post(engine.clone(), "/api/projects", &json!({ "key": "web" })).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, preview) = remote(
        &engine,
        as_member("GET", "/api/projects/web/preview", &eve, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert!(
        preview["baseUrl"]
            .as_str()
            .unwrap()
            .starts_with("https://b.example.com/_preview/"),
        "{preview}"
    );
    let (status, _) = remote(
        &engine,
        as_member("DELETE", "/api/admin/sharing", &eve, None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The owner's token works from anywhere, like any admin's.
    let owner = remote_sign_in(&engine, &owner_token).await;
    let (status, stopped) = remote(
        &engine,
        as_member("DELETE", "/api/admin/sharing", &owner, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{stopped}");
    let (status, _) = remote(&engine, me()).await;
    assert_eq!(status, StatusCode::OK, "the remote door stays open");
}

#[tokio::test]
async fn handoff_export_mark_and_import_again() {
    let engine = engine(true);
    let import = json!({ "key": "web", "import": { "source": "git@x:web.git", "commit": "aaaaaaa",
    "files": [
        { "path": "app/src/a.ts", "content": b64("a1\n") },
        { "path": "app/.charet/mocks.ts", "content": b64("m1\n") }
    ]}});
    let (status, created) = post(engine.clone(), "/api/projects", &import).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["version"]["sourceCommit"], "aaaaaaa");
    let v1 = created["version"]["commit"].as_str().unwrap().to_owned();

    let proposal = json!({ "title": "edit", "from": v1, "files": [
        { "path": "app/src/a.ts", "content": b64("a2\n") },
        { "path": "app/.charet/mocks.ts", "content": b64("m2\n") }
    ]});
    post(engine.clone(), "/api/projects/web/branches", &proposal).await;
    let (status, v2) = post(
        engine.clone(),
        "/api/projects/web/branches/1/merge",
        &json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{v2}");

    let (_, state) = call(engine.clone(), "/api/projects/web/handoff").await;
    assert_eq!(state["baseline"]["version"], 1);
    assert_eq!(state["baseline"]["repoCommit"], "aaaaaaa");
    assert_eq!(state["pending"], json!([2]));
    assert_eq!(
        state["changed"],
        json!([{ "path": "app/src/a.ts", "kind": "modified" }])
    );

    let (status, export) = call(engine.clone(), "/api/projects/web/handoff/export").await;
    assert_eq!(status, StatusCode::OK, "{export}");
    let patch = export["patch"].as_str().unwrap();
    assert!(patch.contains("+a2"), "{patch}");
    assert!(!patch.contains(".charet"), "{patch}");
    let (_, mocks) = call(
        engine.clone(),
        "/api/projects/web/handoff/export?withMocks=true",
    )
    .await;
    assert!(
        mocks["patch"]
            .as_str()
            .unwrap()
            .contains(".charet/mocks.ts")
    );

    let (status, marked) = post(
        engine.clone(),
        "/api/projects/web/versions/2/handoff",
        &json!({ "commit": "bbbbbbb" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{marked}");
    assert_eq!(marked["handoff"]["commit"], "bbbbbbb");

    let again = json!({ "source": "git@x:web.git", "commit": "ccccccc", "files": [
        { "path": "app/src/a.ts", "content": b64("a3\n") }
    ]});
    let (status, v3) = post(engine.clone(), "/api/projects/web/import", &again).await;
    assert_eq!(status, StatusCode::CREATED, "{v3}");
    assert_eq!(
        (v3["number"].as_u64(), v3["kind"].as_str()),
        (Some(3), Some("import"))
    );
    // charet's own files stay as they were.
    let (_, kept, _) = send(
        engine.clone(),
        Request::get("/api/projects/web/file?path=app/.charet/mocks.ts")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(&kept[..], b"m2\n");
}

/// Takes every run and remembers its token, for the agent door.
#[derive(Default)]
struct TokenRunner(std::sync::Mutex<Vec<String>>);

impl charet_engine::AgentRunner for TokenRunner {
    fn start(&self, run: &charet_engine::RunnerStart) -> Result<(), WorkspaceError> {
        self.0.lock().unwrap().push(run.token.clone());
        Ok(())
    }
    fn poll(
        &self,
        _: u64,
        _: u32,
    ) -> Result<Option<charet_engine::RunnerProgress>, WorkspaceError> {
        Ok(Some(charet_engine::RunnerProgress::default()))
    }
    fn cancel(&self, _: u64) -> Result<(), WorkspaceError> {
        Ok(())
    }
}

async fn door(
    engine: &Engine,
    method: &str,
    path: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    let response = charet_api::agent_app(engine.clone())
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn a_run_s_container_works_on_its_branch_through_the_agent_door_only() {
    let runner = Arc::new(TokenRunner::default());
    let engine = engine(true).with_agents(
        runner.clone(),
        charet_engine::AgentConfig {
            script: true,
            max_running: 2,
            ..charet_engine::AgentConfig::default()
        },
    );
    let (status, _) = post(engine.clone(), "/api/projects", &json!({ "key": "web" })).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, run) = post(
        engine.clone(),
        "/api/projects/web/agent-runs",
        &json!({ "prompt": "Make it blue", "agent": "script", "context": { "screen": "home" } }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    assert_eq!(run["state"], "running");
    let number = run["branch"].as_u64().unwrap();
    let token = runner.0.lock().unwrap()[0].clone();

    let (status, branch) = door(
        &engine,
        "GET",
        &format!("/api/projects/web/branches/{number}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{branch}");
    let head = branch["head"].as_str().unwrap().to_owned();
    let push = json!({ "base": head, "message": "blue", "files": [{ "path": "a.css", "content": b64("a{color:blue}") }] });
    let (status, pushed) = door(
        &engine,
        "POST",
        &format!("/api/projects/web/branches/{number}/push"),
        &token,
        Some(push.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{pushed}");
    let (status, _) = door(
        &engine,
        "POST",
        "/api/projects/web/branches/99/push",
        &token,
        Some(push),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = door(&engine, "GET", "/api/admin/users", &token, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = door(&engine, "GET", "/api/projects/web", "not-a-run", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, messages) = call(
        engine.clone(),
        &format!("/api/projects/web/agent-runs/{}/messages", run["id"]),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(messages[0]["text"], "Make it blue");
    let (status, run) = post(
        engine.clone(),
        &format!("/api/projects/web/agent-runs/{}/cancel", run["id"]),
        &json!({}),
    )
    .await;
    assert_eq!(
        (status, run["state"].as_str()),
        (StatusCode::OK, Some("canceled"))
    );
    let (status, _) = door(&engine, "GET", "/api/projects/web", &token, None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the token ends with the run"
    );
    let (_, branch) = call(
        engine.clone(),
        &format!("/api/projects/web/branches/{number}"),
    )
    .await;
    assert_eq!(branch["status"], "open", "what it saved stays");
}

/// Providers that accept every key but one that starts with `bad`, and hand out a device code.
struct StubNet;

impl charet_engine::ProviderNet for StubNet {
    fn check_key(&self, _: charet_engine::ProviderId, key: &str) -> charet_engine::KeyCheck {
        let ok = !key.starts_with("bad");
        charet_engine::KeyCheck {
            ok,
            reason: (!ok).then_some(charet_engine::KeyProblem::Invalid),
            message: if ok { "The key works." } else { "Refused." }.into(),
            models: ok.then_some(3),
        }
    }
    fn openai_device_start(
        &self,
    ) -> Result<charet_engine::DeviceAuth, charet_engine::ProviderError> {
        Ok(charet_engine::DeviceAuth {
            device_auth_id: "dev".into(),
            user_code: "WXYZ-0001".into(),
            interval_secs: 5,
        })
    }
    fn openai_device_poll(
        &self,
        _: &charet_engine::DeviceAuth,
    ) -> Result<charet_engine::DevicePoll, charet_engine::ProviderError> {
        Ok(charet_engine::DevicePoll::Pending)
    }
    fn openai_exchange(
        &self,
        _: &str,
        _: &str,
        _: &str,
    ) -> Result<charet_engine::Tokens, charet_engine::ProviderError> {
        Err(charet_engine::ProviderError::Unavailable("stub".into()))
    }
    fn openai_refresh(
        &self,
        _: &str,
    ) -> Result<charet_engine::Tokens, charet_engine::ProviderError> {
        Err(charet_engine::ProviderError::Unavailable("stub".into()))
    }
    fn anthropic_exchange(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: &str,
    ) -> Result<charet_engine::Tokens, charet_engine::ProviderError> {
        Err(charet_engine::ProviderError::Unavailable("stub".into()))
    }
    fn anthropic_refresh(
        &self,
        _: &str,
    ) -> Result<charet_engine::Tokens, charet_engine::ProviderError> {
        Err(charet_engine::ProviderError::Unavailable("stub".into()))
    }
}

async fn put(engine: Engine, path: &str, body: &Value) -> (StatusCode, Value) {
    let request = Request::put(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, bytes, _) = send(engine, request).await;
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn delete(engine: Engine, path: &str) -> StatusCode {
    send(engine, Request::delete(path).body(Body::empty()).unwrap())
        .await
        .0
}

/// An engine with the agent and stub providers on.
fn engine_with_providers() -> Engine {
    engine(true)
        .with_agents(
            Arc::new(TokenRunner::default()),
            charet_engine::AgentConfig {
                max_running: 1,
                ..charet_engine::AgentConfig::default()
            },
        )
        .with_providers(Arc::new(StubNet))
}

#[tokio::test]
async fn admins_check_and_save_provider_keys() {
    let engine = engine_with_providers();
    let base = "/api/admin/agent/providers";

    let (status, body) = call(engine.clone(), base).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ids: Vec<_> = body["providers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["id"].as_str().unwrap(), p["use"].as_str().unwrap()))
        .collect();
    assert_eq!(
        ids,
        [
            ("anthropic", "none"),
            ("openai", "none"),
            ("google", "none")
        ]
    );
    assert!(
        body["providers"][2].get("oauth").is_none(),
        "Google has no sign-in"
    );

    let (status, body) = call(engine.clone(), "/api/admin/agent/providers/nope/oauth").await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::NOT_FOUND, Some("not_found"))
    );

    // A key the provider refuses answers 200 with `ok: false`, and cannot be saved.
    let (status, check) = post(
        engine.clone(),
        &format!("{base}/openai/key/check"),
        &json!({ "key": "bad-1234" }),
    )
    .await;
    assert_eq!(
        (status, check["ok"].clone(), check["reason"].as_str()),
        (StatusCode::OK, json!(false), Some("invalid"))
    );
    let (status, _) = put(
        engine.clone(),
        &format!("{base}/openai/key"),
        &json!({ "key": "bad-1234" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, saved) = put(
        engine.clone(),
        &format!("{base}/openai/key"),
        &json!({ "key": "sk-good-wxyz" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(
        (
            saved["key"]["source"].as_str(),
            saved["key"]["last4"].as_str(),
            saved["use"].as_str()
        ),
        (Some("settings"), Some("wxyz"), Some("key"))
    );
    assert!(
        !saved.to_string().contains("sk-good"),
        "the key is never returned"
    );
    assert_eq!(
        delete(engine.clone(), &format!("{base}/openai/key")).await,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn admins_sign_in_to_providers() {
    let engine = engine_with_providers();
    let base = "/api/admin/agent/providers";
    // A device sign-in shows the code to type; cancelling leaves nobody signed in.
    let (status, waiting) = post(engine.clone(), &format!("{base}/openai/oauth"), &json!({})).await;
    assert_eq!(status, StatusCode::OK, "{waiting}");
    assert_eq!(
        (
            waiting["state"].as_str(),
            waiting["method"].as_str(),
            waiting["userCode"].as_str()
        ),
        (Some("waiting"), Some("device"), Some("WXYZ-0001"))
    );
    // One sign-in at a time per provider.
    let (status, again) = post(engine.clone(), &format!("{base}/openai/oauth"), &json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{again}");
    assert_eq!(again["code"], "exists");
    assert_eq!(
        delete(engine.clone(), &format!("{base}/openai/oauth")).await,
        StatusCode::NO_CONTENT
    );
    let (_, now) = call(engine.clone(), &format!("{base}/openai/oauth")).await;
    assert_eq!(now["state"], "signed_out");
    // Signing out is its own call (and changes nothing when nobody is signed in).
    assert_eq!(
        post_empty(engine.clone(), &format!("{base}/openai/oauth/sign-out")).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        post_empty(engine.clone(), &format!("{base}/nope/oauth/sign-out")).await,
        StatusCode::NOT_FOUND
    );
    let (status, _) = post(engine.clone(), &format!("{base}/google/oauth"), &json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = post(
        engine,
        &format!("{base}/anthropic/oauth/code"),
        &json!({ "code": "c#s" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "no Claude sign-in is waiting for a code"
    );
}
