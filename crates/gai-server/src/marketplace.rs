//! The marketplace: bundles published as versioned, rated listings.
//!
//! Every gai server is a registry: `/v1/marketplace/listings` serves what
//! was published on it. Other registries (other gai servers, by URL and a
//! read key) are browsed and installed from through this one, so the
//! dashboard sees them all the same way. Installing fetches the listing's
//! bundle and imports it like any `.gaibundle`.

use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, Query};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use gai::bundle::{ExportOptions, ImportOptions, ImportReport, Manifest};
use gai::store::{LibraryItem, LibraryRating, NewBlob};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::AppState;
use crate::api::Done;
use crate::auth::Principal;
use crate::error::{ApiError, ApiResult};
use crate::orgs::Tenant;

/// The library kind listings are stored under.
const KIND: &str = "listing";
/// The setting holding other registries.
const REGISTRIES: &str = "marketplace.registries";
/// What this server's own listings are called among registries.
pub const LOCAL: &str = "local";
/// Largest bundle fetched from another registry.
const MAX_BUNDLE: usize = 200 << 20;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/marketplace/listings", get(list).post(publish))
        .route(
            "/v1/marketplace/listings/{name}",
            get(detail).delete(unpublish),
        )
        .route("/v1/marketplace/listings/{name}/bundle", get(bundle))
        .route("/v1/marketplace/listings/{name}/ratings", post(rate))
        .route(
            "/v1/marketplace/registries",
            get(registries).put(put_registries),
        )
        .route("/v1/marketplace/browse", get(browse))
        .route("/v1/marketplace/browse/{registry}/{name}", get(browse_one))
        .route(
            "/v1/marketplace/browse/{registry}/{name}/ratings",
            post(rate_in),
        )
        .route("/v1/marketplace/install", post(install))
        .layer(axum::extract::DefaultBodyLimit::max(4 << 20))
}

/// A listing as stored: what was published and where its bundle is.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ListingSpec {
    name: String,
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    author: Option<String>,
    blob_id: String,
    size: usize,
    manifest: Manifest,
}

/// A listing as people browse it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Listing {
    /// The registry it comes from (`local` for this server).
    #[serde(default)]
    pub registry: String,
    pub name: String,
    pub title: String,
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub author: Option<String>,
    /// The latest version.
    pub version: u32,
    /// Mean stars, 1 to 5.
    pub rating: Option<f64>,
    pub ratings: u32,
    /// What the latest version holds and needs.
    pub manifest: Manifest,
    /// Bytes of the latest bundle.
    pub size: usize,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListingVersion {
    pub version: u32,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListingDetail {
    pub listing: Listing,
    /// Newest first.
    pub versions: Vec<ListingVersion>,
    /// Newest first.
    pub reviews: Vec<LibraryRating>,
}

#[derive(Deserialize, JsonSchema)]
pub struct Publish {
    /// Letters, digits, `-`, `_` and `.`. Publishing to a name again makes
    /// a new version.
    pub name: String,
    pub title: String,
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// What changed in this version.
    pub note: Option<String>,
    /// What goes in the bundle, as for `/v1/bundles/export`.
    pub items: ExportOptions,
}

#[derive(Deserialize, JsonSchema)]
pub struct RateListing {
    /// 1 to 5.
    pub stars: u8,
    pub comment: Option<String>,
}

/// Another registry: a gai server whose listings this one can install.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct Registry {
    /// Letters, digits, `-` and `_`; not `local`.
    pub name: String,
    /// The server's address, e.g. `https://gai.example.com`.
    pub url: String,
    /// A key for it (read scope to browse and install, run to rate). Never
    /// shown again (views are `RegistryView`s); send it empty to keep the
    /// one saved.
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct RegistryView {
    pub name: String,
    pub url: String,
    pub has_token: bool,
}

#[derive(Deserialize)]
struct BrowseQuery {
    /// A registry's name; this server's listings without one.
    registry: Option<String>,
    /// Words to look for in names, titles, descriptions and tags.
    q: Option<String>,
}

#[derive(Deserialize)]
struct VersionQuery {
    version: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
pub struct Install {
    /// `local` (default) or a registry's name.
    #[serde(default)]
    pub registry: Option<String>,
    pub name: String,
    /// Defaults to the latest.
    pub version: Option<u32>,
    /// Leave items that already exist alone.
    #[serde(default)]
    pub skip_existing: bool,
    /// Only say what it holds and needs; install nothing.
    #[serde(default)]
    pub dry_run: bool,
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn listing(item: &LibraryItem, spec: ListingSpec) -> Listing {
    Listing {
        registry: LOCAL.into(),
        name: item.name.clone(),
        title: spec.title,
        description: spec.description,
        tags: spec.tags,
        author: spec.author,
        version: item.latest_version,
        rating: item.rating,
        ratings: item.ratings,
        manifest: spec.manifest,
        size: spec.size,
        updated_at: item.updated_at,
    }
}

fn spec_of(v: Value) -> ApiResult<ListingSpec> {
    serde_json::from_value(v).map_err(|e| ApiError::bad_request(format!("listing: {e}")))
}

async fn local_listings(s: &AppState) -> ApiResult<Vec<Listing>> {
    let store = s.rt.store();
    let mut out = Vec::new();
    for item in store.list_items(KIND).await? {
        if let Some(v) = store.get_item(KIND, &item.name, None).await? {
            out.push(listing(&item, spec_of(v.spec)?));
        }
    }
    Ok(out)
}

async fn local_detail(s: &AppState, name: &str) -> ApiResult<ListingDetail> {
    let store = s.rt.store();
    let not_found = || ApiError::not_found(format!("listing {name}"));
    let item = store.item(KIND, name).await?.ok_or_else(not_found)?;
    let latest = store
        .get_item(KIND, name, None)
        .await?
        .ok_or_else(not_found)?;
    Ok(ListingDetail {
        listing: listing(&item, spec_of(latest.spec)?),
        versions: store
            .list_item_versions(KIND, name)
            .await?
            .into_iter()
            .map(|v| ListingVersion {
                version: v.version,
                note: v.note,
                created_at: v.created_at,
            })
            .collect(),
        reviews: store.list_ratings(KIND, name).await?,
    })
}

async fn local_bundle(s: &AppState, name: &str, version: Option<u32>) -> ApiResult<Vec<u8>> {
    let store = s.rt.store();
    let v = store
        .get_item(KIND, name, version)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("listing {name}")))?;
    let spec = spec_of(v.spec)?;
    store
        .blob_data(&spec.blob_id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("the bundle of listing {name}")))
}

