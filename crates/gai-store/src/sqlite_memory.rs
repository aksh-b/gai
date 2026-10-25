//! SQLite implementation of memory blocks, knowledge and skills.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use async_trait::async_trait;
use futures::TryStreamExt;
use serde_json::Value;
use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite};

use super::{SqliteStore, json, now, parse};
use crate::*;

fn block(r: &SqliteRow) -> Result<MemoryBlock> {
    Ok(MemoryBlock {
        id: r.try_get("id")?,
        scope: r.try_get("scope")?,
        label: r.try_get("label")?,
        description: r.try_get("description")?,
        value: r.try_get("value")?,
        limit: r.try_get::<i64, _>("char_limit")? as u32,
        read_only: r.try_get("read_only")?,
        version: r.try_get::<i64, _>("version")? as u32,
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
    })
}

fn collection(r: &SqliteRow) -> Result<Collection> {
    Ok(Collection {
        name: r.try_get("name")?,
        description: r.try_get("description")?,
        embedding_model: r.try_get("embedding_model")?,
        dimensions: r.try_get::<Option<i64>, _>("dimensions")?.map(|d| d as u32),
        chunk_tokens: r.try_get::<i64, _>("chunk_tokens")? as u32,
        chunk_overlap: r.try_get::<i64, _>("chunk_overlap")? as u32,
        metadata: parse(&r.try_get::<String, _>("metadata")?)?,
        documents: r
            .try_get::<Option<i64>, _>("n_documents")
            .ok()
            .flatten()
            .unwrap_or(0) as u64,
        chunks: r
            .try_get::<Option<i64>, _>("n_chunks")
            .ok()
            .flatten()
            .unwrap_or(0) as u64,
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
    })
}

fn document(r: &SqliteRow) -> Result<Document> {
    Ok(Document {
        id: r.try_get("id")?,
        collection: r.try_get("collection")?,
        source: r.try_get("source")?,
        title: r.try_get("title")?,
        content_hash: r.try_get("content_hash")?,
        chars: r.try_get::<i64, _>("chars")? as u64,
        chunks: r.try_get::<i64, _>("chunks")? as u32,
        metadata: parse(&r.try_get::<String, _>("metadata")?)?,
        created_at: r.try_get("created_at")?,
    })
}

fn chunk(r: &SqliteRow) -> Result<Chunk> {
    Ok(Chunk {
        id: r.try_get("id")?,
        document_id: r.try_get("document_id")?,
        collection: r.try_get("collection")?,
        seq: r.try_get::<i64, _>("seq")? as u32,
        text: r.try_get("text")?,
        metadata: parse(&r.try_get::<String, _>("metadata")?)?,
    })
}

fn skill(r: &SqliteRow) -> Result<StoredSkill> {
    Ok(StoredSkill {
        name: r.try_get("name")?,
        description: r.try_get("description")?,
        version: r.try_get::<i64, _>("version")? as u32,
        body: r.try_get("body")?,
        files: parse(&r.try_get::<String, _>("files")?)?,
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
    })
}

fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn from_blob(b: &[u8]) -> impl Iterator<Item = f32> + '_ {
    b.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c))
}

fn push_in(q: &mut QueryBuilder<Sqlite>, column: &str, values: &[String]) {
    q.push(format!(" {column} IN ("));
    let mut sep = q.separated(", ");
    for v in values {
        sep.push_bind(v.clone());
    }
    q.push(")");
}

/// Plain words as an FTS5 query: each word quoted (so operators are text),
/// any word may match, BM25 ranks documents with more of them higher.
/// Common English words that match nearly every chunk; ORing them in
/// drowns out the words that matter.
const STOPWORDS: &[&str] = &[
    "a", "about", "after", "all", "also", "am", "an", "and", "any", "are", "as", "at", "be",
    "because", "been", "before", "being", "between", "both", "but", "by", "can", "could", "did",
    "do", "does", "doing", "each", "few", "for", "from", "get", "gets", "got", "had", "has",
    "have", "having", "he", "her", "here", "his", "how", "i", "if", "in", "into", "is", "it",
    "its", "just", "me", "more", "most", "my", "no", "not", "of", "on", "once", "only", "or",
    "other", "our", "out", "over", "own", "same", "she", "should", "so", "some", "such", "than",
    "that", "the", "their", "them", "then", "there", "these", "they", "this", "those", "through",
    "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where",
    "which", "while", "who", "whom", "why", "will", "with", "would", "you", "your",
];

