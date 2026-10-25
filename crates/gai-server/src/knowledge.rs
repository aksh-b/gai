//! Knowledge collections, memory blocks and skills.

use std::collections::BTreeMap;

use axum::extract::{DefaultBodyLimit, Path, Query};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use base64::Engine;
use gai::memory::{Ingested, NewCollection, SearchHit, SearchMode};
use gai::skills::{Skill, SkillSource};
use gai::store::{
    BlockRevision, BlockSettings, Chunk, Collection, Document, MemoryBlock, NewBlock,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::AppState;
use crate::auth::Principal;
use crate::error::{ApiError, ApiResult};
use crate::orgs::Tenant;

/// Largest request body for document uploads.
const UPLOAD_LIMIT: usize = 64 * 1024 * 1024;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/collections",
            get(list_collections).post(create_collection),
        )
        .route(
            "/v1/collections/{name}",
            get(get_collection).delete(delete_collection),
        )
        .route(
            "/v1/collections/{name}/documents",
            get(list_documents)
                .post(add_document)
                .layer(DefaultBodyLimit::max(UPLOAD_LIMIT)),
        )
        .route(
            "/v1/collections/{name}/documents/{id}",
            get(get_document).delete(delete_document),
        )
        .route("/v1/collections/{name}/search", post(search))
        .route("/v1/collections/{name}/extract", post(extract))
        .route(
            "/v1/collections/{name}/graph",
            get(graph_view).post(graph_build).delete(graph_clear),
        )
        .route("/v1/collections/{name}/graph/nodes/{id}", get(graph_node))
        .route("/v1/collections/{name}/reindex", post(reindex))
        .route("/v1/memory/blocks", get(list_blocks).post(create_block))
        .route(
            "/v1/memory/blocks/{id}",
            get(get_block).patch(edit_block).delete(delete_block),
        )
        .route("/v1/memory/blocks/{id}/history", get(block_history))
        .route("/v1/skills", get(list_skills).post(save_skill))
        .route("/v1/skills/{name}", get(get_skill).delete(delete_skill))
}

// ---- shapes ----

#[derive(Deserialize, JsonSchema)]
pub struct AddDocument {
    /// Identifies the document in the collection; adding the same source
    /// again replaces it. Defaults to the file name or URL.
    pub source: Option<String>,
    pub title: Option<String>,
    /// Plain text or Markdown to add.
    pub text: Option<String>,
    /// A file, base64-encoded (text, Markdown, HTML or PDF). Needs `filename`.
    pub content_base64: Option<String>,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    /// A web page or file to fetch and add.
    pub url: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Serialize, JsonSchema)]