/// Who a rating is from: a user's email or a key's name.
fn rater(p: &Principal) -> String {
    p.name.clone()
}

fn matches(l: &Listing, q: &str) -> bool {
    let hay = format!(
        "{} {} {} {}",
        l.name,
        l.title,
        l.description.as_deref().unwrap_or_default(),
        l.tags.join(" ")
    )
    .to_lowercase();
    q.to_lowercase().split_whitespace().all(|w| hay.contains(w))
}

// ---- this server as a registry ----

async fn list(Tenant(s): Tenant) -> ApiResult<Json<Vec<Listing>>> {
    Ok(Json(local_listings(&s).await?))
}

async fn detail(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<Json<ListingDetail>> {
    Ok(Json(local_detail(&s, &name).await?))
}

async fn bundle(
    Tenant(s): Tenant,
    Path(name): Path<String>,
    Query(q): Query<VersionQuery>,
) -> ApiResult<Response> {
    let bytes = local_bundle(&s, &name, q.version).await?;
    let mut resp = bytes.into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{name}.gaibundle\"")) {
        resp.headers_mut().insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(resp)
}

/// Export a bundle from this server and publish it as the listing's next
/// version.
async fn publish(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Json(b): Json<Publish>,
) -> ApiResult<(StatusCode, Json<Listing>)> {
    if !valid_name(&b.name) {
        return Err(ApiError::bad_request(format!(
            "listing name {:?} may only use letters, digits, '-', '_' and '.'",
            b.name
        )));
    }
    if b.title.trim().is_empty() {
        return Err(ApiError::bad_request("give the listing a title"));
    }
    let mut items = b.items;
    items.name.get_or_insert_with(|| b.title.clone());
    if items.description.is_none() {
        items.description = b.description.clone();
    }
    let bytes = gai::bundle::export(&s.gai, &items).await?;
    let manifest = gai::bundle::inspect(&s.gai, &bytes).await?.manifest;
    let store = s.rt.store();
    let blob = store
        .put_blob(NewBlob {
            media_type: "application/zip".into(),
            name: Some(format!("{}.gaibundle", b.name)),
            run_id: None,
            source: Some("marketplace".into()),
            metadata: json!({"listing": b.name}),
            data: bytes.clone(),
        })
        .await?;
    let spec = ListingSpec {
        name: b.name.clone(),
        title: b.title.trim().to_owned(),
        description: b.description,
        tags: b.tags,
        author: Some(p.name.clone()),
        blob_id: blob.id,
        size: bytes.len(),
        manifest,
    };
    store
        .put_item(
            KIND,
            &b.name,
            &serde_json::to_value(&spec).expect("listing"),
            b.note.as_deref(),
        )
        .await?;
    let item = store
        .item(KIND, &b.name)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("listing {}", b.name)))?;
    Ok((StatusCode::CREATED, Json(listing(&item, spec))))
}

