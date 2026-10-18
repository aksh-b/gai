//! Knowledge collections: ingest documents (load, chunk, embed, store) and
//! search them by meaning, by keywords, or both fused with reciprocal rank
//! fusion.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gai_llm::ModelRegistry;
use gai_store::{Collection, Document, NewChunk, NewDocument, SharedStore};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::chunk::chunk_text;
use crate::load::{Loaded, is_supported, load_bytes, load_path};
use crate::{MemoryError, Result};

/// Used when neither the collection nor the config names an embedding
/// model. Hashed bag-of-words vectors: no API key, keyword-level matching
/// only. Set `knowledge.embedding_model` for semantic search.
pub const FALLBACK_EMBEDDING_MODEL: &str = "mock/hash";

/// ```toml
/// [knowledge]
/// embedding_model = "openai/text-embedding-3-small"   # or ollama/nomic-embed-text
/// chunk_tokens = 400
/// chunk_overlap = 60
/// ```
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct KnowledgeSettings {
    /// Model new collections (and archival memory) embed with.
    pub embedding_model: Option<String>,
    /// Target chunk size, in tokens.
    pub chunk_tokens: u32,
    /// Tokens repeated between neighbouring chunks.
    pub chunk_overlap: u32,
    /// Texts per embedding request.
    pub embed_batch: usize,
    /// Largest file read when ingesting, in bytes.
    pub max_file_bytes: u64,
}

