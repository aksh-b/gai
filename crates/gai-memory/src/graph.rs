//! A knowledge graph per collection: a model reads the chunks for entities
//! and the relations between them, and graph search adds the passages that
//! mention what the question is about (and what that is connected to), plus
//! the connections themselves as facts.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gai_llm::{ChatRequest, ResponseFormat};
use gai_store::{GraphEdge, GraphNode, NewEntity, NewRelation};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::docs::Spent;
use crate::{Access, Knowledge, Result, SearchHit, SearchMode};

/// ```toml
/// [knowledge.graph]
/// model = "openai/gpt-4o-mini"   # reads chunks for entities (default: the default model)
/// ```
///
/// A collection created with `metadata: {graph: true}` builds its graph
/// after each ingest; any collection can be built on request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct GraphSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Text sent to the model at once (several chunks).
    pub batch_chars: usize,
}

impl Default for GraphSettings {
    fn default() -> Self {
        Self {
            model: None,
            batch_chars: 6000,
        }
    }
}

/// What a build did.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GraphBuild {
    pub chunks: u64,
    pub entities: u64,
    pub relations: u64,
    pub cost_usd: f64,
    /// Batches the model answered badly (skipped, marked read).
    pub failed: u64,
}

/// Graph search: passages, and the connections found among what the
/// question is about.
#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
pub struct GraphHits {
    pub hits: Vec<SearchHit>,
    /// `Acme —signed with→ Globex`, strongest first.
    pub facts: Vec<String>,
    pub entities: Vec<GraphNode>,
}

const KINDS: &[&str] = &[
    "person",
    "organization",
    "place",
    "product",
    "event",
    "concept",
    "date",
    "amount",
    "document",
    "other",
];

const PROMPT: &str = "You build a knowledge graph from text. List the important named entities (people, organizations, places, products, events, key concepts, and dates or amounts that matter) and the relations the text states between them. Use each entity's full, usual name, the same way every time. Relations are short verbs or phrases (\"signed with\", \"works for\", \"located in\", \"pays\"). Reply with only JSON: {\"entities\": [{\"name\": \"...\", \"type\": \"person|organization|place|product|event|concept|date|amount|document|other\", \"description\": \"one short line\"}], \"relations\": [{\"source\": \"...\", \"relation\": \"...\", \"target\": \"...\"}]}";

#[derive(Deserialize)]
struct Found {
    #[serde(default)]
    entities: Vec<FoundEntity>,
    #[serde(default)]
    relations: Vec<FoundRelation>,
}

#[derive(Deserialize)]
struct FoundEntity {
    name: String,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize)]
struct FoundRelation {
    source: String,
    relation: String,
    target: String,
}

fn parse_found(text: &str) -> Option<Found> {
    let start = text.find('{')?;
    let end = text.rfind('}')? + 1;
    serde_json::from_str(&text[start..end]).ok()
}

fn mentions(text: &str, name: &str) -> bool {
    !name.trim().is_empty() && text.to_lowercase().contains(&name.trim().to_lowercase())
}

const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "what", "who", "whom", "which", "when", "where", "how", "why",
    "does", "did", "are", "was", "were", "has", "have", "had", "about", "from", "that", "this",
    "these", "those", "into", "between", "our", "their", "they", "them", "there", "you", "your",
    "can", "could", "would", "should", "tell", "show", "list", "all", "any", "not",
];

impl Knowledge {
    /// The model a collection's graph is built with: `[knowledge.graph]`'s,
    /// when the collection asks for a graph (`metadata.graph`).
    pub async fn graph_model_for(&self, collection: &str) -> Option<String> {
        let c = self.collection(collection).await.ok()?;
        (c.metadata["graph"] == true).then(|| self.settings().graph.model.clone())?
    }

    /// Read the collection's unread chunks into its graph with `model`.
    /// One build per collection at a time; a second call waits for nothing
    /// and returns an empty build.
    pub async fn build_graph(&self, collection: &str, model: &str) -> Result<GraphBuild> {
        self.collection(collection).await?;
        if !self
            .graph_lock
            .lock()
            .expect("graph lock")
            .insert(collection.to_owned())
        {
            return Ok(GraphBuild::default());
        }
        let result = self.build_graph_unlocked(collection, model).await;
        self.graph_lock
            .lock()
            .expect("graph lock")
            .remove(collection);
        result
    }

