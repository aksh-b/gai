//! Media: stored blobs, speech to text, text to speech, image generation.

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use base64::Engine;
use gai::core::Usage;
use gai::llm::{ImageRequest, SpeechRequest, TranscribeRequest};
use gai::store::{Blob, NewBlob};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AppState;
use crate::auth::{Principal, charge};
use crate::error::{ApiError, ApiResult};
use crate::orgs::Tenant;

/// Uploads and generated media can be large; the default limit is 2 MB.
const MAX_MEDIA_BYTES: usize = 50 * 1024 * 1024;
/// Uploads include meeting recordings and videos.
const MAX_UPLOAD_BYTES: usize = 500 * 1024 * 1024;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/blobs",
            get(list_blobs)
                .post(upload_blob)
                .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .route("/v1/blobs/{id}", get(blob_bytes).delete(delete_blob))
        .route("/v1/blobs/{id}/info", get(blob_info))
        .route("/v1/media", get(defaults))
        .route("/v1/audio/transcriptions", post(transcribe))
        .route("/v1/audio/speech", post(speak))
        .route("/v1/images/generations", post(generate_images))
        .layer(DefaultBodyLimit::max(MAX_MEDIA_BYTES))
}

#[derive(Serialize, JsonSchema)]
pub struct MediaDefaults {
    pub transcription: Option<String>,
    pub speech: Option<String>,
    pub voice: Option<String>,
    pub image: Option<String>,
}

#[derive(Deserialize)]
struct BlobQuery {
    run_id: Option<String>,
    limit: Option<u32>,
}

#[derive(Deserialize)]
struct UploadQuery {
    name: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct TranscribeBody {
    /// Speech-to-text model; defaults to `[media] transcription`.
    pub model: Option<String>,
    /// A stored blob to transcribe...
    pub blob_id: Option<String>,
    /// ...or the audio itself, base64-encoded, with its media type.
    pub audio_base64: Option<String>,
    pub media_type: Option<String>,
    pub language: Option<String>,
    pub prompt: Option<String>,
    /// Label who says what, from models that can.
    #[serde(default)]
    pub speakers: bool,
}

#[derive(Serialize, JsonSchema)]
pub struct TranscribeResult {
    pub text: String,
    pub language: Option<String>,
    pub duration_secs: Option<f64>,
    /// Timed stretches, with speakers where known.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<gai::llm::Segment>,
    pub model: String,
    pub cost: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SpeakBody {
    /// Text-to-speech model; defaults to `[media] speech`.
    pub model: Option<String>,
    #[serde(flatten)]
    pub speech: SpeechRequest,
}

#[derive(Deserialize, JsonSchema)]
pub struct ImagesBody {
    /// Image model; defaults to `[media] image`.
    pub model: Option<String>,
    #[serde(flatten)]
    pub image: ImageRequest,
}

#[derive(Serialize, JsonSchema)]
pub struct MediaResult {
    /// The stored audio or images; fetch bytes from `/v1/blobs/{id}`.
    pub blobs: Vec<Blob>,
    pub model: String,
    pub cost: Option<f64>,
    /// For images: the prompts providers actually used, where they say.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub revised_prompts: Vec<String>,
}

async fn defaults(Tenant(s): Tenant) -> Json<MediaDefaults> {
    let m = &s.rt.config().media;
    Json(MediaDefaults {
        transcription: m.transcription.clone(),
        speech: m.speech.clone(),
        voice: m.voice.clone(),
        image: m.image.clone(),
    })
}

async fn list_blobs(Tenant(s): Tenant, Query(q): Query<BlobQuery>) -> ApiResult<Json<Vec<Blob>>> {
    Ok(Json(
        s.rt.store()
            .list_blobs(q.run_id.as_deref(), q.limit.unwrap_or(100).min(1000))
            .await?,
    ))
}

/// Store the request body as a blob; its `content-type` is the media type.
async fn upload_blob(
    Tenant(s): Tenant,
    Query(q): Query<UploadQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<Blob>)> {
    if body.is_empty() {
        return Err(ApiError::bad_request("the body is empty"));
    }
    let media_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_owned();
    let blob =
        s.rt.store()
            .put_blob(NewBlob {
                media_type,
                name: q.name,
                source: Some("upload".into()),
                data: body.into(),
                ..Default::default()
            })
            .await?;
    Ok((StatusCode::CREATED, Json(blob)))
}

async fn blob_info(Tenant(s): Tenant, Path(id): Path<String>) -> ApiResult<Json<Blob>> {
    s.rt.store()
        .get_blob(&id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("blob {id}")))
}

/// The bytes. Served so a stored SVG or HTML file cannot run script on the
/// dashboard's origin.
async fn blob_bytes(Tenant(s): Tenant, Path(id): Path<String>) -> ApiResult<Response> {
    let store = s.rt.store();
    let b = store
        .get_blob(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("blob {id}")))?;
    let data = store.blob_data(&id).await?.unwrap_or_default();
    let name = b
        .name
        .unwrap_or_else(|| format!("{id}.{}", gai::llm::extension_for(&b.media_type)));
    let disposition = format!("inline; filename=\"{}\"", name.replace('"', ""));
    Ok((
        [
            (header::CONTENT_TYPE, b.media_type),
            (header::CONTENT_DISPOSITION, disposition),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".into()),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; style-src 'unsafe-inline'; sandbox".into(),
            ),
            (
                header::CACHE_CONTROL,
                "private, max-age=31536000, immutable".into(),
            ),
        ],
        data,
    )
        .into_response())
}