impl Default for KnowledgeSettings {
    fn default() -> Self {
        Self {
            embedding_model: None,
            chunk_tokens: 400,
            chunk_overlap: 60,
            embed_batch: 64,
            max_file_bytes: 50 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
pub struct NewCollection {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Defaults to `knowledge.embedding_model` from config.
    #[serde(default)]
    pub embedding_model: Option<String>,
    #[serde(default)]
    pub chunk_tokens: Option<u32>,
    #[serde(default)]
    pub chunk_overlap: Option<u32>,
    #[serde(default)]
    pub metadata: Value,
}

/// Text to add to a collection.
#[derive(Clone, Debug, Default)]
pub struct DocumentInput {
    /// Identifies the document; ingesting the same source again replaces it.
    pub source: String,
    pub title: Option<String>,
    pub text: String,
    pub metadata: Value,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct Ingested {
    pub document: Document,
    /// The source was already ingested with identical text; nothing changed.
    pub unchanged: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    /// Vector and keyword results fused by rank.
    #[default]
    Hybrid,
    Vector,
    Text,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SearchHit {
    pub collection: String,
    pub document_id: String,
    pub chunk_id: i64,
    pub seq: u32,
    pub source: String,
    pub title: Option<String>,
    pub heading: Option<String>,
    pub text: String,
    /// Higher is better. Fused rank score in hybrid mode, cosine similarity
    /// in vector mode, BM25 in text mode.
    pub score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector_score: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_score: Option<f32>,
}

/// Collections, ingestion and search over the store.
pub struct Knowledge {
    store: SharedStore,
    models: Arc<ModelRegistry>,
    settings: KnowledgeSettings,
}

/// Rank constant for reciprocal rank fusion; 60 is the usual choice.
const RRF_K: f32 = 20.0;
/// Weight of keyword results against vector results in hybrid search.
const TEXT_WEIGHT: f32 = 0.3;

impl Knowledge {
    pub fn new(
        store: SharedStore,
        models: Arc<ModelRegistry>,
        settings: KnowledgeSettings,
    ) -> Self {
        Self {
            store,
            models,
            settings,
        }
    }

    pub fn store(&self) -> &SharedStore {
        &self.store
    }

    pub fn settings(&self) -> &KnowledgeSettings {
        &self.settings
    }

    pub fn default_embedding_model(&self) -> String {
        self.settings
            .embedding_model
            .clone()
            .unwrap_or_else(|| FALLBACK_EMBEDDING_MODEL.to_owned())
    }

    /// Names people choose: letters, digits, `-`, `_` and `.`, up to 64.
    /// Names with a `:` are reserved (archival memory uses `memory:<scope>`).
    pub fn validate_name(name: &str) -> Result<()> {
        let ok = !name.is_empty()
            && name.len() <= 64
            && name.starts_with(|c: char| c.is_ascii_alphanumeric())
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        if ok {
            Ok(())
        } else {
            Err(MemoryError::Invalid(format!(
                "collection name {name:?}: use letters, digits, '-', '_' and '.' (at most 64, starting with a letter or digit)"
            )))
        }
    }

    pub async fn create_collection(&self, new: NewCollection) -> Result<Collection> {
        Self::validate_name(&new.name)?;
        self.create_unchecked(new).await
    }

    async fn create_unchecked(&self, new: NewCollection) -> Result<Collection> {
        let model = new
            .embedding_model
            .unwrap_or_else(|| self.default_embedding_model());
        // Fail now, not at the first ingest, if the model is unusable.
        let resolved = self.models.embedding(&model)?;
        let chunk_tokens = new
            .chunk_tokens
            .unwrap_or(self.settings.chunk_tokens)
            .max(50);
        let chunk_overlap = new
            .chunk_overlap
            .unwrap_or(self.settings.chunk_overlap)
            .min(chunk_tokens / 2);
        let now = chrono_now();
        let c = Collection {
            name: new.name,
            description: new.description,
            embedding_model: resolved.info().id.clone(),
            dimensions: None,
            chunk_tokens,
            chunk_overlap,
            metadata: new.metadata,
            documents: 0,
            chunks: 0,
            created_at: now,
            updated_at: now,
        };
        self.store.create_collection(&c).await?;
        self.collection(&c.name).await
    }

    /// A collection gai manages itself (such as archival memory), created
    /// with the default settings on first use.
    pub async fn ensure_collection(&self, name: &str, description: &str) -> Result<Collection> {
        if let Some(c) = self.store.get_collection(name).await? {
            return Ok(c);
        }
        let new = NewCollection {
            name: name.to_owned(),
            description: Some(description.to_owned()),
            ..Default::default()
        };
        match self.create_unchecked(new).await {
            Err(MemoryError::Store(gai_store::StoreError::Conflict(_))) => {
                self.collection(name).await
            }
            other => other,
        }
    }

    pub async fn collection(&self, name: &str) -> Result<Collection> {
        self.store
            .get_collection(name)
            .await?
            .ok_or_else(|| MemoryError::NotFound(format!("collection {name}")))
    }

    /// Chunk, embed and store a document. Re-ingesting a source with the
    /// same text does nothing; with new text it replaces the old document.
    pub async fn ingest(&self, collection: &str, doc: DocumentInput) -> Result<Ingested> {
        let c = self.collection(collection).await?;
        if doc.source.trim().is_empty() {
            return Err(MemoryError::Invalid("a document needs a source".into()));
        }
        if doc.text.trim().is_empty() {
            return Err(MemoryError::Invalid(format!("{} has no text", doc.source)));
        }
        let hash = format!("{:x}", Sha256::digest(doc.text.as_bytes()));
        if let Some(existing) = self.store.find_document(collection, &doc.source).await?
            && existing.content_hash == hash
            && existing.title == doc.title
        {
            return Ok(Ingested {
                document: existing,
                unchanged: true,
            });
        }
        let pieces = chunk_text(&doc.text, c.chunk_tokens as usize, c.chunk_overlap as usize);
        let inputs: Vec<String> = pieces
            .iter()
            .map(|p| match (&p.heading, &doc.title) {
                (Some(h), _) => format!("{h}\n\n{}", p.text),
                (None, Some(t)) => format!("{t}\n\n{}", p.text),
                (None, None) => p.text.clone(),
            })
            .collect();
        let vectors = self.embed(&c, &inputs).await?;
        let chunks = pieces
            .into_iter()
            .zip(vectors)
            .map(|(p, v)| NewChunk {
                text: p.text,
                embedding: Some(v),
                metadata: match p.heading {
                    Some(h) => json!({"heading": h}),
                    None => json!({}),
                },
            })
            .collect();
        let document = self
            .store
            .put_document(
                NewDocument {
                    collection: collection.to_owned(),
                    source: doc.source,
                    title: doc.title,
                    content_hash: hash,
                    chars: doc.text.chars().count() as u64,
                    metadata: doc.metadata,
                },
                chunks,
            )
            .await?;
        Ok(Ingested {
            document,
            unchanged: false,
        })
    }

    /// Ingest uploaded bytes (text, Markdown, HTML or PDF). The source
    /// defaults to the file name.
    pub async fn ingest_bytes(
        &self,
        collection: &str,
        name: &str,
        content_type: Option<&str>,
        bytes: Vec<u8>,
        source: Option<String>,
        metadata: Value,
    ) -> Result<Ingested> {
        let (n, ct) = (name.to_owned(), content_type.map(str::to_owned));
        let loaded = tokio::task::spawn_blocking(move || load_bytes(&n, ct.as_deref(), &bytes))
            .await
            .map_err(|e| MemoryError::Invalid(format!("loader crashed: {e}")))??;
        self.ingest_loaded(
            collection,
            source.unwrap_or_else(|| name.to_owned()),
            loaded,
            metadata,
        )
        .await
    }

    async fn ingest_loaded(
        &self,
        collection: &str,
        source: String,
        loaded: Loaded,
        metadata: Value,
    ) -> Result<Ingested> {
        let mut metadata = match metadata {
            Value::Object(m) => m,
            _ => Default::default(),
        };
        metadata.insert("format".into(), json!(loaded.format));
        self.ingest(
            collection,
            DocumentInput {
                source,
                title: loaded.title,
                text: loaded.text,
                metadata: Value::Object(metadata),
            },
        )
        .await
    }

    /// Ingest a file, or every supported file under a directory. Each file
    /// succeeds or fails on its own.
    pub async fn ingest_path(
        &self,
        collection: &str,
        path: &Path,
    ) -> Result<Vec<(PathBuf, Result<Ingested>)>> {
        self.collection(collection).await?;
        let files = if path.is_dir() {
            let mut files = Vec::new();
            walk(path, &mut files);
            files.sort();
            files
        } else if path.is_file() {
            vec![path.to_path_buf()]
        } else {
            return Err(MemoryError::NotFound(path.display().to_string()));
        };
        let mut out = Vec::with_capacity(files.len());
        for f in files {
            let size = std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
            let r = if size > self.settings.max_file_bytes {
                Err(MemoryError::Invalid(format!(
                    "{} is {size} bytes, over the {} byte limit",
                    f.display(),
                    self.settings.max_file_bytes
                )))
            } else {
                let p = f.clone();
                match tokio::task::spawn_blocking(move || load_path(&p)).await {
                    Ok(Ok(loaded)) => {
                        let source = f.to_string_lossy().replace('\\', "/");
                        self.ingest_loaded(collection, source, loaded, json!({}))
                            .await
                    }
                    Ok(Err(e)) => Err(e),
                    Err(e) => Err(MemoryError::Invalid(format!("loader crashed: {e}"))),
                }
            };
            out.push((f, r));
        }
        Ok(out)
    }

    async fn embed(&self, c: &Collection, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let model = self.models.embedding(&c.embedding_model)?;
        let prefix = model
            .info()
            .embed_prefixes
            .as_ref()
            .map(|p| p.document.clone())
            .unwrap_or_default();
        let inputs: Vec<String> = inputs.iter().map(|t| format!("{prefix}{t}")).collect();
        let mut out = Vec::with_capacity(inputs.len());
        for batch in inputs.chunks(self.settings.embed_batch.max(1)) {
            let resp = model.embed(batch).await?;
            if resp.vectors.len() != batch.len() {
                return Err(MemoryError::Invalid(format!(
                    "{} returned {} embeddings for {} inputs",
                    c.embedding_model,
                    resp.vectors.len(),
                    batch.len()
                )));
            }
            if resp.usage.total() > 0 {
                let cost = model.info().cost(&resp.usage);
                if let Err(e) = self
                    .store
                    .record_usage(None, &model.info().id, &resp.usage, cost)
                    .await
                {
                    tracing::warn!(error = %e, "could not record embedding usage");
                }
            }
            out.extend(resp.vectors);
        }
        let dims = out[0].len() as u32;
        match c.dimensions {
            None => self.store.set_collection_dimensions(&c.name, dims).await?,
            Some(d) if d != dims => {
                return Err(MemoryError::Invalid(format!(
                    "{} returned {dims}-dimensional vectors, but collection {} holds {d}-dimensional ones",
                    c.embedding_model, c.name
                )));
            }
            _ => {}
        }
        Ok(out)
    }

    /// The `k` best chunks for `query` across `collections`.
    pub async fn search(
        &self,
        collections: &[String],
        query: &str,
        k: usize,
        mode: SearchMode,
    ) -> Result<Vec<SearchHit>> {
        let k = k.clamp(1, 100);
        if query.trim().is_empty() {
            return Err(MemoryError::Invalid("the search query is empty".into()));
        }
        let mut by_model: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for name in collections {
            let c = self.collection(name).await?;
            by_model.entry(c.embedding_model).or_default().push(c.name);
        }
        // Each ranking contributes more candidates than asked for, so fusion
        // has something to work with.
        let pool = (k * 3).max(20);
        let mut vector: Vec<gai_store::ChunkHit> = Vec::new();
        if mode != SearchMode::Text {
            for (model, names) in &by_model {
                let m = self.models.embedding(model)?;
                let prefix = m
                    .info()
                    .embed_prefixes
                    .as_ref()
                    .map(|p| p.query.as_str())
                    .unwrap_or_default();
                let resp = m.embed(&[format!("{prefix}{query}")]).await?;
                if let Some(q) = resp.vectors.first() {
                    vector.extend(self.store.vector_search(names, q, pool).await?);
                }
            }
            vector.sort_by(|a, b| b.score.total_cmp(&a.score));
            vector.truncate(pool);
        }
        let text = if mode != SearchMode::Vector {
            self.store.text_search(collections, query, pool).await?
        } else {
            Vec::new()
        };

        let mut hits: HashMap<i64, SearchHit> = HashMap::new();
        let mut fused: HashMap<i64, f32> = HashMap::new();
        for (rank, h) in vector.into_iter().enumerate() {
            *fused.entry(h.chunk.id).or_default() += 1.0 / (RRF_K + rank as f32 + 1.0);
            let score = h.score;
            hits.entry(h.chunk.id)
                .or_insert_with(|| to_hit(h))
                .vector_score = Some(score);
        }
        // Keyword matches count half: for questions in plain words, meaning
        // is the better signal; keywords mostly help with names and codes.
        for (rank, h) in text.into_iter().enumerate() {
            *fused.entry(h.chunk.id).or_default() += TEXT_WEIGHT / (RRF_K + rank as f32 + 1.0);
            let score = h.score;
            hits.entry(h.chunk.id)
                .or_insert_with(|| to_hit(h))
                .text_score = Some(score);
        }
        let mut out: Vec<SearchHit> = hits
            .into_values()
            .map(|mut h| {
                h.score = match mode {
                    SearchMode::Hybrid => fused[&h.chunk_id],
                    SearchMode::Vector => h.vector_score.unwrap_or_default(),
                    SearchMode::Text => h.text_score.unwrap_or_default(),
                };
                h
            })
            .collect();
        out.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then(a.chunk_id.cmp(&b.chunk_id))
        });
        out.truncate(k);
        Ok(out)
    }
}

fn to_hit(h: gai_store::ChunkHit) -> SearchHit {
    SearchHit {
        heading: h.chunk.metadata["heading"].as_str().map(str::to_owned),
        collection: h.chunk.collection,
        document_id: h.chunk.document_id,
        chunk_id: h.chunk.id,
        seq: h.chunk.seq,
        source: h.source,
        title: h.title,
        text: h.chunk.text,
        score: 0.0,
        vector_score: None,
        text_score: None,
    }
}

fn chrono_now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

/// Supported files under `dir`, skipping hidden directories and build or
/// dependency folders.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if !name.starts_with('.')
                && !matches!(
                    name.as_str(),
                    "node_modules" | "target" | "dist" | "build" | "__pycache__" | "venv"
                )
            {
                walk(&p, out);
            }
        } else if is_supported(&p) {
            out.push(p);
        }
    }
}

/// Search results as text for a model, numbered for citation.
pub fn format_hits(hits: &[SearchHit]) -> String {
    if hits.is_empty() {
        return "No results.".into();
    }
    let mut out = String::new();
    for (i, h) in hits.iter().enumerate() {
        let mut label = h.title.clone().unwrap_or_else(|| h.source.clone());
        // The heading path usually starts with the document's own title.
        let head = h.heading.as_deref().map(|head| {
            head.strip_prefix(label.as_str())
                .map(|rest| rest.trim_start_matches(" > "))
                .unwrap_or(head)
        });
        if let Some(head) = head.filter(|s| !s.is_empty()) {
            label.push_str(" › ");
            label.push_str(head);
        }
        out.push_str(&format!(
            "[{}] {label}\nsource: {} (collection {}, chunk {})\n{}\n\n",
            i + 1,
            h.source,
            h.collection,
            h.seq,
            h.text.trim()
        ));
    }
    out.trim_end().to_owned()
}