    async fn build_graph_unlocked(&self, collection: &str, model: &str) -> Result<GraphBuild> {
        let m = self.models.chat(model)?;
        let mut out = GraphBuild::default();
        let mut spent = Spent::default();
        loop {
            let pending = self.store().graph_pending(collection, 100).await?;
            if pending.is_empty() {
                break;
            }
            // Pack chunks into batches of about `batch_chars`.
            let mut batches: Vec<Vec<gai_store::Chunk>> = vec![Vec::new()];
            let mut size = 0;
            for c in pending {
                if size > 0 && size + c.text.len() > self.settings().graph.batch_chars {
                    batches.push(Vec::new());
                    size = 0;
                }
                size += c.text.len();
                batches.last_mut().expect("a batch").push(c);
            }
            for batch in batches {
                let text: String = batch
                    .iter()
                    .map(|c| c.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n---\n\n");
                let mut req = ChatRequest::user(text.clone()).with_system(PROMPT);
                req.temperature = Some(0.0);
                req.max_tokens = Some(3000);
                req.response_format = Some(ResponseFormat::Json);
                let resp = m.complete(req).await?;
                spent.usage += resp.usage;
                spent.cost += m.info().cost(&resp.usage).unwrap_or(0.0);
                let ids: Vec<i64> = batch.iter().map(|c| c.id).collect();
                let Some(found) = parse_found(&resp.text()) else {
                    out.failed += 1;
                    self.store().graph_add(collection, &ids, &[], &[]).await?;
                    continue;
                };
                // Each entity is mentioned by the chunks that name it (the
                // batch's first, when the model reworded it).
                let entities: Vec<NewEntity> = found
                    .entities
                    .iter()
                    .filter(|e| !e.name.trim().is_empty() && e.name.len() <= 200)
                    .map(|e| {
                        let mut chunks: Vec<i64> = batch
                            .iter()
                            .filter(|c| mentions(&c.text, &e.name))
                            .map(|c| c.id)
                            .collect();
                        if chunks.is_empty() {
                            chunks.push(ids[0]);
                        }
                        let kind = e
                            .kind
                            .as_deref()
                            .map(str::to_lowercase)
                            .filter(|k| KINDS.contains(&k.as_str()))
                            .unwrap_or_else(|| "other".into());
                        NewEntity {
                            name: e.name.trim().to_owned(),
                            kind,
                            description: e.description.clone().filter(|d| !d.trim().is_empty()),
                            chunk_ids: chunks,
                        }
                    })
                    .collect();
                let relations: Vec<NewRelation> = found
                    .relations
                    .iter()
                    .filter(|r| r.relation.len() <= 80)
                    .map(|r| {
                        let chunk = batch
                            .iter()
                            .find(|c| mentions(&c.text, &r.source) && mentions(&c.text, &r.target))
                            .or_else(|| batch.iter().find(|c| mentions(&c.text, &r.source)))
                            .map(|c| c.id)
                            .unwrap_or(ids[0]);
                        NewRelation {
                            source: r.source.clone(),
                            target: r.target.clone(),
                            relation: r.relation.clone(),
                            chunk_id: chunk,
                        }
                    })
                    .collect();
                out.chunks += ids.len() as u64;
                out.entities += entities.len() as u64;
                out.relations += relations.len() as u64;
                self.store()
                    .graph_add(collection, &ids, &entities, &relations)
                    .await?;
            }
        }
        out.cost_usd = spent.cost;
        Ok(out)
    }

    /// Hybrid search, joined (by rank) with the passages that mention the
    /// entities the question names or the top passages mention, and those
    /// entities' neighbours; plus the relations among them as facts.
    pub async fn search_graph(
        &self,
        collections: &[String],
        query: &str,
        k: usize,
        access: Option<&Access>,
    ) -> Result<GraphHits> {
        let k = k.clamp(1, 100);
        let hits = self
            .search_as(collections, query, k, SearchMode::Hybrid, access)
            .await?;
        let words: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric())
            .map(str::to_lowercase)
            .filter(|w| w.len() >= 3 && !STOPWORDS.contains(&w.as_str()))
            .collect();
        let mut seeds: Vec<GraphNode> = Vec::new();
        let mut names: HashMap<i64, String> = HashMap::new();
        let mut edges: Vec<GraphEdge> = Vec::new();
        // The seeds and their neighbours.
        let mut expanded: Vec<i64> = Vec::new();
        for c in collections {
            let mut found: Vec<GraphNode> = Vec::new();
            for w in &words {
                found.extend(self.store().graph_nodes(c, w, 5).await?);
            }
            // Entities in the best passages count too, but less.
            let hit_chunks: Vec<i64> = hits
                .iter()
                .filter(|h| &h.collection == c)
                .take(3)
                .map(|h| h.chunk_id)
                .collect();
            let from_hits = self.store().graph_chunk_nodes(&hit_chunks).await?;
            found.extend(
                self.store()
                    .graph_nodes_by_id(&from_hits)
                    .await?
                    .into_iter()
                    .take(8),
            );
            let mut seen = HashSet::new();
            found.retain(|n| seen.insert(n.id));
            found.truncate(12);
            if found.is_empty() {
                continue;
            }
            let ids: Vec<i64> = found.iter().map(|n| n.id).collect();
            let around = self.store().graph_edges(c, &ids, 60).await?;
            let mut all: HashSet<i64> = ids.iter().copied().collect();
            for e in &around {
                all.insert(e.source);
                all.insert(e.target);
            }
            let all: Vec<i64> = all.into_iter().collect();
            expanded.extend(ids.iter().copied());
            expanded.extend(all.iter().filter(|i| !ids.contains(i)).take(20));
            for n in self.store().graph_nodes_by_id(&all).await? {
                names.insert(n.id, n.name);
            }
            seeds.extend(found);
            edges.extend(around);
        }
        if seeds.is_empty() {
            return Ok(GraphHits {
                hits,
                ..Default::default()
            });
        }
        // Passages that mention the most of the seeds and their neighbours.
        let ranked = self.store().graph_chunks(&expanded, (k * 3) as u32).await?;
        let scored: Vec<(i64, f32)> = ranked.iter().map(|(id, n)| (*id, *n as f32)).collect();
        let graph_hits: Vec<gai_store::ChunkHit> = self
            .store()
            .chunk_hits(scored)
            .await?
            .into_iter()
            .filter(|h| collections.contains(&h.chunk.collection))
            .filter(|h| access.is_none_or(|a| a.allows(&h.chunk.metadata["acl"])))
            .collect();
        // Reciprocal rank fusion of the two lists.
        let mut score: HashMap<i64, f32> = HashMap::new();
        let mut by_id: HashMap<i64, SearchHit> = HashMap::new();
        for (rank, h) in hits.into_iter().enumerate() {
            *score.entry(h.chunk_id).or_default() += 1.0 / (60.0 + rank as f32 + 1.0);
            by_id.insert(h.chunk_id, h);
        }
        // A passage linked through the question's entities is about them,
        // so the graph's list counts a little more than plain search.
        for (rank, h) in graph_hits.into_iter().enumerate() {
            *score.entry(h.chunk.id).or_default() += 1.2 / (60.0 + rank as f32 + 1.0);
            by_id
                .entry(h.chunk.id)
                .or_insert_with(|| crate::knowledge::to_hit(h));
        }
        let mut out: Vec<SearchHit> = by_id
            .into_values()
            .map(|mut h| {
                h.score = score[&h.chunk_id];
                h
            })
            .collect();
        out.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then(a.chunk_id.cmp(&b.chunk_id))
        });
        out.truncate(k);
        let mut seen = HashSet::new();
        edges.retain(|e| seen.insert((e.source, e.target, e.relation.clone())));
        edges.sort_by_key(|e| std::cmp::Reverse(e.weight));
        // Someone who may not see every document only gets connections the
        // passages they can see bear out.
        let visible: Vec<String> = out.iter().map(|h| h.text.to_lowercase()).collect();
        let shown = |a: &str, b: &str| {
            access.is_none()
                || visible
                    .iter()
                    .any(|t| t.contains(&a.to_lowercase()) && t.contains(&b.to_lowercase()))
        };
        let facts = edges
            .iter()
            .filter_map(|e| {
                let (a, b) = (names.get(&e.source)?, names.get(&e.target)?);
                shown(a, b).then(|| format!("{a} —{}→ {b}", e.relation))
            })
            .take(15)
            .collect();
        if access.is_some() {
            seeds.retain(|n| visible.iter().any(|t| t.contains(&n.name.to_lowercase())));
        }
        Ok(GraphHits {
            hits: out,
            facts,
            entities: seeds,
        })
    }
}