pub struct DocumentView {
    pub document: Document,
    pub chunks: Vec<Chunk>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchRequest {
    pub query: String,
    /// Results to return (default 8).
    pub k: Option<usize>,
    #[serde(default)]
    pub mode: SearchMode,
}

#[derive(Deserialize, JsonSchema)]
pub struct EditBlock {
    /// New contents (checked against the limit).
    pub value: Option<String>,
    pub description: Option<String>,
    pub limit: Option<u32>,
    pub read_only: Option<bool>,
}

#[derive(Serialize, JsonSchema)]
pub struct SkillView {
    pub name: String,
    pub description: String,
    pub source: SkillSource,
    pub files: Vec<String>,
    pub metadata: Map<String, Value>,
}

#[derive(Serialize, JsonSchema)]
pub struct SkillDetail {
    #[serde(flatten)]
    pub view: SkillView,
    pub instructions: String,
    /// The whole SKILL.md.
    pub raw: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct SaveSkill {
    /// The SKILL.md, front matter included.
    pub content: String,
    /// Bundled text files by relative path.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

#[derive(Serialize, JsonSchema)]
pub struct SavedSkill {
    pub name: String,
    pub version: u32,
}

fn view(s: &Skill) -> SkillView {
    SkillView {
        name: s.name.clone(),
        description: s.description.clone(),
        source: s.source.clone(),
        files: s.files.clone(),
        metadata: s.metadata.clone(),
    }
}

// ---- collections ----

#[derive(Deserialize)]
struct CollectionsQuery {
    /// Include archival-memory collections (`memory:*`).
    #[serde(default)]
    all: bool,
}

async fn list_collections(
    Tenant(s): Tenant,
    Query(q): Query<CollectionsQuery>,
) -> ApiResult<Json<Vec<Collection>>> {
    let all = s.rt.store().list_collections().await?;
    Ok(Json(
        all.into_iter()
            .filter(|c| q.all || !c.name.contains(':'))
            .collect(),
    ))
}

async fn create_collection(
    Tenant(s): Tenant,
    Json(new): Json<NewCollection>,
) -> ApiResult<(StatusCode, Json<Collection>)> {
    let c = s.rt.knowledge().create_collection(new).await?;
    Ok((StatusCode::CREATED, Json(c)))
}

async fn get_collection(
    Tenant(s): Tenant,
    Path(name): Path<String>,
) -> ApiResult<Json<Collection>> {
    Ok(Json(s.rt.knowledge().collection(&name).await?))
}

#[derive(Serialize, JsonSchema)]
pub struct Reindexed {
    /// Where vectors are searched: builtin, qdrant or pgvector.
    pub backend: String,
    pub vectors: u64,
}

/// Copy a collection's stored vectors into the vector index.
async fn reindex(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<Json<Reindexed>> {
    let k = s.rt.knowledge();
    let vectors = k.reindex(&name).await?;
    Ok(Json(Reindexed {
        backend: k.index_backend().to_owned(),
        vectors,
    }))
}

async fn delete_collection(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<StatusCode> {
    if s.rt.knowledge().delete_collection(&name).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("collection {name}")))
    }
}

#[derive(Deserialize)]
struct PageQuery {
    limit: Option<u32>,
    offset: Option<u32>,
}

async fn list_documents(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
    Query(q): Query<PageQuery>,
) -> ApiResult<Json<Vec<Document>>> {
    s.rt.knowledge().collection(&name).await?;
    let mut docs =
        s.rt.store()
            .list_documents(
                &name,
                q.limit.unwrap_or(100).min(1000),
                q.offset.unwrap_or(0),
            )
            .await?;
    if let Some(a) = &p.access {
        docs.retain(|d| a.allows(&d.metadata["acl"]));
    }
    Ok(Json(docs))
}

/// Fetch a URL for ingestion, refusing private addresses unless tools may
/// reach them.
async fn fetch(s: &AppState, url: &str) -> ApiResult<(String, Option<String>, Vec<u8>)> {
    let u = reqwest::Url::parse(url).map_err(|e| ApiError::bad_request(format!("url: {e}")))?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err(ApiError::bad_request("url must be http(s)"));
    }
    let host = u.host_str().unwrap_or_default().to_owned();
    let port = u.port_or_known_default().unwrap_or(80);
    let private =
        tokio::task::spawn_blocking(move || gai::tools::builtin::is_private_host(&host, port))
            .await
            .unwrap_or(true);
    if private && !s.gai.config().tools.allow_private_network {
        return Err(ApiError::bad_request(
            "url points at a private network address (set tools.allow_private_network to allow it)",
        ));
    }
    let resp = reqwest::Client::new()
        .get(u.clone())
        .header("user-agent", concat!("gai/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, format!("fetching {url}: {e}")))?;
    if !resp.status().is_success() {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            format!("fetching {url}: HTTP {}", resp.status()),
        ));
    }
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, e.to_string()))?;
    let name = u
        .path_segments()
        .and_then(|mut p| p.next_back().map(str::to_owned))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "index.html".into());
    Ok((name, ct, bytes.to_vec()))
}

