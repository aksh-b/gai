//! Prompt templates: instructions any agent can use with `prompt:`.
//! Versioned like agents, with labels and rollback.

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use gai::agent::{PROMPT_KIND, PromptSpec};
use gai::store::{LibraryItem, LibraryVersion};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::AppState;
use crate::api::{Done, LabelBody, Saved};
use crate::error::{ApiError, ApiResult};
use crate::orgs::Tenant;
use crate::versions::{DiffQuery, Rollback, VersionDiff, diff_of, pick};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/prompts", get(list).post(save))
        .route("/v1/prompts/render", post(render))
        .route("/v1/prompts/{name}", get(get_one).delete(delete))
        .route("/v1/prompts/{name}/versions", get(versions))
        .route("/v1/prompts/{name}/diff", get(diff))
        .route("/v1/prompts/{name}/rollback", post(rollback))
        .route(
            "/v1/prompts/{name}/labels/{label}",
            post(set_label).delete(remove_label),
        )
}

#[derive(Serialize, JsonSchema)]
pub struct PromptEntry {
    pub summary: LibraryItem,
    pub spec: Option<PromptSpec>,
    /// Stored agents that use it.
    pub used_by: Vec<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct PromptView {
    pub name: String,
    pub version: u32,
    pub spec: PromptSpec,
}

#[derive(Deserialize, JsonSchema)]
pub struct RenderPrompt {
    pub prompt: PromptSpec,
    /// Values for its variables; the prompt's defaults fill the rest.
    #[serde(default)]
    pub vars: Map<String, Value>,
}

#[derive(Serialize, JsonSchema)]
pub struct RenderedPrompt {
    pub text: String,
}

#[derive(Deserialize)]
struct SaveQuery {
    note: Option<String>,
}

#[derive(Deserialize)]
struct VersionQuery {
    version: Option<u32>,
}

async fn list(Tenant(s): Tenant) -> ApiResult<Json<Vec<PromptEntry>>> {
    let mut out = Vec::new();
    for summary in s.rt.store().list_items(PROMPT_KIND).await? {
        let spec =
            s.rt.resolve_prompt(&summary.name)
                .await
                .ok()
                .map(|(p, _)| p);
        let used_by = s.rt.prompt_users(&summary.name).await?;
        out.push(PromptEntry {
            summary,
            spec,
            used_by,
        });
    }
    Ok(Json(out))
}

async fn save(
    Tenant(s): Tenant,
    Query(q): Query<SaveQuery>,
    Json(spec): Json<PromptSpec>,
) -> ApiResult<(StatusCode, Json<Saved>)> {
    let version = s.rt.save_prompt(&spec, q.note.as_deref()).await?;
    Ok((
        StatusCode::CREATED,
        Json(Saved {
            name: spec.name,
            version,
        }),
    ))
}

/// The text as an agent would get it, with these variables.
async fn render(Json(b): Json<RenderPrompt>) -> ApiResult<Json<RenderedPrompt>> {
    let text = b.prompt.render(&b.vars).map_err(ApiError::bad_request)?;
    Ok(Json(RenderedPrompt { text }))
}

async fn get_one(
    Tenant(s): Tenant,
    Path(name): Path<String>,
    Query(q): Query<VersionQuery>,
) -> ApiResult<Json<PromptView>> {
    let reference = match q.version {
        Some(v) => format!("{name}@v{v}"),
        None => name.clone(),
    };
    let (spec, version) = s.rt.resolve_prompt(&reference).await?;
    Ok(Json(PromptView {
        name,
        version,
        spec,
    }))
}

async fn versions(
    Tenant(s): Tenant,
    Path(name): Path<String>,
) -> ApiResult<Json<Vec<LibraryVersion>>> {
    Ok(Json(
        s.rt.store().list_item_versions(PROMPT_KIND, &name).await?,
    ))
}

/// A prompt in use can't be deleted: the agents that use it would stop
/// working.
async fn delete(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<StatusCode> {
    let users = s.rt.prompt_users(&name).await?;
    if !users.is_empty() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!(
                "prompt {name} is used by {}; change them first",
                users.join(", ")
            ),
        ));
    }
    if s.rt.store().delete_item(PROMPT_KIND, &name).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("prompt {name}")))
    }
}

async fn diff(
    Tenant(s): Tenant,
    Path(name): Path<String>,
    Query(q): Query<DiffQuery>,
) -> ApiResult<Json<VersionDiff>> {
    let store = s.rt.store();
    let latest = store
        .get_item(PROMPT_KIND, &name, None)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("prompt {name}")))?;
    let (from, to) = pick(&q, latest.version)?;
    let get = |v: u32| {
        let name = name.clone();
        async move {
            store
                .get_item(PROMPT_KIND, &name, Some(v))
                .await?
                .ok_or_else(|| ApiError::not_found(format!("prompt {name} v{v}")))
        }
    };
    let (a, b) = (get(from).await?, get(to).await?);
    Ok(Json(diff_of(name, (from, &a.spec), (to, &b.spec))))
}

async fn rollback(
    Tenant(s): Tenant,
    Path(name): Path<String>,
    Json(b): Json<Rollback>,
) -> ApiResult<(StatusCode, Json<Saved>)> {
    let store = s.rt.store();
    let old = store
        .get_item(PROMPT_KIND, &name, Some(b.version))
        .await?
        .ok_or_else(|| ApiError::not_found(format!("prompt {name} v{}", b.version)))?;
    let note = b
        .note
        .unwrap_or_else(|| format!("rollback to v{}", b.version));
    let version = store
        .put_item(PROMPT_KIND, &name, &old.spec, Some(&note))
        .await?;
    Ok((StatusCode::CREATED, Json(Saved { name, version })))
}

async fn set_label(
    Tenant(s): Tenant,
    Path((name, label)): Path<(String, String)>,
    Json(b): Json<LabelBody>,
) -> ApiResult<Json<Done>> {
    s.rt.store()
        .set_item_label(PROMPT_KIND, &name, &label, Some(b.version))
        .await?;
    Ok(Json(Done { ok: true }))
}

async fn remove_label(
    Tenant(s): Tenant,
    Path((name, label)): Path<(String, String)>,
) -> ApiResult<Json<Done>> {
    s.rt.store()
        .set_item_label(PROMPT_KIND, &name, &label, None)
        .await?;
    Ok(Json(Done { ok: true }))
}
