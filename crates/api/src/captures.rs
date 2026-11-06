//! Check and capture routes: render screens and stories of a revision, list stored captures with
//! their staleness, and read a capture's image or element map.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use charet_engine::{CaptureStatus, ComponentGroup, Engine, EngineError, Rendered, SheetItem};
use serde::{Deserialize, Serialize};

use crate::projects::blocking;
use crate::{ApiError, Caller};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenderInput {
    rev: Option<String>,
    /// Screen or story ids; empty or absent = all.
    #[serde(default)]
    targets: Vec<String>,
    /// `light` (default) and / or `dark`.
    #[serde(default)]
    themes: Vec<String>,
    /// Capture only: render again although the stored capture is at this commit.
    #[serde(default)]
    force: bool,
    /// Check only: return each picture (JPEG, base64); still nothing is stored.
    #[serde(default)]
    image: bool,
}

pub async fn check(
    State(engine): State<Engine>,
    Caller(actor): Caller,
    Path(key): Path<String>,
    Json(input): Json<RenderInput>,
) -> Result<Json<Rendered>, ApiError> {
    let rev = input.rev.unwrap_or_else(|| "main".into());
    let rendered = blocking(move || {
        if input.image {
            engine.look(&actor, &key, &rev, &input.targets, &input.themes)
        } else {
            engine.check(&actor, &key, &rev, &input.targets, &input.themes)
        }
    })
    .await?;
    Ok(Json(rendered))
}

pub async fn capture(
    State(engine): State<Engine>,
    Caller(actor): Caller,
    Path(key): Path<String>,
    Json(input): Json<RenderInput>,
) -> Result<Json<Rendered>, ApiError> {
    let rev = input.rev.unwrap_or_else(|| "main".into());
    let rendered = blocking(move || {
        engine.capture(
            &actor,
            &key,
            &rev,
            &input.targets,
            &input.themes,
            input.force,
        )
    })
    .await?;
    Ok(Json(rendered))
}

#[derive(Serialize)]
pub struct CaptureList {
    captures: Vec<CaptureStatus>,
}

pub async fn list(
    State(engine): State<Engine>,
    Caller(actor): Caller,
    Path(key): Path<String>,
) -> Result<Json<CaptureList>, ApiError> {
    let captures = blocking(move || engine.captures(&actor, &key)).await?;
    Ok(Json(CaptureList { captures }))
}

pub async fn image(
    State(engine): State<Engine>,
    Caller(actor): Caller,
    Path((key, kind, target, theme)): Path<(String, String, String, String)>,
) -> Result<Response, ApiError> {
    let bytes =
        blocking(move || engine.capture_image(&actor, &key, &kind, &target, &theme)).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/jpeg"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        bytes,
    )
        .into_response())
}

pub async fn elements(
    State(engine): State<Engine>,
    Caller(actor): Caller,
    Path((key, kind, target, theme)): Path<(String, String, String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let map =
        blocking(move || engine.capture_elements(&actor, &key, &kind, &target, &theme)).await?;
    Ok(Json(map))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentsQuery {
    /// `light` (default) or `dark`.
    theme: Option<String>,
}

#[derive(Serialize)]
pub struct ComponentList {
    components: Vec<ComponentGroup>,
}

pub async fn components(
    State(engine): State<Engine>,
    Caller(actor): Caller,
    Path(key): Path<String>,
    Query(query): Query<ComponentsQuery>,
) -> Result<Json<ComponentList>, ApiError> {
    let theme = query.theme.unwrap_or_else(|| "light".into());
    let components = blocking(move || engine.components(&actor, &key, &theme)).await?;
    Ok(Json(ComponentList { components }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SheetQuery {
    /// `kind:target` items separated by commas; `planned` for a step with no screen yet.
    items: String,
    theme: Option<String>,
}

fn sheet_items(items: &str) -> Result<Vec<SheetItem>, EngineError> {
    items
        .split(',')
        .map(|item| match item.split_once(':') {
            Some((kind @ ("screen" | "story"), target)) if !target.is_empty() => {
                Ok(SheetItem::Capture {
                    kind: kind.to_owned(),
                    target: target.to_owned(),
                })
            }
            _ if item == "planned" => Ok(SheetItem::Planned),
            _ => Err(EngineError::Invalid(format!("unknown sheet item: {item}"))),
        })
        .collect()
}

pub async fn sheet(
    State(engine): State<Engine>,
    Caller(actor): Caller,
    Path(key): Path<String>,
    Query(query): Query<SheetQuery>,
) -> Result<Response, ApiError> {
    let items = sheet_items(&query.items)?;
    let theme = query.theme.unwrap_or_else(|| "light".to_owned());
    let bytes = blocking(move || engine.sheet(&actor, &key, &items, &theme)).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/jpeg"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        bytes,
    )
        .into_response())
}