async fn add_document(
    Tenant(s): Tenant,
    Path(name): Path<String>,
    Json(b): Json<AddDocument>,
) -> ApiResult<(StatusCode, Json<Ingested>)> {
    let k = s.rt.knowledge();
    let given = [
        b.text.is_some(),
        b.content_base64.is_some(),
        b.url.is_some(),
    ];
    if given.iter().filter(|x| **x).count() != 1 {
        return Err(ApiError::bad_request(
            "give exactly one of text, content_base64 (with filename) or url",
        ));
    }
    let ingested = if let Some(text) = b.text {
        let source = b.source.unwrap_or_else(|| gai::core::new_id("text"));
        k.ingest(
            &name,
            gai::memory::DocumentInput {
                source,
                title: b.title,
                text,
                metadata: b.metadata,
            },
        )
        .await?
    } else if let Some(data) = b.content_base64 {
        let filename = b
            .filename
            .ok_or_else(|| ApiError::bad_request("content_base64 needs a filename"))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data.trim())
            .map_err(|e| ApiError::bad_request(format!("content_base64: {e}")))?;
        k.ingest_bytes(
            &name,
            &filename,
            b.content_type.as_deref(),
            bytes,
            b.source,
            b.metadata,
        )
        .await?
    } else {
        let url = b.url.expect("checked above");
        k.collection(&name).await?;
        let (file, ct, bytes) = fetch(&s, &url).await?;
        k.ingest_bytes(
            &name,
            &file,
            ct.as_deref(),
            bytes,
            Some(b.source.unwrap_or(url)),
            b.metadata,
        )
        .await?
    };
    let status = if ingested.unchanged {
        StatusCode::OK
    } else {
        gai::memory::graph::build_in_background(s.rt.knowledge().clone(), name.clone());
        StatusCode::CREATED
    };
    Ok((status, Json(ingested)))
}

async fn get_document(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Path((name, id)): Path<(String, String)>,
) -> ApiResult<Json<DocumentView>> {
    let document =
        s.rt.store()
            .get_document(&id)
            .await?
            .filter(|d| d.collection == name)
            .filter(|d| {
                p.access
                    .as_ref()
                    .is_none_or(|a| a.allows(&d.metadata["acl"]))
            })
            .ok_or_else(|| ApiError::not_found(format!("document {id} in {name}")))?;
    let chunks = s.rt.store().document_chunks(&id).await?;
    Ok(Json(DocumentView { document, chunks }))
}

async fn delete_document(
    Tenant(s): Tenant,
    Path((name, id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let found =
        s.rt.store()
            .get_document(&id)
            .await?
            .is_some_and(|d| d.collection == name);
    if found && s.rt.knowledge().delete_document(&id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("document {id} in {name}")))
    }
}

/// A collection's graph, for a picture: its most mentioned entities and
/// the relations among them.
#[derive(Serialize, JsonSchema)]
pub struct GraphView {
    pub stats: gai::store::GraphStats,
    pub nodes: Vec<gai::store::GraphNode>,
    pub edges: Vec<gai::store::GraphEdge>,
}

#[derive(Deserialize)]
struct GraphQuery {
    /// Entities whose name contains this.
    q: Option<String>,
    limit: Option<u32>,
}

/// The graph of a collection that has documents some may not find is for
/// those who may find everything: entity names would give them away.
async fn graph_allowed(s: &AppState, p: &Principal, name: &str) -> ApiResult<()> {
    if p.access.is_none() {
        return Ok(());
    }
    let restricted =
        s.rt.store()
            .list_documents(name, 100_000, 0)
            .await?
            .iter()
            .any(|d| d.metadata["acl"].as_array().is_some_and(|a| !a.is_empty()));
    if restricted {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "this collection has documents only some may find, so its graph is for admins",
        ));
    }
    Ok(())
}

async fn graph_view(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
    Query(q): Query<GraphQuery>,
) -> ApiResult<Json<GraphView>> {
    s.rt.knowledge().collection(&name).await?;
    graph_allowed(&s, &p, &name).await?;
    let store = s.rt.store();
    let nodes = store
        .graph_nodes(
            &name,
            q.q.as_deref().unwrap_or(""),
            q.limit.unwrap_or(80).min(500),
        )
        .await?;
    let ids: Vec<i64> = nodes.iter().map(|n| n.id).collect();
    let set: std::collections::HashSet<i64> = ids.iter().copied().collect();
    let edges = if ids.is_empty() {
        Vec::new()
    } else {
        store
            .graph_edges(&name, &ids, 2000)
            .await?
            .into_iter()
            .filter(|e| set.contains(&e.source) && set.contains(&e.target))
            .collect()
    };
    Ok(Json(GraphView {
        stats: store.graph_stats(&name).await?,
        nodes,
        edges,
    }))
}

