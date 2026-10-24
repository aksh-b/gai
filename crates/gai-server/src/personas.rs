//! Personas: reusable voices for agents and chats. Versioned like agents,
//! with labels, diffs and rollback, plus the global default.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use gai::agent::{NO_PERSONA, PersonaDefaults, PersonaSpec};
use gai::store::{PersonaSummary, PersonaVersion};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::api::{Done, LabelBody, Saved};
use crate::error::{ApiError, ApiResult};
use crate::versions::{DiffQuery, Rollback, VersionDiff, diff_of, pick};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/personas", get(list).post(save))
        .route("/v1/personas/render", post(render))
        .route("/v1/personas/defaults", get(defaults).put(put_defaults))
        .route("/v1/personas/{name}", get(get_one).delete(delete))
        .route("/v1/personas/{name}/versions", get(versions))
        .route("/v1/personas/{name}/usage", get(usage))
        .route("/v1/personas/{name}/diff", get(diff))
        .route("/v1/personas/{name}/rollback", post(rollback))
        .route(
            "/v1/personas/{name}/labels/{label}",
            post(set_label).delete(remove_label),
        )
}

#[derive(Serialize, JsonSchema)]
pub struct PersonaEntry {
    pub summary: PersonaSummary,
    pub spec: Option<PersonaSpec>,
    /// Stored agents that speak as this persona by default.
    pub used_by: Vec<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct PersonaView {
    pub name: String,
    pub version: u32,
    pub spec: PersonaSpec,
}

#[derive(Serialize, JsonSchema)]
pub struct RenderedPersona {
    /// The persona as it goes into the system prompt.
    pub text: String,
}

#[derive(Serialize, JsonSchema)]
pub struct PersonaDefaultsView {
    #[serde(flatten)]
    pub defaults: PersonaDefaults,
    /// `settings` (chosen in the dashboard) or `config` (gai.toml).
    pub source: String,
}

#[derive(Deserialize)]
struct SaveQuery {
    note: Option<String>,
}

#[derive(Deserialize)]
struct VersionQuery {
    version: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
pub struct PutPersonaDefaults {
    /// Persona for runs whose agent names none; `null` for none.
    pub persona: Option<String>,
    /// `prepend` (default) or `append`.
    #[serde(default)]
    pub placement: gai::agent::Placement,
    /// Forget the dashboard's choice and go back to gai.toml.
    #[serde(default)]
    pub reset: bool,
}

async fn list(State(s): State<AppState>) -> ApiResult<Json<Vec<PersonaEntry>>> {
    let mut out = Vec::new();
    for summary in s.rt.store().list_personas().await? {
        let spec =
            s.rt.resolve_persona(&summary.name)
                .await
                .ok()
                .map(|(p, _)| p);
        let used_by = s.rt.persona_users(&summary.name).await?;
        out.push(PersonaEntry {
            summary,
            spec,
            used_by,
        });
    }
    Ok(Json(out))
}

async fn save(
    State(s): State<AppState>,
    Query(q): Query<SaveQuery>,
    Json(spec): Json<PersonaSpec>,
) -> ApiResult<(StatusCode, Json<Saved>)> {
    let version = s.rt.save_persona(&spec, q.note.as_deref()).await?;
    Ok((
        StatusCode::CREATED,
        Json(Saved {
            name: spec.name,
            version,
        }),
    ))
}

async fn render(Json(spec): Json<PersonaSpec>) -> Json<RenderedPersona> {
    Json(RenderedPersona {
        text: spec.render(),
    })
}

async fn get_one(
    State(s): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<VersionQuery>,
) -> ApiResult<Json<PersonaView>> {
    let reference = match q.version {
        Some(v) => format!("{name}@v{v}"),
        None => name.clone(),
    };
    let (spec, version) = s.rt.resolve_persona(&reference).await?;
    Ok(Json(PersonaView {
        name,
        version,
        spec,
    }))
}

async fn versions(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<Vec<PersonaVersion>>> {
    Ok(Json(s.rt.store().list_persona_versions(&name).await?))
}

async fn usage(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<Vec<String>>> {
    Ok(Json(s.rt.persona_users(&name).await?))
}

/// A persona in use can't be deleted: the agents (or the default) that
/// speak as it would stop working.
async fn delete(State(s): State<AppState>, Path(name): Path<String>) -> ApiResult<StatusCode> {
    let users = s.rt.persona_users(&name).await?;
    if !users.is_empty() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!(
                "persona {name} is the default of {}; pick another persona for them first",
                users.join(", ")
            ),
        ));
    }
    let (defaults, _) = s.gai.persona_defaults().await?;
    if defaults
        .persona
        .as_deref()
        .is_some_and(|p| p.split('@').next() == Some(name.as_str()))
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("persona {name} is the global default; change the default first"),
        ));
    }
    if s.rt.store().delete_persona(&name).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("persona {name}")))
    }
}

async fn diff(
    State(s): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<DiffQuery>,
) -> ApiResult<Json<VersionDiff>> {
    let store = s.rt.store();
    let latest = store
        .get_persona(&name, None)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("persona {name}")))?;
    let (from, to) = pick(&q, latest.version)?;
    let get = |v: u32| {
        let name = name.clone();
        async move {
            store
                .get_persona(&name, Some(v))
                .await?
                .ok_or_else(|| ApiError::not_found(format!("persona {name} v{v}")))
        }
    };
    let (a, b) = (get(from).await?, get(to).await?);
    Ok(Json(diff_of(name, (from, &a.spec), (to, &b.spec))))
}

async fn rollback(
    State(s): State<AppState>,
    Path(name): Path<String>,
    Json(b): Json<Rollback>,
) -> ApiResult<(StatusCode, Json<Saved>)> {
    let store = s.rt.store();
    let old = store
        .get_persona(&name, Some(b.version))
        .await?
        .ok_or_else(|| ApiError::not_found(format!("persona {name} v{}", b.version)))?;
    let note = b
        .note
        .unwrap_or_else(|| format!("rollback to v{}", b.version));
    let version = store.put_persona(&name, &old.spec, Some(&note)).await?;
    Ok((StatusCode::CREATED, Json(Saved { name, version })))
}

async fn set_label(
    State(s): State<AppState>,
    Path((name, label)): Path<(String, String)>,
    Json(b): Json<LabelBody>,
) -> ApiResult<Json<Done>> {
    s.rt.store()
        .set_persona_label(&name, &label, Some(b.version))
        .await?;
    Ok(Json(Done { ok: true }))
}

async fn remove_label(
    State(s): State<AppState>,
    Path((name, label)): Path<(String, String)>,
) -> ApiResult<Json<Done>> {
    s.rt.store().set_persona_label(&name, &label, None).await?;
    Ok(Json(Done { ok: true }))
}

async fn defaults(State(s): State<AppState>) -> ApiResult<Json<PersonaDefaultsView>> {
    let (defaults, source) = s.gai.persona_defaults().await?;
    Ok(Json(PersonaDefaultsView {
        defaults,
        source: source.into(),
    }))
}

async fn put_defaults(
    State(s): State<AppState>,
    Json(b): Json<PutPersonaDefaults>,
) -> ApiResult<Json<PersonaDefaultsView>> {
    let choice = (!b.reset).then(|| PersonaDefaults {
        persona: b
            .persona
            .map(|p| p.trim().to_owned())
            .filter(|p| !p.is_empty() && p != NO_PERSONA),
        placement: b.placement,
    });
    s.gai.set_persona_defaults(choice).await?;
    defaults(State(s)).await
}
