//! charet-server: the charet server binary.

mod config;
mod lan;
mod providers;
mod secret;
mod update;
mod webhook;

use std::process::ExitCode;
use std::sync::Arc;

use charet_collab::Hub;
use charet_db::{CaptureStore, CommentStore, Db};
use charet_engine::{Engine, Ports, PreviewSettings};
use charet_render_client::RenderClient;
use charet_store::GitStore;
use charet_workspace_client::{AgentClient, WorkspaceClient};

use crate::config::Config;
use crate::providers::ProviderClient;

#[tokio::main]
async fn main() -> ExitCode {
    // `charet-server admin-token`: print the owner's token (a public server's way in) and exit.
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let admin_token = args.first().is_some_and(|a| a == "admin-token");
    if admin_token {
        args.remove(0);
    }
    let config = match Config::from_env(args, |key| std::env::var(key).ok()) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("charet-server: {message}");
            return ExitCode::FAILURE;
        }
    };
    match run(config, admin_token).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("charet-server: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Data from before the rename kept its database as `board.db`: take it over once.
fn adopt_old_database(dir: &std::path::Path) -> Result<(), String> {
    let old = dir.join("board.db");
    if !old.exists() || dir.join("charet.db").exists() {
        return Ok(());
    }
    for suffix in ["", "-wal", "-shm"] {
        let from = dir.join(format!("board.db{suffix}"));
        if from.exists() {
            std::fs::rename(&from, dir.join(format!("charet.db{suffix}")))
                .map_err(|e| format!("cannot rename {}: {e}", from.display()))?;
        }
    }
    Ok(())
}

async fn run(config: Config, admin_token: bool) -> Result<(), String> {
    std::fs::create_dir_all(config.data_dir())
        .map_err(|e| format!("cannot create {}: {e}", config.data_dir().display()))?;
    let secret = secret::load(&config.data_dir(), std::env::var("CHARET_SECRET_KEY").ok())?;
    adopt_old_database(&config.data_dir())?;
    let db = Arc::new(Db::open(&config.data_dir().join("charet.db")).map_err(|e| e.to_string())?);
    let ports = Ports {
        settings: db.clone(),
        records: db.clone(),
        captures: Arc::new(CaptureStore::new(
            db.clone(),
            config.data_dir().join("captures"),
        )),
        comments: Arc::new(CommentStore::new(
            db.clone(),
            config.data_dir().join("comments"),
        )),
        accounts: db.clone(),
        agents: db.clone(),
        notices: db,
        webhooks: Arc::new(webhook::Sender::new()),
        realtime: Arc::new(Hub::new()),
        renderer: Arc::new(RenderClient::new(
            &config.render_url,
            config.render_token.clone(),
        )),
        store: Arc::new(GitStore::new(config.data_dir().join("projects"))),
        workspace: Arc::new(WorkspaceClient::new(
            &config.workspace_url,
            config.workspace_token.clone(),
        )),
    };
    let engine = Engine::new(ports, config.local_mode())
        .with_preview(PreviewSettings {
            origin: config.preview_url.clone(),
            render_origin: config.render_preview_url.clone(),
            secret,
            ttl_secs: PREVIEW_TOKEN_SECS,
        })
        .with_tool_url(&config.tool_url())
        .with_merge_check(config.merge_check)
        .with_secret(secret);
    let engine = with_agents(engine, &config);
    // The owner: the first admin. Their token (the admin link) is the way in everywhere.
    engine.ensure_owner().map_err(|e| e.to_string())?;
    if admin_token {
        let token = engine
            .owner_token()
            .map_err(|e| e.to_string())?
            .ok_or("the owner was removed")?;
        println!("{token}");
        return Ok(());
    }
    let first_token = engine.owner_announcement().map_err(|e| e.to_string())?;
    let engine = with_sharing(engine, &config).await?;
    start_jobs(&engine, &config);
    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .map_err(|e| format!("cannot listen on {}: {e}", config.bind))?;
    let preview_listener = tokio::net::TcpListener::bind(config.preview_bind)
        .await
        .map_err(|e| format!("cannot listen on {}: {e}", config.preview_bind))?;
    announce(&config, first_token);
    let origin = charet_api::PreviewOrigin {
        frame_ancestors: config.tool_origins(),
        sandbox: false,
    };
    let remote = serve_remote(
        config.remote_bind,
        charet_api::remote_app(engine.clone(), config.web_dir.clone()),
    )
    .await?;
    let agent_door = serve_remote(
        config.agent.as_ref().map(|a| a.bind),
        charet_api::agent_app(engine.clone()),
    )
    .await?;
    let previews = axum::serve(
        preview_listener,
        charet_api::preview_router(engine.clone(), &origin),
    )
    .with_graceful_shutdown(shutdown());
    let api = axum::serve(listener, charet_api::app(engine, config.web_dir.clone()))
        .with_graceful_shutdown(shutdown());
    let (api, previews, remote, agent_door) = tokio::join!(
        api.into_future(),
        previews.into_future(),
        remote,
        agent_door
    );
    api.and(previews)
        .and(remote)
        .and(agent_door)
        .map_err(|e| e.to_string())
}

/// Turns the web agent on when `charet-agent` is configured.
fn with_agents(engine: Engine, config: &Config) -> Engine {
    match &config.agent {
        Some(agent) => engine
            .with_agents(
                Arc::new(AgentClient::new(&agent.url, agent.token.clone())),
                charet_engine::AgentConfig {
                    env_keys: agent.keys.clone(),
                    script: agent.script,
                    max_running: agent.max_running,
                    run_minutes: agent.run_minutes,
                },
            )
            .with_providers(Arc::new(ProviderClient::new())),
        None => engine,
    }
}

/// Lets a local charet run tunnels to its remote door, and reopens sharing that was on.
async fn with_sharing(mut engine: Engine, config: &Config) -> Result<Engine, String> {
    let remote = config.remote_bind;
    if let Some(addr) = remote {
        let target = format!("http://127.0.0.1:{}", addr.port());
        let runtime = tokio::runtime::Handle::current();
        let machine = charet_tunnel::Machine {
            tailscale: tailscale(&target),
            ngrok: charet_tunnel::Ngrok::new(runtime.clone(), &target)?,
        };
        let lan = lan::Lan {
            runtime,
            port: addr.port(),
            app: charet_api::remote_app(engine.clone(), config.web_dir.clone()),
            container: config.container,
            running: std::sync::Mutex::default(),
        };
        engine = engine.with_tunnels(Arc::new(lan::ServerTunnels { machine, lan }));
    }
    // Tunnels block on the runtime, so they reopen from a blocking thread.
    let restored = engine.clone();
    tokio::task::spawn_blocking(move || {
        restored.restore_sharing()?;
        restored.sharing(&charet_model::Actor::Local)
    })
    .await
    .map_err(|e| e.to_string())?
    .map(|status| {
        if let Some(warning) = status.warning {
            eprintln!("charet-server: sharing: {warning}");
        }
    })
    .map_err(|e| e.to_string())?;
    Ok(engine)
}

/// The tailscale CLI, sending people to the remote door at `target`.
fn tailscale(target: &str) -> charet_tunnel::Tailscale {
    charet_tunnel::Tailscale {
        cli: std::env::var("CHARET_TAILSCALE").unwrap_or_else(|_| "tailscale".to_owned()),
        socket: std::env::var("CHARET_TAILSCALE_SOCKET").ok(),
        // tailscaled runs on the host: in docker the remote door is published on its loopback.
        target: std::env::var("CHARET_TAILSCALE_TARGET").unwrap_or_else(|_| target.to_owned()),
    }
}

/// Says where the server listens, and prints a public server's admin token the first time.
fn announce(config: &Config, admin_token: Option<String>) {
    println!(
        "charet-server {} listening on {} ({}), previews on {}",
        charet_model::VERSION,
        config.tool_url(),
        if config.local_mode() {
            "local"
        } else {
            "public"
        },
        config.preview_url,
    );
    if let Some(agent) = &config.agent {
        println!(
            "Web agent via {}; run containers come in at the agent door on {}.",
            agent.url, agent.bind
        );
    }
    if let Some(addr) = config.remote_bind {
        println!(
            "Remote door on {addr}: tunnels and reverse proxies point here (members bring their token)."
        );
    }
    if let Some(token) = admin_token {
        // Printed once; `charet-server admin-token` (or `charet setup`) shows it again.
        println!(
            "Admin link (open it once; the browser stays signed in):\n  {}/?t={token}\nAdmin token (for the CLI: `charet login --token …`; `charet-server admin-token` shows it again):\n  {token}",
            config.tool_url()
        );
    }
}

/// Listens on a door (remote, agent) when there is one; the future runs until shutdown.
async fn serve_remote(
    addr: Option<std::net::SocketAddr>,
    app: axum::Router,
) -> Result<impl Future<Output = std::io::Result<()>>, String> {
    let listener = match addr {
        Some(addr) => Some(
            tokio::net::TcpListener::bind(addr)
                .await
                .map_err(|e| format!("cannot listen on {addr}: {e}"))?,
        ),
        None => None,
    };
    Ok(async move {
        match listener {
            Some(listener) => {
                axum::serve(listener, app)
                    .with_graceful_shutdown(shutdown())
                    .await
            }
            None => Ok(()),
        }
    })
}

/// Preview tokens last an hour; the tool asks for a fresh one when it reopens a preview.
const PREVIEW_TOKEN_SECS: u64 = 3600;

/// Resolves on Ctrl-C or SIGTERM (`docker stop`).
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

/// The jobs that run beside the server for as long as it does.
fn start_jobs(engine: &Engine, config: &Config) {
    if config.update_check {
        update::spawn(engine.clone());
    }
    archive_idle_drafts(engine.clone());
    if config.agent.is_some() {
        follow_agent_runs(engine.clone());
        keep_logins_fresh(engine.clone());
    }
}

/// Once an hour, archives drafts nobody touched for two weeks.
fn archive_idle_drafts(engine: Engine) {
    tokio::spawn(async move {
        loop {
            let run = engine.clone();
            let done = tokio::task::spawn_blocking(move || {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                run.archive_idle(now)
            })
            .await;
            if let Ok(Err(error)) = done {
                eprintln!("charet-server: cannot archive idle drafts: {error}");
            }
            tokio::time::sleep(std::time::Duration::from_hours(1)).await;
        }
    });
}

/// Every few minutes, renews the subscription logins the web agent runs on before a run needs them.
fn keep_logins_fresh(engine: Engine) {
    tokio::spawn(async move {
        let mut last_error = String::new();
        loop {
            let run = engine.clone();
            if let Ok(Err(error)) = tokio::task::spawn_blocking(move || run.providers_tick()).await
            {
                let error = error.to_string();
                if error != last_error {
                    eprintln!("charet-server: web agent logins: {error}");
                }
                last_error = error;
            } else {
                last_error.clear();
            }
            tokio::time::sleep(std::time::Duration::from_secs(
                charet_engine::PROVIDERS_TICK_SECS,
            ))
            .await;
        }
    });
}

/// Every second, takes in what web agent runs did and starts waiting ones.
fn follow_agent_runs(engine: Engine) {
    tokio::spawn(async move {
        let mut last_error = String::new();
        loop {
            let run = engine.clone();
            if let Ok(Err(error)) = tokio::task::spawn_blocking(move || run.agents_tick()).await {
                let error = error.to_string();
                if error != last_error {
                    eprintln!("charet-server: web agent: {error}");
                }
                last_error = error;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    });
}