#[derive(Deserialize, JsonSchema)]
pub struct BuildGraph {
    /// The model that reads the chunks (default: `[knowledge.graph] model`,
    /// else the default model).
    pub model: Option<String>,
}

#[derive(Deserialize)]
struct WaitQuery {
    #[serde(default)]
    wait: bool,
}

/// Read the collection's unread chunks into its graph: in the background
/// (202), or with `?wait=true` (200 and what was found).
async fn graph_build(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
    Query(w): Query<WaitQuery>,
    body: Option<Json<BuildGraph>>,
) -> ApiResult<(StatusCode, Json<Option<gai::memory::graph::GraphBuild>>)> {
    p.check_budget()?;
    let k = s.rt.knowledge().clone();
    k.collection(&name).await?;
    let model = body
        .and_then(|b| b.0.model)
        .or_else(|| k.settings().graph.model.clone())
        .or_else(|| s.rt.default_model())
        .ok_or_else(|| ApiError::bad_request("no model: pass one, or set a default model"))?;
    if w.wait {
        let b = k.build_graph(&name, &model).await?;
        crate::auth::charge(s.orgs.control(), p.key_id(), b.cost_usd).await;
        return Ok((StatusCode::OK, Json(Some(b))));
    }
    let (control, key) = (s.orgs.control().clone(), p.key_id().map(str::to_owned));
    tokio::spawn(async move {
        match k.build_graph(&name, &model).await {
            Ok(b) => crate::auth::charge(&control, key.as_deref(), b.cost_usd).await,
            Err(e) => {
                tracing::warn!(collection = name, error = %e, "could not build the knowledge graph")
            }
        }
    });
    Ok((StatusCode::ACCEPTED, Json(None)))
}