async fn delete_blob(Tenant(s): Tenant, Path(id): Path<String>) -> ApiResult<StatusCode> {
    if s.rt.store().delete_blob(&id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("blob {id}")))
    }
}

fn pick(given: Option<String>, default: &Option<String>, what: &str) -> ApiResult<String> {
    given.or_else(|| default.clone()).ok_or_else(|| {
        ApiError::bad_request(format!(
            "no {what} model given, and none is set under [media] in gai.toml"
        ))
    })
}

/// Log a media call's cost and charge it to the calling key.
async fn account(s: &AppState, p: &Principal, model: &str, cost: Option<f64>) {
    let store = s.rt.store();
    if let Err(e) = store
        .record_usage(None, model, &Usage::default(), cost)
        .await
    {
        tracing::warn!(error = %e, "could not record media usage");
    }
    charge(store, p.key_id(), cost.unwrap_or(0.0)).await;
}

async fn transcribe(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Json(b): Json<TranscribeBody>,
) -> ApiResult<Json<TranscribeResult>> {
    p.check_budget()?;
    let model = pick(b.model, &s.rt.config().media.transcription, "transcription")?;
    let m = s.gai.models().transcription(&model)?;
    let (audio, media_type, filename) = match (b.blob_id, b.audio_base64) {
        (Some(id), _) => {
            let store = s.rt.store();
            let blob = store
                .get_blob(&id)
                .await?
                .ok_or_else(|| ApiError::not_found(format!("blob {id}")))?;
            let data = store.blob_data(&id).await?.unwrap_or_default();
            (data, blob.media_type, blob.name)
        }
        (None, Some(b64)) => {
            let data = base64::engine::general_purpose::STANDARD
                .decode(b64.trim())
                .map_err(|e| ApiError::bad_request(format!("audio_base64: {e}")))?;
            let media_type = b
                .media_type
                .ok_or_else(|| ApiError::bad_request("give the audio's media_type"))?;
            (data, media_type, None)
        }
        (None, None) => return Err(ApiError::bad_request("give blob_id or audio_base64")),
    };
    let t = m
        .transcribe(TranscribeRequest {
            audio,
            media_type,
            filename,
            language: b.language,
            prompt: b.prompt,
            diarize: b.speakers,
        })
        .await?;
    let cost = m.info().media_cost(&t.usage);
    account(&s, &p, &m.info().id, cost).await;
    Ok(Json(TranscribeResult {
        text: t.text,
        language: t.language,
        duration_secs: t.duration_secs,
        segments: t.segments,
        model: m.info().id.clone(),
        cost,
    }))
}

async fn speak(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Json(b): Json<SpeakBody>,
) -> ApiResult<Json<MediaResult>> {
    p.check_budget()?;
    let model = pick(b.model, &s.rt.config().media.speech, "speech")?;
    let m = s.gai.models().speech(&model)?;
    let mut req = b.speech;
    if req.text.trim().is_empty() {
        return Err(ApiError::bad_request("text is empty"));
    }
    if req.voice.is_none() {
        req.voice = s.rt.config().media.voice.clone();
    }
    let text = req.text.clone();
    let out = m.speak(req).await?;
    let cost = m.info().media_cost(&out.usage);
    account(&s, &p, &m.info().id, cost).await;
    let blob =
        s.rt.store()
            .put_blob(NewBlob {
                media_type: out.media_type,
                source: Some("speech".into()),
                metadata: json!({"text": text, "model": m.info().id}),
                data: out.audio,
                ..Default::default()
            })
            .await?;
    Ok(Json(MediaResult {
        blobs: vec![blob],
        model: m.info().id.clone(),
        cost,
        revised_prompts: Vec::new(),
    }))
}

async fn generate_images(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Json(b): Json<ImagesBody>,
) -> ApiResult<Json<MediaResult>> {
    p.check_budget()?;
    let model = pick(b.model, &s.rt.config().media.image, "image")?;
    let m = s.gai.models().image(&model)?;
    let mut req = b.image;
    if req.prompt.trim().is_empty() {
        return Err(ApiError::bad_request("prompt is empty"));
    }
    req.n = Some(req.n.unwrap_or(1).clamp(1, 4));
    let prompt = req.prompt.clone();
    let out = m.generate(req).await?;
    let cost = m.info().media_cost(&out.usage);
    account(&s, &p, &m.info().id, cost).await;
    let mut blobs = Vec::new();
    let mut revised = Vec::new();
    for img in out.images {
        if let Some(r) = &img.revised_prompt {
            revised.push(r.clone());
        }
        blobs.push(
            s.rt.store()
                .put_blob(NewBlob {
                    media_type: img.media_type,
                    source: Some("images".into()),
                    metadata: json!({
                        "prompt": prompt,
                        "model": m.info().id,
                        "revised_prompt": img.revised_prompt,
                    }),
                    data: img.data,
                    ..Default::default()
                })
                .await?,
        );
    }
    Ok(Json(MediaResult {
        blobs,
        model: m.info().id.clone(),
        cost,
        revised_prompts: revised,
    }))
}
