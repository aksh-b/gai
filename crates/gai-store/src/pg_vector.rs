//! A [`VectorIndex`] in Postgres with the pgvector extension.
//!
//! One table holds every collection's vectors, with the dimensions beside
//! them; an HNSW index per dimension size (cosine) answers searches, and
//! iterative scans keep the collection filter from starving results.

use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Mutex;

use async_trait::async_trait;
use sqlx::Row;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};

use crate::{Result, StoreError, VectorIndex};

pub struct PgVectorIndex {
    pool: PgPool,
    /// `gai_vectors`, or `gai_vectors_<org>` for an organization.
    table: String,
    /// Dimension sizes that already have their HNSW index.
    indexed: Mutex<HashSet<usize>>,
}

/// pgvector's text form: `[0.1,0.2,...]`.
fn literal(v: &[f32]) -> String {
    let mut s = String::with_capacity(v.len() * 10 + 2);
    s.push('[');
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&x.to_string());
    }
    s.push(']');
    s
}

impl PgVectorIndex {
    /// Connect to a Postgres that has (or may create) the `vector`
    /// extension, and create the vectors table.
    pub async fn connect(url: &str) -> Result<Self> {
        let opts =
            PgConnectOptions::from_str(url).map_err(|e| StoreError::Database(e.to_string()))?;
        Self::connect_with(opts, "gai_vectors").await
    }

    /// Like [`connect`](Self::connect), keeping the vectors in `table`.
    pub async fn connect_table(url: &str, table: &str) -> Result<Self> {
        let opts =
            PgConnectOptions::from_str(url).map_err(|e| StoreError::Database(e.to_string()))?;
        Self::connect_with(opts, table).await
    }

    /// Keep the vectors in `table` (letters, digits and `_`).
    pub async fn connect_with(opts: PgConnectOptions, table: &str) -> Result<Self> {
        if table.is_empty() || !table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(StoreError::Database(format!(
                "bad vector table name {table:?}"
            )));
        }
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect_with(opts)
            .await?;
        sqlx::query("CREATE EXTENSION IF NOT EXISTS vector")
            .execute(&pool)
            .await
            .map_err(|e| {
                StoreError::Database(format!(
                    "pgvector is not available in this Postgres (CREATE EXTENSION vector: {e})"
                ))
            })?;
        sqlx::query(&format!(
            "CREATE TABLE IF NOT EXISTS {table} (
                 chunk_id   BIGINT PRIMARY KEY,
                 collection TEXT NOT NULL,
                 dims       INTEGER NOT NULL,
                 embedding  vector NOT NULL
             )"
        ))
        .execute(&pool)
        .await?;
        sqlx::query(&format!(
            "CREATE INDEX IF NOT EXISTS {table}_collection ON {table}(collection)"
        ))
        .execute(&pool)
        .await?;
        Ok(Self {
            pool,
            table: table.to_owned(),
            indexed: Mutex::default(),
        })
    }

    async fn ensure_index(&self, dims: usize) -> Result<()> {
        if self.indexed.lock().unwrap().contains(&dims) {
            return Ok(());
        }
        // HNSW needs a fixed size; a partial index per size keeps one table.
        let t = &self.table;
        sqlx::query(&format!(
            "CREATE INDEX IF NOT EXISTS {t}_hnsw_{dims} ON {t}
             USING hnsw ((embedding::vector({dims})) vector_cosine_ops) WHERE dims = {dims}"
        ))
        .execute(&self.pool)
        .await?;
        self.indexed.lock().unwrap().insert(dims);
        Ok(())
    }
}

#[async_trait]
impl VectorIndex for PgVectorIndex {
    fn backend(&self) -> &'static str {
        "pgvector"
    }

    async fn upsert(&self, collection: &str, points: &[(i64, Vec<f32>)]) -> Result<()> {
        let mut dims = HashSet::new();
        let mut tx = self.pool.begin().await?;
        for (id, v) in points {
            if v.is_empty() {
                continue;
            }
            dims.insert(v.len());
            sqlx::query(&format!(
                "INSERT INTO {} (chunk_id, collection, dims, embedding) VALUES ($1, $2, $3, $4::vector)
                 ON CONFLICT (chunk_id) DO UPDATE SET collection = EXCLUDED.collection, dims = EXCLUDED.dims, embedding = EXCLUDED.embedding",
                self.table
            ))
            .bind(id)
            .bind(collection)
            .bind(v.len() as i32)
            .bind(literal(v))
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        for d in dims {
            self.ensure_index(d).await?;
        }
        Ok(())
    }

    async fn remove(&self, _collection: &str, ids: &[i64]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        sqlx::query(&format!(
            "DELETE FROM {} WHERE chunk_id = ANY($1)",
            self.table
        ))
        .bind(ids)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn drop_collection(&self, collection: &str) -> Result<()> {
        sqlx::query(&format!("DELETE FROM {} WHERE collection = $1", self.table))
            .bind(collection)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn search(
        &self,
        collections: &[String],
        query: &[f32],
        k: usize,
    ) -> Result<Vec<(i64, f32)>> {
        if collections.is_empty() || query.is_empty() || k == 0 {
            return Ok(Vec::new());
        }
        let d = query.len();
        let mut tx = self.pool.begin().await?;
        // Keep scanning the index until enough rows pass the filter.
        for setting in [
            "SET LOCAL hnsw.iterative_scan = relaxed_order",
            "SET LOCAL hnsw.ef_search = 100",
        ] {
            // Older pgvector lacks iterative scans; exact results still come back.
            let _ = sqlx::query(setting).execute(&mut *tx).await;
        }
        let rows = sqlx::query(&format!(
            "SELECT chunk_id, 1 - (embedding::vector({d}) <=> $1::vector({d})) AS score
             FROM {} WHERE dims = {d} AND collection = ANY($2)
             ORDER BY embedding::vector({d}) <=> $1::vector({d}) LIMIT $3",
            self.table
        ))
        .bind(literal(query))
        .bind(collections)
        .bind(k as i64)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        rows.iter()
            .map(|r| {
                Ok((
                    r.try_get::<i64, _>("chunk_id")?,
                    r.try_get::<f64, _>("score")? as f32,
                ))
            })
            .collect()
    }
}
