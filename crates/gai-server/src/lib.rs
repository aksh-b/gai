//! The gai HTTP API: REST for agents, sessions, runs, models and keys;
//! Server-Sent Events for live runs; an OpenAI-compatible endpoint; the
//! OpenAPI document; and the dashboard's static files.

mod a2a;
mod api;
mod auth;
mod error;
mod knowledge;
pub mod mcp_server;
mod media;
mod openai;
mod openapi;
mod setup;
mod triggers;
mod versions;
mod workflows;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::routing::get;
use gai::agent::Runtime;
use gai::{AuthMode, Gai};
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

pub use auth::{AuthPolicy, Principal};
pub use versions::{spec_yaml, unified_diff};

#[derive(Clone)]
pub struct AppState {
    pub gai: Arc<Gai>,
    pub rt: Runtime,
    pub wf: gai::orchestra::Workflows,
    pub auth: AuthPolicy,
}

pub struct ServeOptions {
    pub bind: SocketAddr,
    /// Built dashboard (`web/dist`). Served at `/` when present.
    pub ui_dir: Option<PathBuf>,
    /// Overrides `server.auth` from config.
    pub auth: Option<AuthMode>,
    /// Treat the server as reachable only from this machine even though it
    /// binds a non-loopback address (a container port published on
    /// 127.0.0.1). Lets `auth = "auto"` stay open until a key exists.
    pub local: bool,
}

/// The full router: API, OpenAI-compatible routes, OpenAPI docs and the
/// dashboard.
pub async fn router(
    gai: Arc<Gai>,
    auth: AuthPolicy,
    ui_dir: Option<PathBuf>,
) -> Result<Router, gai::Error> {
    let rt = gai.runtime().await?.clone();
    // Before recovery: the workflow engine must be registered to pick up
    // its own unfinished and queued runs.
    let wf = gai.workflows().await?.clone();
    // A server is long-lived: connect MCP servers now so their status and
    // tools show up, and pick up runs a previous process left unfinished.
    rt.ensure_mcp();
    let resumed = rt.recover().await;
    if !resumed.is_empty() {
        tracing::info!(runs = ?resumed, "resumed unfinished runs");
    }
    rt.start_queue(gai.config().server.max_background_runs);
    // Schedules and file watchers start runs on their own from now on.
    gai.triggers().await?.start_loops();
    // Runs started with an API key are charged to it when they end, however
    // they were started and whichever process finishes them.
    let store = rt.store().clone();
    rt.set_finish_hook(
        "charge-key",
        Arc::new(move |o: gai::agent::RunOutcome| {
            let store = store.clone();
            Box::pin(async move {
                if let Ok(Some(run)) = store.get_run(&o.run_id).await
                    // A workflow's cost is its agent runs', which are charged themselves.
                    && run.metadata["kind"] != "workflow"
                    && let Some(key) = run.metadata["key_id"].as_str()
                {
                    auth::charge(&store, Some(key), o.cost).await;
                }
            })
        }),
    );
    let mcp = gai.config().server.mcp.clone();
    let state = AppState { gai, rt, wf, auth };
    let mut routes = api::routes();
    if mcp.enabled {
        let handler =
            mcp_server::GaiMcp::new(state.gai.clone(), state.rt.clone(), state.wf.clone());
        routes = routes.nest_service("/mcp", mcp_server::http_service(handler, &mcp));
    }
    let mut app = routes
        .merge(knowledge::routes())
        .merge(workflows::routes())
        .merge(versions::routes())
        .merge(media::routes())
        .merge(triggers::routes())
        .merge(setup::routes())
        .merge(a2a::routes())
        .merge(openai::routes())
        .route("/openapi.json", get(openapi::spec))
        .route("/docs", get(openapi::docs))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::authenticate,
        ))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive());
    if let Some(dir) = ui_dir.filter(|d| d.join("index.html").is_file()) {
        // Client-side routes fall back to index.html.
        let spa = ServeDir::new(&dir).fallback(ServeFile::new(dir.join("index.html")));
        app = app.fallback_service(spa);
    }
    Ok(app)
}

pub async fn serve(
    gai: Arc<Gai>,
    opts: ServeOptions,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let settings = gai.config().server.clone();
    let mode = opts.auth.unwrap_or(settings.auth);
    let local = opts.local || opts.bind.ip().is_loopback();
    if mode == AuthMode::Off && !local {
        tracing::warn!(bind = %opts.bind, "authentication is off on a non-local address");
    }
    let policy = AuthPolicy::new(mode, local, settings.rate_limit_per_minute);
    let app = router(gai, policy, opts.ui_dir).await?;
    let listener = tokio::net::TcpListener::bind(opts.bind).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}