/// Remove a listing and its bundles.
async fn unpublish(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<StatusCode> {
    let store = s.rt.store();
    for v in store.list_item_versions(KIND, &name).await? {
        if let Ok(spec) = spec_of(v.spec) {
            store.delete_blob(&spec.blob_id).await?;
        }
    }
    if store.delete_item(KIND, &name).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("listing {name}")))
    }
}

async fn rate(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
    Json(b): Json<RateListing>,
) -> ApiResult<Json<Done>> {
    if !(1..=5).contains(&b.stars) {
        return Err(ApiError::bad_request("stars go from 1 to 5"));
    }
    s.rt.store()
        .rate_item(KIND, &name, &rater(&p), b.stars, b.comment.as_deref())
        .await?;
    Ok(Json(Done { ok: true }))
}

// ---- other registries ----

async fn saved_registries(s: &AppState) -> ApiResult<Vec<Registry>> {
    Ok(match s.rt.store().get_setting(REGISTRIES).await? {
        Some(v) => serde_json::from_str(&v).unwrap_or_default(),
        None => Vec::new(),
    })
}

async fn registries(Tenant(s): Tenant) -> ApiResult<Json<Vec<RegistryView>>> {
    Ok(Json(
        saved_registries(&s)
            .await?
            .into_iter()
            .map(|r| RegistryView {
                has_token: r.token.is_some(),
                name: r.name,
                url: r.url,
            })
            .collect(),
    ))
}

/// Replace the list of registries. A registry sent without a token keeps
/// the one saved for it.
async fn put_registries(
    Tenant(s): Tenant,
    Json(list): Json<Vec<Registry>>,
) -> ApiResult<Json<Vec<RegistryView>>> {
    let old = saved_registries(&s).await?;
    let mut out: Vec<Registry> = Vec::new();
    for mut r in list {
        r.name = r.name.trim().to_owned();
        r.url = r.url.trim().trim_end_matches('/').to_owned();
        if r.name == LOCAL
            || !r
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            || r.name.is_empty()
        {
            return Err(ApiError::bad_request(format!(
                "registry name {:?}: use letters, digits, '-' and '_' (and not {LOCAL})",
                r.name
            )));
        }
        if out.iter().any(|o| o.name == r.name) {
            return Err(ApiError::bad_request(format!(
                "registry {} is listed twice",
                r.name
            )));
        }
        let url = url::Url::parse(&r.url)
            .map_err(|e| ApiError::bad_request(format!("registry {}: {e}", r.name)))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ApiError::bad_request(format!(
                "registry {}: use an http(s) address",
                r.name
            )));
        }
        r.token = match r.token.map(|t| t.trim().to_owned()) {
            Some(t) if !t.is_empty() => Some(t),
            _ => old
                .iter()
                .find(|o| o.name == r.name)
                .and_then(|o| o.token.clone()),
        };
        out.push(r);
    }
    s.rt.store()
        .put_setting(REGISTRIES, &serde_json::to_string(&out).expect("json"))
        .await?;
    registries(Tenant(s)).await
}

async fn registry(s: &AppState, name: &str) -> ApiResult<Registry> {
    saved_registries(s)
        .await?
        .into_iter()
        .find(|r| r.name == name)
        .ok_or_else(|| ApiError::not_found(format!("registry {name}")))
}

/// A request to another registry, with its token.
fn remote(r: &Registry, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .expect("http client");
    let mut rb = client.request(method, format!("{}{path}", r.url));
    if let Some(t) = &r.token {
        rb = rb.bearer_auth(t);
    }
    rb
}

async fn remote_send(r: &Registry, rb: reqwest::RequestBuilder) -> ApiResult<reqwest::Response> {
    let resp = rb
        .send()
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, format!("registry {}: {e}", r.name)))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or_default();
        let msg = body["error"]["message"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| status.to_string());
        let code = if status == reqwest::StatusCode::NOT_FOUND {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::BAD_GATEWAY
        };
        return Err(ApiError::new(code, format!("registry {}: {msg}", r.name)));
    }
    Ok(resp)
}

async fn remote_json<T: serde::de::DeserializeOwned>(
    r: &Registry,
    rb: reqwest::RequestBuilder,
) -> ApiResult<T> {
    remote_send(r, rb)
        .await?
        .json()
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, format!("registry {}: {e}", r.name)))
}

