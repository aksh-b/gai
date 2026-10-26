//! Recordings: a meeting, call or video uploaded as a blob, processed in
//! the background into a transcript with speakers and notes or a summary.

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Extension, Json, Router};
use gai::media::{NotesTemplate, RecordingOptions};
use gai::store::Recording;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::auth::{Principal, charge};
use crate::error::{ApiError, ApiResult};
use crate::orgs::Tenant;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/recordings", get(list).post(create))
        .route("/v1/recordings/templates", get(templates))
        .route("/v1/recordings/{id}", get(get_one).delete(delete))
}

#[derive(Deserialize, JsonSchema)]
pub struct NewRecordingBody {
    /// The uploaded media (`POST /v1/blobs`).
    pub blob_id: String,
    /// `meeting` (default): speakers, notes, decisions and action items;
    /// `audio`: speakers, language and sentiment; `video`: frames and
    /// speech summarized in chapters.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub options: RecordingOptions,
}

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<u32>,
}

#[derive(Serialize, JsonSchema)]
pub struct NotesTemplateInfo {
    pub name: String,
    pub instructions: Option<String>,
    pub sections: Vec<String>,
}

/// Newest first, without their (large) results.
async fn list(Tenant(s): Tenant, Query(q): Query<ListQuery>) -> ApiResult<Json<Vec<Recording>>> {
    let mut list = s
        .gai
        .recordings()
        .await?
        .list(q.limit.unwrap_or(50).min(500))
        .await?;
    for r in &mut list {
        // Keep the title-level facts a list shows.
        r.result = r.result.take().map(|v| {
            serde_json::json!({
                "duration_secs": v["transcript"]["duration_secs"],
                "summary": v["notes"]["summary"].as_str().or(v["video"]["summary"].as_str()),
                "cost": v["cost"],
            })
        });
    }
    Ok(Json(list))
}

async fn create(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Json(b): Json<NewRecordingBody>,
) -> ApiResult<(StatusCode, Json<Recording>)> {
    p.check_budget()?;
    let recordings = s.gai.recordings().await?;
    let kind = b.kind.as_deref().unwrap_or("meeting");
    let title = match b.title.filter(|t| !t.trim().is_empty()) {
        Some(t) => t,
        None => {
            s.rt.store()
                .get_blob(&b.blob_id)
                .await?
                .and_then(|blob| blob.name)
                .unwrap_or_else(|| "Recording".into())
        }
    };
    let rec = recordings
        .create(kind, &title, &b.blob_id, b.options)
        .await?;
    let (id, store, key) = (
        rec.id.clone(),
        s.rt.store().clone(),
        p.key_id().map(str::to_owned),
    );
    tokio::spawn(async move {
        if let Some(cost) = recordings.process(&id).await {
            charge(&store, key.as_deref(), cost).await;
        }
    });
    Ok((StatusCode::ACCEPTED, Json(rec)))
}

async fn get_one(Tenant(s): Tenant, Path(id): Path<String>) -> ApiResult<Json<Recording>> {
    s.gai
        .recordings()
        .await?
        .get(&id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("recording {id}")))
}

async fn delete(Tenant(s): Tenant, Path(id): Path<String>) -> ApiResult<StatusCode> {
    if s.gai.recordings().await?.delete(&id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("recording {id}")))
    }
}

/// The built-in meeting notes templates.
async fn templates() -> Json<Vec<NotesTemplateInfo>> {
    Json(
        NotesTemplate::BUILTIN
            .iter()
            .filter_map(|n| NotesTemplate::builtin(n))
            .map(|t| NotesTemplateInfo {
                name: t.name,
                instructions: t.instructions,
                sections: t.sections.into_keys().collect(),
            })
            .collect(),
    )
}

/// Process again recordings a restart left half done.
pub fn recover(gai: std::sync::Arc<gai::Gai>) {
    tokio::spawn(async move {
        match gai.recordings().await {
            Ok(r) => r.recover().await,
            Err(e) => tracing::warn!(error = %e, "could not look for unfinished recordings"),
        }
    });
}