fn fts_query(text: &str) -> Option<String> {
    let all: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    let mut words: Vec<&String> = all
        .iter()
        .filter(|w| !STOPWORDS.contains(&w.as_str()))
        .collect();
    // A query made only of common words still searches for them.
    if words.is_empty() {
        words = all.iter().collect();
    }
    words.dedup();
    let words: Vec<String> = words
        .into_iter()
        .take(64)
        .map(|w| format!("\"{w}\""))
        .collect();
    (!words.is_empty()).then(|| words.join(" OR "))
}

impl SqliteStore {
    /// Full chunks for hits found by id, in the order given.
    async fn hits(&self, scored: Vec<(i64, f32)>) -> Result<Vec<ChunkHit>> {
        if scored.is_empty() {
            return Ok(Vec::new());
        }
        let mut q: QueryBuilder<Sqlite> = QueryBuilder::new(
            "SELECT c.*, d.source AS doc_source, d.title AS doc_title FROM chunks c JOIN documents d ON d.id = c.document_id WHERE c.id IN (",
        );
        let mut sep = q.separated(", ");
        for (id, _) in &scored {
            sep.push_bind(*id);
        }
        q.push(")");
        let rows = q.build().fetch_all(&self.pool).await?;
        let mut by_id = BTreeMap::new();
        for r in &rows {
            let c = chunk(r)?;
            let source: String = r.try_get("doc_source")?;
            let title: Option<String> = r.try_get("doc_title")?;
            by_id.insert(c.id, (c, source, title));
        }
        Ok(scored
            .into_iter()
            .filter_map(|(id, score)| {
                by_id.remove(&id).map(|(chunk, source, title)| ChunkHit {
                    chunk,
                    score,
                    source,
                    title,
                })
            })
            .collect())
    }
}

#[async_trait]
impl MemoryStore for SqliteStore {
    async fn list_blocks(&self, scope: Option<&str>) -> Result<Vec<MemoryBlock>> {
        let rows = match scope {
            Some(s) => {
                sqlx::query("SELECT * FROM memory_blocks WHERE scope = ? ORDER BY label")
                    .bind(s)
                    .fetch_all(&self.pool)
                    .await?
            }
            None => {
                sqlx::query("SELECT * FROM memory_blocks ORDER BY scope, label")
                    .fetch_all(&self.pool)
                    .await?
            }
        };
        rows.iter().map(block).collect()
    }

