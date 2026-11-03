//! The accounts the web agent runs on (admins): API keys, checked by the provider before they
//! are saved, and sign-ins to a subscription (Claude, `ChatGPT`).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use charet_engine::{Engine, EngineError, KeyCheck, OAuthStatus, Provider, ProviderId};
use serde::{Deserialize, Serialize};

use crate::projects::blocking;
use crate::{ApiError, Caller};

pub fn routes() -> Router<Engine> {
    Router::new()
        .route("/api/admin/agent/providers", get(list))
        .route(
            "/api/admin/agent/providers/{id}/key",
            put(save_key).delete(remove_key),
        )
        .route("/api/admin/agent/providers/{id}/key/check", post(check_key))
        .route("/api/admin/agent/providers/{id}/use", put(choose))
        .route(
            "/api/admin/agent/providers/{id}/oauth",
            get(sign_in_status).post(start_sign_in).delete(end_sign_in),
        )
        .route(
            "/api/admin/agent/providers/{id}/oauth/code",
            post(finish_sign_in),
        )
}

/// A provider named in the path; one that does not exist is a 404, like any unknown thing.
fn provider(id: &str) -> Result<ProviderId, ApiError> {
    ProviderId::parse(id).ok_or_else(|| ApiError::from(EngineError::NotFound("provider")))
}

#[derive(Serialize)]
struct Providers {
    providers: Vec<Provider>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyInput {
    key: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UseInput {
    #[serde(rename = "use")]
    using: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CodeInput {
    code: String,
}

/// `GET /api/admin/agent/providers`
async fn list(
    State(engine): State<Engine>,
    Caller(actor): Caller,
) -> Result<Json<Providers>, ApiError> {
    let providers = blocking(move || engine.providers(&actor)).await?;
    Ok(Json(Providers { providers }))
}

/// `PUT /api/admin/agent/providers/{id}/key`
async fn save_key(
    State(engine): State<Engine>,
    Caller(actor): Caller,
    Path(id): Path<String>,
    Json(input): Json<KeyInput>,
) -> Result<Json<Provider>, ApiError> {
    let id = provider(&id)?;
    Ok(Json(
        blocking(move || engine.save_provider_key(&actor, id, &input.key)).await?,
    ))
}

/// `DELETE /api/admin/agent/providers/{id}/key`
async fn remove_key(
    State(engine): State<Engine>,
    Caller(actor): Caller,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = provider(&id)?;
    blocking(move || engine.remove_provider_key(&actor, id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/admin/agent/providers/{id}/key/check`
async fn check_key(
    State(engine): State<Engine>,
    Caller(actor): Caller,
    Path(id): Path<String>,
    Json(input): Json<KeyInput>,
) -> Result<Json<KeyCheck>, ApiError> {
    let id = provider(&id)?;
    Ok(Json(
        blocking(move || engine.check_provider_key(&actor, id, &input.key)).await?,
    ))
}

/// `PUT /api/admin/agent/providers/{id}/use`
async fn choose(
    State(engine): State<Engine>,
    Caller(actor): Caller,
    Path(id): Path<String>,
    Json(input): Json<UseInput>,
) -> Result<Json<Provider>, ApiError> {
    let id = provider(&id)?;
    Ok(Json(
        blocking(move || engine.choose_provider_auth(&actor, id, &input.using)).await?,
    ))
}

/// `GET /api/admin/agent/providers/{id}/oauth`
async fn sign_in_status(
    State(engine): State<Engine>,
    Caller(actor): Caller,
    Path(id): Path<String>,
) -> Result<Json<OAuthStatus>, ApiError> {
    let id = provider(&id)?;
    Ok(Json(
        blocking(move || engine.provider_sign_in(&actor, id)).await?,
    ))
}

/// `POST /api/admin/agent/providers/{id}/oauth`
async fn start_sign_in(
    State(engine): State<Engine>,
    Caller(actor): Caller,
    Path(id): Path<String>,
) -> Result<Json<OAuthStatus>, ApiError> {
    let id = provider(&id)?;
    Ok(Json(
        blocking(move || engine.start_provider_sign_in(&actor, id)).await?,
    ))
}

/// `DELETE /api/admin/agent/providers/{id}/oauth`
async fn end_sign_in(
    State(engine): State<Engine>,
    Caller(actor): Caller,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = provider(&id)?;
    blocking(move || engine.end_provider_sign_in(&actor, id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/admin/agent/providers/{id}/oauth/code`
async fn finish_sign_in(
    State(engine): State<Engine>,
    Caller(actor): Caller,
    Path(id): Path<String>,
    Json(input): Json<CodeInput>,
) -> Result<Json<OAuthStatus>, ApiError> {
    let id = provider(&id)?;
    Ok(Json(
        blocking(move || engine.finish_provider_sign_in(&actor, id, &input.code)).await?,
    ))
}