async fn graph_clear(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<StatusCode> {
    s.rt.knowledge().collection(&name).await?;
    s.rt.store().graph_clear(&name).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// One entity: its relations, and the passages that mention it.
#[derive(Serialize, JsonSchema)]
pub struct GraphNodeView {
    pub node: gai::store::GraphNode,
    pub relations: Vec<GraphRelationView>,
    pub passages: Vec<SearchHit>,
}

#[derive(Serialize, JsonSchema)]
pub struct GraphRelationView {
    pub relation: String,
    /// `out` when this entity is the subject.
    pub direction: String,
    pub other: gai::store::GraphNode,
    pub weight: u64,
}

async fn graph_node(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Path((name, id)): Path<(String, i64)>,
) -> ApiResult<Json<GraphNodeView>> {
    s.rt.knowledge().collection(&name).await?;
    graph_allowed(&s, &p, &name).await?;
    let store = s.rt.store();
    let node = store
        .graph_nodes_by_id(&[id])
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::not_found(format!("entity {id}")))?;
    let edges = store.graph_edges(&name, &[id], 200).await?;
    let others: Vec<i64> = edges
        .iter()
        .map(|e| if e.source == id { e.target } else { e.source })
        .collect();
    let others = store.graph_nodes_by_id(&others).await?;
    let relations = edges
        .iter()
        .filter_map(|e| {
            let (direction, other) = if e.source == id {
                ("out", e.target)
            } else {
                ("in", e.source)
            };
            Some(GraphRelationView {
                relation: e.relation.clone(),
                direction: direction.into(),
                other: others.iter().find(|n| n.id == other)?.clone(),
                weight: e.weight,
            })
        })
        .collect();
    let chunks = store.graph_chunks(&[id], 10).await?;
    let passages = store
        .chunk_hits(chunks.iter().map(|(c, n)| (*c, *n as f32)).collect())
        .await?
        .into_iter()
        .map(|h| SearchHit {
            heading: h.chunk.metadata["heading"].as_str().map(str::to_owned),
            collection: h.chunk.collection,
            document_id: h.chunk.document_id,
            chunk_id: h.chunk.id,
            seq: h.chunk.seq,
            source: h.source,
            title: h.title,
            text: h.chunk.text,
            score: h.score,
            vector_score: None,
            text_score: None,
            rerank_score: None,
        })
        .collect();
    Ok(Json(GraphNodeView {
        node,
        relations,
        passages,
    }))
}

#[derive(Deserialize, JsonSchema)]
pub struct ExtractRequest {
    /// `{"field": "what it is"}`, or a JSON Schema with `properties`.
    pub fields: Value,
    /// Only these documents (ids, sources or titles); default: every
    /// document, up to 500.
    #[serde(default)]
    pub documents: Vec<String>,
    /// The model that reads them (default: the default model).
    pub model: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct Extracted {
    /// The fields, as a JSON Schema.
    pub schema: Value,
    pub rows: Vec<gai::memory::docs::ExtractedRow>,
    pub cost_usd: f64,
}

#[derive(Deserialize)]
struct ExtractQuery {
    /// `csv` for a download.
    format: Option<String>,
}

/// Read the same fields out of a collection's documents into a table.
async fn extract(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
    Query(q): Query<ExtractQuery>,
    Json(b): Json<ExtractRequest>,
) -> ApiResult<axum::response::Response> {
    use axum::response::IntoResponse;
    p.check_budget()?;
    let k = s.rt.knowledge();
    k.collection(&name).await?;
    let schema = gai::memory::docs::fields_schema(&b.fields)?;
    let model = b
        .model
        .or_else(|| s.rt.default_model())
        .ok_or_else(|| ApiError::bad_request("no model: pass one, or set a default model"))?;
    let docs = if b.documents.is_empty() {
        s.rt.store().list_documents(&name, 500, 0).await?
    } else {
        let mut v = Vec::new();
        for d in &b.documents {
            v.push(k.find_document(&name, d).await?);
        }
        v
    };
    let mut spent = gai::memory::docs::Spent::default();
    let rows = gai::memory::docs::extract(
        k,
        s.gai.models(),
        &model,
        docs,
        &schema,
        p.access.as_ref(),
        &mut spent,
    )
    .await;
    if let Err(e) =
        s.rt.store()
            .record_usage(None, &model, &spent.usage, Some(spent.cost))
            .await
    {
        tracing::warn!(error = %e, "could not record extraction usage");
    }
    crate::auth::charge(s.orgs.control(), p.key_id(), spent.cost).await;
    if q.format.as_deref() == Some("csv") {
        let csv = gai::memory::docs::rows_csv(&rows, &schema);
        return Ok((
            [
                (
                    axum::http::header::CONTENT_TYPE,
                    "text/csv; charset=utf-8".to_owned(),
                ),
                (
                    axum::http::header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{name}-fields.csv\""),
                ),
            ],
            csv,
        )
            .into_response());
    }
    Ok(Json(Extracted {
        schema,
        rows,
        cost_usd: spent.cost,
    })
    .into_response())
}

async fn search(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
    Json(b): Json<SearchRequest>,
) -> ApiResult<Json<Vec<SearchHit>>> {
    Ok(Json(
        s.rt.knowledge()
            .search_as(
                &[name],
                &b.query,
                b.k.unwrap_or(8),
                b.mode,
                p.access.as_ref(),
            )
            .await?,
    ))
}

// ---- memory blocks ----

#[derive(Deserialize)]
struct ScopeQuery {
    scope: Option<String>,
}

fn check_scope(scope: &str) -> ApiResult<()> {
    let ok = ["agent:", "user:", "shared:"].iter().any(|p| {
        scope
            .strip_prefix(p)
            .is_some_and(|rest| !rest.is_empty() && rest.len() <= 128)
    });
    if ok {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "scope must be agent:<name>, user:<id> or shared:<name>",
        ))
    }
}