    async fn get_block(&self, id: &str) -> Result<Option<MemoryBlock>> {
        let row = sqlx::query("SELECT * FROM memory_blocks WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(block).transpose()
    }

    async fn find_block(&self, scope: &str, label: &str) -> Result<Option<MemoryBlock>> {
        let row = sqlx::query("SELECT * FROM memory_blocks WHERE scope = ? AND label = ?")
            .bind(scope)
            .bind(label)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(block).transpose()
    }

    async fn create_block(&self, new: NewBlock, actor: Option<&str>) -> Result<MemoryBlock> {
        let id = gai_core::new_id("blk");
        let ts = now();
        let mut tx = self.write_tx().await?;
        let r = sqlx::query(
            "INSERT INTO memory_blocks (id, scope, label, description, value, char_limit, read_only, version, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, 1, ?, ?) ON CONFLICT (scope, label) DO NOTHING",
        )
        .bind(&id)
        .bind(&new.scope)
        .bind(&new.label)
        .bind(&new.description)
        .bind(&new.value)
        .bind(new.limit as i64)
        .bind(new.read_only)
        .bind(ts)
        .bind(ts)
        .execute(&mut *tx)
        .await?;
        if r.rows_affected() == 0 {
            return Err(StoreError::Conflict(format!(
                "{} already has a block named {}",
                new.scope, new.label
            )));
        }
        sqlx::query(
            "INSERT INTO memory_block_history (block_id, version, value, op, actor, created_at) VALUES (?, 1, ?, 'create', ?, ?)",
        )
        .bind(&id)
        .bind(&new.value)
        .bind(actor)
        .bind(ts)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.get_block(&id).await?.ok_or(StoreError::NotFound(id))
    }

    async fn ensure_block(&self, new: NewBlock) -> Result<MemoryBlock> {
        if let Some(b) = self.find_block(&new.scope, &new.label).await? {
            return Ok(b);
        }
        let (scope, label) = (new.scope.clone(), new.label.clone());
        match self.create_block(new, None).await {
            Ok(b) => Ok(b),
            // Someone else created it in between.
            Err(StoreError::Conflict(_)) => self
                .find_block(&scope, &label)
                .await?
                .ok_or(StoreError::NotFound(format!("block {scope}/{label}"))),
            Err(e) => Err(e),
        }
    }

    async fn set_block_value(
        &self,
        id: &str,
        value: &str,
        expected_version: Option<u32>,
        op: &str,
        actor: Option<&str>,
    ) -> Result<MemoryBlock> {
        let mut tx = self.write_tx().await?;
        let current: Option<(i64,)> =
            sqlx::query_as("SELECT version FROM memory_blocks WHERE id = ?")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some((version,)) = current else {
            return Err(StoreError::NotFound(format!("block {id}")));
        };
        if let Some(expected) = expected_version
            && expected as i64 != version
        {
            return Err(StoreError::Conflict(format!(
                "block {id} changed (now version {version}, expected {expected})"
            )));
        }
        let next = version + 1;
        let ts = now();
        sqlx::query("UPDATE memory_blocks SET value = ?, version = ?, updated_at = ? WHERE id = ?")
            .bind(value)
            .bind(next)
            .bind(ts)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO memory_block_history (block_id, version, value, op, actor, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(next)
        .bind(value)
        .bind(op)
        .bind(actor)
        .bind(ts)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.get_block(id)
            .await?
            .ok_or_else(|| StoreError::NotFound(format!("block {id}")))
    }

    async fn update_block_settings(&self, id: &str, s: BlockSettings) -> Result<MemoryBlock> {
        let mut q: QueryBuilder<Sqlite> =
            QueryBuilder::new("UPDATE memory_blocks SET updated_at = ");
        q.push_bind(now());
        if let Some(d) = &s.description {
            q.push(", description = ").push_bind(d.clone());
        }
        if let Some(l) = s.limit {
            q.push(", char_limit = ").push_bind(l as i64);
        }
        if let Some(r) = s.read_only {
            q.push(", read_only = ").push_bind(r);
        }
        q.push(" WHERE id = ").push_bind(id);
        if q.build().execute(&self.pool).await?.rows_affected() == 0 {
            return Err(StoreError::NotFound(format!("block {id}")));
        }
        self.get_block(id)
            .await?
            .ok_or_else(|| StoreError::NotFound(format!("block {id}")))
    }

    async fn delete_block(&self, id: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM memory_blocks WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    async fn block_history(&self, id: &str) -> Result<Vec<BlockRevision>> {
        let rows = sqlx::query(
            "SELECT * FROM memory_block_history WHERE block_id = ? ORDER BY version DESC",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(BlockRevision {
                    version: r.try_get::<i64, _>("version")? as u32,
                    value: r.try_get("value")?,
                    op: r.try_get("op")?,
                    actor: r.try_get("actor")?,
                    created_at: r.try_get("created_at")?,
                })
            })
            .collect()
    }
}

const COLLECTION_SELECT: &str = "SELECT c.*,
    (SELECT COUNT(*) FROM documents d WHERE d.collection = c.name) AS n_documents,
    (SELECT COUNT(*) FROM chunks k WHERE k.collection = c.name) AS n_chunks
    FROM collections c";

#[async_trait]
impl KnowledgeStore for SqliteStore {
    async fn create_collection(&self, c: &Collection) -> Result<()> {
        let ts = now();
        let r = sqlx::query(
            "INSERT INTO collections (name, description, embedding_model, dimensions, chunk_tokens, chunk_overlap, metadata, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (name) DO NOTHING",
        )
        .bind(&c.name)
        .bind(&c.description)
        .bind(&c.embedding_model)
        .bind(c.dimensions.map(|d| d as i64))
        .bind(c.chunk_tokens as i64)
        .bind(c.chunk_overlap as i64)
        .bind(if c.metadata.is_null() {
            "{}".to_owned()
        } else {
            json(&c.metadata)
        })
        .bind(ts)
        .bind(ts)
        .execute(&self.pool)
        .await?;
        if r.rows_affected() == 0 {
            return Err(StoreError::Conflict(format!(
                "collection {} already exists",
                c.name
            )));
        }
        Ok(())
    }

    async fn get_collection(&self, name: &str) -> Result<Option<Collection>> {
        let row = sqlx::query(&format!("{COLLECTION_SELECT} WHERE c.name = ?"))
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(collection).transpose()
    }

    async fn list_collections(&self) -> Result<Vec<Collection>> {
        let rows = sqlx::query(&format!("{COLLECTION_SELECT} ORDER BY c.name"))
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(collection).collect()
    }

    async fn set_collection_dimensions(&self, name: &str, dimensions: u32) -> Result<()> {
        sqlx::query("UPDATE collections SET dimensions = ?, updated_at = ? WHERE name = ?")
            .bind(dimensions as i64)
            .bind(now())
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn delete_collection(&self, name: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM collections WHERE name = ?")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    async fn put_document(&self, doc: NewDocument, chunks: Vec<NewChunk>) -> Result<Document> {
        let id = gai_core::new_id("doc");
        let ts = now();
        let mut tx = self.write_tx().await?;
        sqlx::query("DELETE FROM documents WHERE collection = ? AND source = ?")
            .bind(&doc.collection)
            .bind(&doc.source)
            .execute(&mut *tx)
            .await?;
        let metadata = if doc.metadata.is_null() {
            Value::Object(Default::default())
        } else {
            doc.metadata
        };
        sqlx::query(
            "INSERT INTO documents (id, collection, source, title, content_hash, chars, chunks, metadata, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&doc.collection)
        .bind(&doc.source)
        .bind(&doc.title)
        .bind(&doc.content_hash)
        .bind(doc.chars as i64)
        .bind(chunks.len() as i64)
        .bind(json(&metadata))
        .bind(ts)
        .execute(&mut *tx)
        .await?;
        for (seq, c) in chunks.iter().enumerate() {
            let meta = if c.metadata.is_null() {
                Value::Object(Default::default())
            } else {
                c.metadata.clone()
            };
            sqlx::query(
                "INSERT INTO chunks (document_id, collection, seq, text, embedding, metadata) VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(&id)
            .bind(&doc.collection)
            .bind(seq as i64)
            .bind(&c.text)
            .bind(c.embedding.as_deref().map(to_blob))
            .bind(json(&meta))
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("UPDATE collections SET updated_at = ? WHERE name = ?")
            .bind(ts)
            .bind(&doc.collection)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.get_document(&id)
            .await?
            .ok_or(StoreError::NotFound(id))
    }

    async fn get_document(&self, id: &str) -> Result<Option<Document>> {
        let row = sqlx::query("SELECT * FROM documents WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(document).transpose()
    }

    async fn find_document(&self, collection: &str, source: &str) -> Result<Option<Document>> {
        let row = sqlx::query("SELECT * FROM documents WHERE collection = ? AND source = ?")
            .bind(collection)
            .bind(source)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(document).transpose()
    }

    async fn list_documents(
        &self,
        collection: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Document>> {
        let rows = sqlx::query(
            "SELECT * FROM documents WHERE collection = ? ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?",
        )
        .bind(collection)
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(document).collect()
    }

    async fn delete_document(&self, id: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM documents WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    async fn document_chunks(&self, id: &str) -> Result<Vec<Chunk>> {
        let rows = sqlx::query("SELECT * FROM chunks WHERE document_id = ? ORDER BY seq")
            .bind(id)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(chunk).collect()
    }

    async fn vector_search(
        &self,
        collections: &[String],
        query: &[f32],
        k: usize,
    ) -> Result<Vec<ChunkHit>> {
        if collections.is_empty() || k == 0 {
            return Ok(Vec::new());
        }
        let qnorm = query.iter().map(|x| x * x).sum::<f32>().sqrt();
        if qnorm == 0.0 {
            return Ok(Vec::new());
        }
        let mut q: QueryBuilder<Sqlite> =
            QueryBuilder::new("SELECT id, embedding FROM chunks WHERE embedding IS NOT NULL AND");
        push_in(&mut q, "collection", collections);
        // Brute force over the stream, keeping the best k: fine up to about
        // 100k chunks, after which an index (sqlite-vec, Qdrant) belongs
        // behind this trait.
        let mut best: Vec<(i64, f32)> = Vec::with_capacity(k + 1);
        let mut rows = q.build().fetch(&self.pool);
        while let Some(r) = rows.try_next().await? {
            let blob: Vec<u8> = r.try_get("embedding")?;
            if blob.len() != query.len() * 4 {
                continue; // embedded with another model
            }
            let (mut dot, mut norm) = (0f32, 0f32);
            for (a, b) in from_blob(&blob).zip(query) {
                dot += a * b;
                norm += a * a;
            }
            if norm == 0.0 {
                continue;
            }
            let score = dot / (norm.sqrt() * qnorm);
            if best.len() < k || score > best[best.len() - 1].1 {
                let pos = best
                    .iter()
                    .position(|(_, s)| score > *s)
                    .unwrap_or(best.len());
                best.insert(pos, (r.try_get("id")?, score));
                best.truncate(k);
            }
        }
        drop(rows);
        self.hits(best).await
    }

    async fn chunk_hits(&self, scored: Vec<(i64, f32)>) -> Result<Vec<ChunkHit>> {
        self.hits(scored).await
    }

    async fn chunk_vectors(
        &self,
        collection: &str,
        document_id: Option<&str>,
        after: i64,
        limit: u32,
    ) -> Result<Vec<(i64, Vec<f32>)>> {
        let rows = match document_id {
            Some(d) => {
                sqlx::query("SELECT id, embedding FROM chunks WHERE collection = ? AND document_id = ? AND embedding IS NOT NULL ORDER BY id")
                    .bind(collection)
                    .bind(d)
                    .fetch_all(&self.pool)
                    .await?
            }
            None => {
                sqlx::query("SELECT id, embedding FROM chunks WHERE collection = ? AND id > ? AND embedding IS NOT NULL ORDER BY id LIMIT ?")
                    .bind(collection)
                    .bind(after)
                    .bind(limit as i64)
                    .fetch_all(&self.pool)
                    .await?
            }
        };
        rows.iter()
            .map(|r| {
                let blob: Vec<u8> = r.try_get("embedding")?;
                Ok((r.try_get::<i64, _>("id")?, from_blob(&blob).collect()))
            })
            .collect()
    }

    async fn text_search(
        &self,
        collections: &[String],
        query: &str,
        k: usize,
    ) -> Result<Vec<ChunkHit>> {
        let Some(fts) = fts_query(query) else {
            return Ok(Vec::new());
        };
        if collections.is_empty() || k == 0 {
            return Ok(Vec::new());
        }
        let mut q: QueryBuilder<Sqlite> = QueryBuilder::new(
            "SELECT c.id, bm25(chunks_fts) AS rank FROM chunks_fts JOIN chunks c ON c.id = chunks_fts.rowid WHERE chunks_fts MATCH ",
        );
        q.push_bind(fts).push(" AND");
        push_in(&mut q, "c.collection", collections);
        q.push(" ORDER BY rank LIMIT ").push_bind(k as i64);
        let rows = q.build().fetch_all(&self.pool).await?;
        let mut scored = Vec::with_capacity(rows.len());
        for r in &rows {
            let rank: f64 = r.try_get("rank")?;
            scored.push((r.try_get::<i64, _>("id")?, -rank as f32));
        }
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));
        self.hits(scored).await
    }
}

#[async_trait]
impl SkillStore for SqliteStore {
    async fn put_skill(
        &self,
        name: &str,
        description: &str,
        body: &str,
        files: &BTreeMap<String, String>,
    ) -> Result<u32> {
        let ts = now();
        let mut tx = self.write_tx().await?;
        let current: Option<(i64, String, String)> =
            sqlx::query_as("SELECT version, body, files FROM skills WHERE name = ?")
                .bind(name)
                .fetch_optional(&mut *tx)
                .await?;
        let version = match current {
            Some((v, b, f)) if b == body && f == json(files) => return Ok(v as u32),
            Some((v, ..)) => v + 1,
            None => 1,
        };
        sqlx::query(
            "INSERT INTO skills (name, description, version, body, files, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (name) DO UPDATE SET description = excluded.description, version = excluded.version,
                 body = excluded.body, files = excluded.files, updated_at = excluded.updated_at",
        )
        .bind(name)
        .bind(description)
        .bind(version)
        .bind(body)
        .bind(json(files))
        .bind(ts)
        .bind(ts)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(version as u32)
    }

    async fn get_skill(&self, name: &str) -> Result<Option<StoredSkill>> {
        let row = sqlx::query("SELECT * FROM skills WHERE name = ?")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(skill).transpose()
    }

    async fn list_skills(&self) -> Result<Vec<StoredSkill>> {
        let rows = sqlx::query("SELECT * FROM skills ORDER BY name")
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(skill).collect()
    }

    async fn delete_skill(&self, name: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM skills WHERE name = ?")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }
}
