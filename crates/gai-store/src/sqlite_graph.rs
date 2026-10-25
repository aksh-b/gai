//! SQLite implementation of the knowledge graph.

use async_trait::async_trait;
use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite};

use super::{SqliteStore, parse};
use crate::*;

fn node(r: &SqliteRow) -> Result<GraphNode> {
    Ok(GraphNode {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        kind: r.try_get("kind")?,
        description: r.try_get("description")?,
        mentions: r.try_get::<i64, _>("mentions")? as u64,
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

/// `name` as the graph keys it.
pub(crate) fn graph_key(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

const NODE_COLUMNS: &str =
    "SELECT n.id, n.name, n.kind, n.description, COUNT(m.chunk_id) AS mentions
     FROM graph_nodes n JOIN graph_mentions m ON m.node_id = n.id";

#[async_trait]
impl GraphStore for SqliteStore {
    async fn graph_pending(&self, collection: &str, limit: u32) -> Result<Vec<Chunk>> {
        let rows = sqlx::query(
            "SELECT c.id, c.document_id, c.collection, c.seq, c.text, c.metadata FROM chunks c
             WHERE c.collection = ? AND NOT EXISTS (SELECT 1 FROM graph_done d WHERE d.chunk_id = c.id)
             ORDER BY c.document_id, c.seq LIMIT ?",
        )
        .bind(collection)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(chunk).collect()
    }

    async fn graph_add(
        &self,
        collection: &str,
        done: &[i64],
        entities: &[NewEntity],
        relations: &[NewRelation],
    ) -> Result<()> {
        let mut tx = self.write_tx().await?;
        let mut ids: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        let upsert = async |tx: &mut sqlx::Transaction<'static, Sqlite>,
                            name: &str,
                            kind: &str,
                            description: Option<&str>|
               -> Result<i64> {
            let key = graph_key(name);
            Ok(sqlx::query_scalar(
                "INSERT INTO graph_nodes (collection, key, name, kind, description) VALUES (?, ?, ?, ?, ?)
                 ON CONFLICT (collection, key) DO UPDATE SET
                     description = COALESCE(graph_nodes.description, excluded.description),
                     kind = CASE WHEN graph_nodes.kind = 'other' THEN excluded.kind ELSE graph_nodes.kind END
                 RETURNING id",
            )
            .bind(collection)
            .bind(&key)
            .bind(name.trim())
            .bind(kind)
            .bind(description)
            .fetch_one(&mut **tx)
            .await?)
        };
        for e in entities {
            if graph_key(&e.name).is_empty() {
                continue;
            }
            let id = upsert(&mut tx, &e.name, &e.kind, e.description.as_deref()).await?;
            ids.insert(graph_key(&e.name), id);
            for c in &e.chunk_ids {
                sqlx::query("INSERT INTO graph_mentions (node_id, chunk_id) VALUES (?, ?) ON CONFLICT DO NOTHING")
                    .bind(id)
                    .bind(c)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        for r in relations {
            let (s, t) = (graph_key(&r.source), graph_key(&r.target));
            if s.is_empty() || t.is_empty() || s == t || r.relation.trim().is_empty() {
                continue;
            }
            let end = async |key: &str,
                             name: &str,
                             tx: &mut sqlx::Transaction<'static, Sqlite>|
                   -> Result<i64> {
                match ids.get(key) {
                    Some(id) => Ok(*id),
                    None => {
                        let id = upsert(tx, name, "other", None).await?;
                        sqlx::query("INSERT INTO graph_mentions (node_id, chunk_id) VALUES (?, ?) ON CONFLICT DO NOTHING")
                            .bind(id)
                            .bind(r.chunk_id)
                            .execute(&mut **tx)
                            .await?;
                        Ok(id)
                    }
                }
            };
            let sid = end(&s, &r.source, &mut tx).await?;
            let tid = end(&t, &r.target, &mut tx).await?;
            ids.insert(s, sid);
            ids.insert(t, tid);
            sqlx::query(
                "INSERT INTO graph_edges (collection, source_id, target_id, relation, chunk_id) VALUES (?, ?, ?, ?, ?)
                 ON CONFLICT DO NOTHING",
            )
            .bind(collection)
            .bind(sid)
            .bind(tid)
            .bind(r.relation.trim().to_lowercase())
            .bind(r.chunk_id)
            .execute(&mut *tx)
            .await?;
        }
        for c in done {
            sqlx::query("INSERT INTO graph_done (chunk_id) VALUES (?) ON CONFLICT DO NOTHING")
                .bind(c)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn graph_nodes(
        &self,
        collection: &str,
        contains: &str,
        limit: u32,
    ) -> Result<Vec<GraphNode>> {
        let mut q = QueryBuilder::<Sqlite>::new(NODE_COLUMNS);
        q.push(" WHERE n.collection = ").push_bind(collection);
        let k = graph_key(contains);
        if !k.is_empty() {
            q.push(" AND n.key LIKE ")
                .push_bind(format!("%{}%", k.replace('%', "")));
        }
        q.push(" GROUP BY n.id ORDER BY mentions DESC, n.name LIMIT ")
            .push_bind(i64::from(limit.clamp(1, 5000)));
        let rows = q.build().fetch_all(&self.pool).await?;
        rows.iter().map(node).collect()
    }

    async fn graph_nodes_by_id(&self, ids: &[i64]) -> Result<Vec<GraphNode>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut q = QueryBuilder::<Sqlite>::new(NODE_COLUMNS);
        q.push(" WHERE n.id IN (");
        let mut sep = q.separated(", ");
        for id in ids {
            sep.push_bind(id);
        }
        q.push(") GROUP BY n.id ORDER BY mentions DESC");
        let rows = q.build().fetch_all(&self.pool).await?;
        rows.iter().map(node).collect()
    }

    async fn graph_edges(
        &self,
        collection: &str,
        nodes: &[i64],
        limit: u32,
    ) -> Result<Vec<GraphEdge>> {
        let mut q = QueryBuilder::<Sqlite>::new(
            "SELECT source_id, target_id, relation, COUNT(DISTINCT chunk_id) AS weight FROM graph_edges WHERE collection = ",
        );
        q.push_bind(collection);
        if !nodes.is_empty() {
            q.push(" AND (source_id IN (");
            let mut sep = q.separated(", ");
            for id in nodes {
                sep.push_bind(id);
            }
            q.push(") OR target_id IN (");
            let mut sep = q.separated(", ");
            for id in nodes {
                sep.push_bind(id);
            }
            q.push("))");
        }
        q.push(" GROUP BY source_id, target_id, relation ORDER BY weight DESC, relation LIMIT ")
            .push_bind(i64::from(limit.clamp(1, 20_000)));
        let rows = q.build().fetch_all(&self.pool).await?;
        rows.iter()
            .map(|r| {
                Ok(GraphEdge {
                    source: r.try_get("source_id")?,
                    target: r.try_get("target_id")?,
                    relation: r.try_get("relation")?,
                    weight: r.try_get::<i64, _>("weight")? as u64,
                })
            })
            .collect()
    }

    async fn graph_chunks(&self, nodes: &[i64], limit: u32) -> Result<Vec<(i64, u32)>> {
        if nodes.is_empty() {
            return Ok(Vec::new());
        }
        let mut q = QueryBuilder::<Sqlite>::new(
            "SELECT chunk_id, COUNT(*) AS n FROM graph_mentions WHERE node_id IN (",
        );
        let mut sep = q.separated(", ");
        for id in nodes {
            sep.push_bind(id);
        }
        q.push(") GROUP BY chunk_id ORDER BY n DESC, chunk_id LIMIT ")
            .push_bind(i64::from(limit.clamp(1, 1000)));
        let rows = q.build().fetch_all(&self.pool).await?;
        rows.iter()
            .map(|r| Ok((r.try_get("chunk_id")?, r.try_get::<i64, _>("n")? as u32)))
            .collect()
    }

    async fn graph_chunk_nodes(&self, chunks: &[i64]) -> Result<Vec<i64>> {
        if chunks.is_empty() {
            return Ok(Vec::new());
        }
        let mut q = QueryBuilder::<Sqlite>::new(
            "SELECT DISTINCT node_id FROM graph_mentions WHERE chunk_id IN (",
        );
        let mut sep = q.separated(", ");
        for id in chunks {
            sep.push_bind(id);
        }
        q.push(")");
        Ok(q.build_query_scalar().fetch_all(&self.pool).await?)
    }

    async fn graph_stats(&self, collection: &str) -> Result<GraphStats> {
        let row = sqlx::query(
            "SELECT
                 (SELECT COUNT(*) FROM graph_nodes n WHERE n.collection = ? AND EXISTS (SELECT 1 FROM graph_mentions m WHERE m.node_id = n.id)) AS nodes,
                 (SELECT COUNT(*) FROM graph_edges WHERE collection = ?) AS edges,
                 (SELECT COUNT(*) FROM chunks c WHERE c.collection = ? AND EXISTS (SELECT 1 FROM graph_done d WHERE d.chunk_id = c.id)) AS done,
                 (SELECT COUNT(*) FROM chunks WHERE collection = ?) AS chunks",
        )
        .bind(collection)
        .bind(collection)
        .bind(collection)
        .bind(collection)
        .fetch_one(&self.pool)
        .await?;
        let done = row.try_get::<i64, _>("done")? as u64;
        let chunks = row.try_get::<i64, _>("chunks")? as u64;
        Ok(GraphStats {
            nodes: row.try_get::<i64, _>("nodes")? as u64,
            edges: row.try_get::<i64, _>("edges")? as u64,
            chunks_done: done,
            chunks_pending: chunks.saturating_sub(done),
        })
    }

    async fn graph_clear(&self, collection: &str) -> Result<()> {
        let mut tx = self.write_tx().await?;
        sqlx::query(
            "DELETE FROM graph_done WHERE chunk_id IN (SELECT id FROM chunks WHERE collection = ?)",
        )
        .bind(collection)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM graph_nodes WHERE collection = ?")
            .bind(collection)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}