async fn list_blocks(
    Tenant(s): Tenant,
    Query(q): Query<ScopeQuery>,
) -> ApiResult<Json<Vec<MemoryBlock>>> {
    Ok(Json(s.rt.store().list_blocks(q.scope.as_deref()).await?))
}

async fn create_block(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Json(new): Json<NewBlock>,
) -> ApiResult<(StatusCode, Json<MemoryBlock>)> {
    check_scope(&new.scope)?;
    if !gai::memory::blocks::valid_label(&new.label) {
        return Err(ApiError::bad_request(
            "labels use lowercase letters, digits, '_' and '-' (at most 40)",
        ));
    }
    if new.limit == 0 || new.value.chars().count() > new.limit as usize {
        return Err(ApiError::bad_request("the value is longer than the limit"));
    }
    let actor = actor(&p);
    let b = s.rt.store().create_block(new, Some(&actor)).await?;
    Ok((StatusCode::CREATED, Json(b)))
}

fn actor(p: &Principal) -> String {
    match p.key_id() {
        Some(id) => format!("api:{id}"),
        None => "api".into(),
    }
}

async fn get_block(Tenant(s): Tenant, Path(id): Path<String>) -> ApiResult<Json<MemoryBlock>> {
    Ok(Json(s.rt.store().get_block(&id).await?.ok_or_else(
        || ApiError::not_found(format!("memory block {id}")),
    )?))
}

async fn edit_block(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
    Json(b): Json<EditBlock>,
) -> ApiResult<Json<MemoryBlock>> {
    let store = s.rt.store();
    let mut block = store
        .get_block(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("memory block {id}")))?;
    if b.description.is_some() || b.limit.is_some() || b.read_only.is_some() {
        if b.limit == Some(0) {
            return Err(ApiError::bad_request("limit must be at least 1"));
        }
        block = store
            .update_block_settings(
                &id,
                BlockSettings {
                    description: b.description,
                    limit: b.limit,
                    read_only: b.read_only,
                },
            )
            .await?;
    }
    if let Some(value) = b.value
        && value != block.value
    {
        if value.chars().count() > block.limit as usize {
            return Err(ApiError::bad_request(format!(
                "the value is {} characters, over the block's limit of {}",
                value.chars().count(),
                block.limit
            )));
        }
        block = store
            .set_block_value(&id, &value, None, "edit", Some(&actor(&p)))
            .await?;
    }
    Ok(Json(block))
}

async fn delete_block(Tenant(s): Tenant, Path(id): Path<String>) -> ApiResult<StatusCode> {
    if s.rt.store().delete_block(&id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("memory block {id}")))
    }
}

async fn block_history(
    Tenant(s): Tenant,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<BlockRevision>>> {
    Ok(Json(s.rt.store().block_history(&id).await?))
}

// ---- skills ----

async fn list_skills(Tenant(s): Tenant) -> ApiResult<Json<Vec<SkillView>>> {
    Ok(Json(s.rt.skills().list().await?.iter().map(view).collect()))
}

async fn get_skill(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<Json<SkillDetail>> {
    let sk =
        s.rt.skills()
            .get(&name)
            .await?
            .ok_or_else(|| ApiError::not_found(format!("skill {name}")))?;
    Ok(Json(SkillDetail {
        view: view(&sk),
        instructions: sk.instructions,
        raw: sk.raw,
    }))
}

async fn save_skill(
    Tenant(s): Tenant,
    Json(b): Json<SaveSkill>,
) -> ApiResult<(StatusCode, Json<SavedSkill>)> {
    let (name, version) = s.rt.skills().save(&b.content, b.files).await?;
    Ok((StatusCode::CREATED, Json(SavedSkill { name, version })))
}

async fn delete_skill(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<StatusCode> {
    if s.rt.skills().delete(&name).await? {
        Ok(StatusCode::NO_CONTENT)
    } else if s.rt.skills().get(&name).await?.is_some() {
        Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("skill {name} comes from a skills folder; remove it there"),
        ))
    } else {
        Err(ApiError::not_found(format!("skill {name}")))
    }
}