/// Build `collection`'s graph in the background when it asks for one
/// (`metadata.graph`) and a model is set.
pub fn build_in_background(k: Arc<Knowledge>, collection: String) {
    tokio::spawn(async move {
        let Some(model) = k.graph_model_for(&collection).await else {
            return;
        };
        match k.build_graph(&collection, &model).await {
            Ok(b) if b.chunks > 0 => {
                tracing::info!(
                    collection,
                    chunks = b.chunks,
                    entities = b.entities,
                    relations = b.relations,
                    "knowledge graph updated"
                )
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(collection, error = %e, "could not build the knowledge graph"),
        }
    });
}

/// The facts after search results, for a model.
pub fn format_facts(facts: &[String]) -> String {
    if facts.is_empty() {
        return String::new();
    }
    format!(
        "\n\nConnections in the knowledge graph:\n{}",
        facts
            .iter()
            .map(|f| format!("- {f}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_what_models_return() {
        let f = parse_found("Here: {\"entities\": [{\"name\": \"Acme\", \"type\": \"organization\"}], \"relations\": [{\"source\": \"Acme\", \"relation\": \"owns\", \"target\": \"Globex\"}]} done").unwrap();
        assert_eq!(f.entities[0].name, "Acme");
        assert_eq!(f.relations[0].target, "Globex");
        assert!(parse_found("no json").is_none());
        assert!(mentions("ACME signed", "acme"));
        assert!(!mentions("x", " "));
    }
}