fn path_name(name: &str) -> String {
    url::form_urlencoded::byte_serialize(name.as_bytes()).collect()
}

/// Listings of this server or another registry.
async fn browse(Tenant(s): Tenant, Query(q): Query<BrowseQuery>) -> ApiResult<Json<Vec<Listing>>> {
    let mut out = match q.registry.as_deref() {
        None | Some(LOCAL) | Some("") => local_listings(&s).await?,
        Some(name) => {
            let r = registry(&s, name).await?;
            let mut list: Vec<Listing> = remote_json(
                &r,
                remote(&r, reqwest::Method::GET, "/v1/marketplace/listings"),
            )
            .await?;
            for l in &mut list {
                l.registry = r.name.clone();
            }
            list
        }
    };
    if let Some(q) = q.q.as_deref().filter(|q| !q.trim().is_empty()) {
        out.retain(|l| matches(l, q));
    }
    Ok(Json(out))
}

async fn browse_one(
    Tenant(s): Tenant,
    Path((reg, name)): Path<(String, String)>,
) -> ApiResult<Json<ListingDetail>> {
    if reg == LOCAL {
        return Ok(Json(local_detail(&s, &name).await?));
    }
    let r = registry(&s, &reg).await?;
    let path = format!("/v1/marketplace/listings/{}", path_name(&name));
    let mut d: ListingDetail = remote_json(&r, remote(&r, reqwest::Method::GET, &path)).await?;
    d.listing.registry = r.name;
    Ok(Json(d))
}

/// RateListing a listing here or, with this server's key there, in another
/// registry.
async fn rate_in(
    Tenant(s): Tenant,
    p: Extension<Principal>,
    Path((reg, name)): Path<(String, String)>,
    Json(b): Json<RateListing>,
) -> ApiResult<Json<Done>> {
    if reg == LOCAL {
        return rate(Tenant(s), p, Path(name), Json(b)).await;
    }
    let r = registry(&s, &reg).await?;
    let path = format!("/v1/marketplace/listings/{}/ratings", path_name(&name));
    remote_send(
        &r,
        remote(&r, reqwest::Method::POST, &path)
            .json(&json!({"stars": b.stars, "comment": b.comment})),
    )
    .await?;
    Ok(Json(Done { ok: true }))
}

async fn fetch_bundle(s: &AppState, b: &Install) -> ApiResult<Bytes> {
    match b.registry.as_deref() {
        None | Some(LOCAL) | Some("") => Ok(local_bundle(s, &b.name, b.version).await?.into()),
        Some(reg) => {
            let r = registry(s, reg).await?;
            let mut path = format!("/v1/marketplace/listings/{}/bundle", path_name(&b.name));
            if let Some(v) = b.version {
                path.push_str(&format!("?version={v}"));
            }
            let resp = remote_send(&r, remote(&r, reqwest::Method::GET, &path)).await?;
            if resp
                .content_length()
                .is_some_and(|n| n as usize > MAX_BUNDLE)
            {
                return Err(ApiError::bad_request("the bundle is too large"));
            }
            let bytes = resp.bytes().await.map_err(|e| {
                ApiError::new(StatusCode::BAD_GATEWAY, format!("registry {reg}: {e}"))
            })?;
            if bytes.len() > MAX_BUNDLE {
                return Err(ApiError::bad_request("the bundle is too large"));
            }
            Ok(bytes)
        }
    }
}

/// Install a listing (or, with `dry_run`, see what it holds and needs).
async fn install(Tenant(s): Tenant, Json(b): Json<Install>) -> ApiResult<Json<ImportReport>> {
    let bytes = fetch_bundle(&s, &b).await?;
    let report = if b.dry_run {
        gai::bundle::inspect(&s.gai, &bytes).await?
    } else {
        gai::bundle::import(
            &s.gai,
            &bytes,
            &ImportOptions {
                skip_existing: b.skip_existing,
            },
        )
        .await?
    };
    Ok(Json(report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_matches_every_word() {
        let l = Listing {
            registry: LOCAL.into(),
            name: "support-kit".into(),
            title: "Support kit".into(),
            description: Some("Answers refund questions".into()),
            tags: vec!["customer".into()],
            author: None,
            version: 1,
            rating: None,
            ratings: 0,
            manifest: Manifest::default(),
            size: 0,
            updated_at: Utc::now(),
        };
        assert!(matches(&l, "refund customer"));
        assert!(matches(&l, "KIT"));
        assert!(!matches(&l, "refund sales"));
    }
}
