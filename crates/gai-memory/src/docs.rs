//! Working with whole documents: finding one, its full text, summaries,
//! comparisons, and fields extracted into a table.

use std::sync::Arc;

use futures::StreamExt;
use gai_core::Usage;
use gai_llm::{ChatRequest, ModelRegistry, ResponseFormat};
use gai_store::Document;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{Access, Knowledge, MemoryError, Result};

/// About 3,000 tokens: the piece of a long document summarized at once.
const PIECE_CHARS: usize = 12_000;
/// Documents up to this long are read whole for comparisons and extraction.
const WHOLE_CHARS: usize = 40_000;

/// What model calls cost, for the caller to record.
#[derive(Clone, Debug, Default)]
pub struct Spent {
    pub usage: Usage,
    pub cost: f64,
}

/// A document's text from its chunks, dropping the text consecutive chunks
/// share (the chunk overlap).
pub fn rejoin_chunks<'a>(chunks: impl Iterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for c in chunks {
        if out.is_empty() {
            out.push_str(c);
            continue;
        }
        let max = c.len().min(out.len()).min(4000);
        let overlap = (1..=max)
            .rev()
            .find(|&k| c.is_char_boundary(k) && out.ends_with(&c[..k]) && k >= 8)
            .unwrap_or(0);
        if overlap == 0 {
            out.push_str("\n\n");
        }
        out.push_str(&c[overlap..]);
    }
    out
}

impl Knowledge {
    /// A document in `collection` by id, source or title: exact matches
    /// first, then a source ending with it, then one containing it.
    pub async fn find_document(&self, collection: &str, reference: &str) -> Result<Document> {
        self.collection(collection).await?;
        let r = reference.trim();
        if let Some(d) = self
            .store()
            .get_document(r)
            .await?
            .filter(|d| d.collection == collection)
        {
            return Ok(d);
        }
        let docs = self.store().list_documents(collection, 10_000, 0).await?;
        let lower = r.to_lowercase();
        let pick = docs
            .iter()
            .find(|d| d.source == r || d.title.as_deref() == Some(r))
            .or_else(|| {
                docs.iter().find(|d| {
                    d.title
                        .as_deref()
                        .is_some_and(|t| t.eq_ignore_ascii_case(r))
                })
            })
            .or_else(|| {
                docs.iter()
                    .find(|d| d.source.to_lowercase().ends_with(&lower))
            })
            .or_else(|| {
                docs.iter()
                    .find(|d| d.source.to_lowercase().contains(&lower))
            })
            .or_else(|| {
                docs.iter().find(|d| {
                    d.title
                        .as_deref()
                        .is_some_and(|t| t.to_lowercase().contains(&lower))
                })
            });
        pick.cloned()
            .ok_or_else(|| MemoryError::NotFound(format!("no document {r:?} in {collection}")))
    }

    /// A document's whole text, rebuilt from its chunks.
    pub async fn document_text(&self, document_id: &str) -> Result<String> {
        let chunks = self.store().document_chunks(document_id).await?;
        Ok(rejoin_chunks(chunks.iter().map(|c| c.text.as_str())))
    }
}

async fn ask(
    models: &Arc<ModelRegistry>,
    model: &str,
    system: &str,
    prompt: String,
    json: bool,
    spent: &mut Spent,
) -> Result<String> {
    let m = models.chat(model)?;
    let mut req = ChatRequest::user(prompt).with_system(system);
    req.temperature = Some(0.2);
    req.max_tokens = Some(4096);
    if json {
        req.response_format = Some(ResponseFormat::Json);
    }
    let resp = m.complete(req).await?;
    spent.usage += resp.usage;
    spent.cost += m.info().cost(&resp.usage).unwrap_or(0.0);
    Ok(resp.text())
}

fn pieces(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for para in text.split("\n\n") {
        if !cur.is_empty() && cur.len() + para.len() > PIECE_CHARS {
            out.push(std::mem::take(&mut cur));
        }
        if para.len() > PIECE_CHARS {
            // One huge paragraph: cut it at character boundaries.
            let chars: Vec<char> = para.chars().collect();
            for c in chars.chunks(PIECE_CHARS) {
                out.push(c.iter().collect());
            }
            continue;
        }
        if !cur.is_empty() {
            cur.push_str("\n\n");
        }
        cur.push_str(para);
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// How long a summary should be.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Length {
    /// A few sentences.
    Short,
    #[default]
    Medium,
    /// A page, with sections.
    Long,
}

impl Length {
    fn words(self) -> &'static str {
        match self {
            Length::Short => "at most 80 words",
            Length::Medium => "about 250 words",
            Length::Long => "about 700 words, in sections with headings",
        }
    }
}

const SUMMARY_SYSTEM: &str = "You summarize documents faithfully. Keep names, dates, amounts and obligations exact; never add facts that aren't in the text. Write in Markdown.";

/// A summary of `text`: at once when it is short, else each part first
/// and then those summaries together.
pub async fn summarize(
    models: &Arc<ModelRegistry>,
    model: &str,
    title: &str,
    text: &str,
    focus: Option<&str>,
    length: Length,
    spent: &mut Spent,
) -> Result<String> {
    let focus = focus
        .map(|f| format!(" Focus on: {f}."))
        .unwrap_or_default();
    let parts = pieces(text);
    if parts.len() <= 1 {
        return ask(
            models,
            model,
            SUMMARY_SYSTEM,
            format!(
                "Summarize this document in {}.{focus}\n\n# {title}\n\n{text}",
                length.words()
            ),
            false,
            spent,
        )
        .await;
    }
    let n = parts.len();
    let mut notes = Vec::with_capacity(n);
    let results: Vec<(usize, Result<(String, Spent)>)> = futures::stream::iter(parts.into_iter().enumerate())
        .map(|(i, p)| {
            let (models, model, title, focus) = (models.clone(), model.to_owned(), title.to_owned(), focus.clone());
            async move {
                let mut s = Spent::default();
                let r = ask(
                    &models,
                    &model,
                    SUMMARY_SYSTEM,
                    format!(
                        "This is part {} of {n} of {title}. Note its key points as a list, keeping every name, date, amount and obligation.{focus}\n\n{p}",
                        i + 1
                    ),
                    false,
                    &mut s,
                )
                .await;
                (i, r.map(|t| (t, s)))
            }
        })
        .buffer_unordered(4)
        .collect()
        .await;
    let mut results = results;
    results.sort_by_key(|(i, _)| *i);
    for (i, r) in results {
        let (t, s) = r?;
        spent.usage += s.usage;
        spent.cost += s.cost;
        notes.push(format!("## Part {}\n{t}", i + 1));
    }
    ask(
        models,
        model,
        SUMMARY_SYSTEM,
        format!(
            "These are notes on the parts of {title}, in order. Write one summary of the whole document in {}.{focus}\n\n{}",
            length.words(),
            notes.join("\n\n")
        ),
        false,
        spent,
    )
    .await
}

/// A comparison of two documents: what matches, what differs (with both
/// wordings) and what only one has.
pub async fn compare(
    models: &Arc<ModelRegistry>,
    model: &str,
    a: (&str, &str),
    b: (&str, &str),
    focus: Option<&str>,
    spent: &mut Spent,
) -> Result<String> {
    // Long documents are compared through faithful summaries.
    let mut prepare = async |title: &str, text: &str| -> Result<String> {
        if text.len() <= WHOLE_CHARS {
            Ok(text.to_owned())
        } else {
            summarize(
                models,
                model,
                title,
                text,
                Some(focus.unwrap_or("every term that could differ: parties, dates, amounts, durations, obligations, conditions")),
                Length::Long,
                spent,
            )
            .await
        }
    };
    let ta = prepare(a.0, a.1).await?;
    let tb = prepare(b.0, b.1).await?;
    let focus = focus
        .map(|f| format!(" Pay most attention to: {f}."))
        .unwrap_or_default();
    ask(
        models,
        model,
        "You compare documents precisely. Quote the exact wording when terms differ. Never invent terms either document doesn't state. Write in Markdown.",
        format!(
            "Compare document A ({}) and document B ({}).{focus}\n\nAnswer with these sections:\n## In both\n## Different (a table: topic | A | B)\n## Only in A\n## Only in B\n\n# Document A: {}\n\n{ta}\n\n# Document B: {}\n\n{tb}",
            a.0, b.0, a.0, b.0
        ),
        false,
        spent,
    )
    .await
}

/// Fields to extract: a JSON Schema object, or `{"field": "what it is"}`.
pub fn fields_schema(fields: &Value) -> Result<Value> {
    let props = match fields.get("properties") {
        Some(p) => p.clone(),
        None => {
            let Some(map) = fields.as_object().filter(|m| !m.is_empty()) else {
                return Err(MemoryError::Invalid(
                    "fields: give a JSON Schema with properties, or {\"field\": \"what it is\"}"
                        .into(),
                ));
            };
            let mut props = Map::new();
            for (k, v) in map {
                props.insert(
                    k.clone(),
                    match v {
                        Value::String(d) => json!({"type": ["string", "null"], "description": d}),
                        Value::Object(_) => v.clone(),
                        _ => json!({"type": ["string", "null"]}),
                    },
                );
            }
            Value::Object(props)
        }
    };
    let Some(p) = props.as_object().filter(|p| !p.is_empty()) else {
        return Err(MemoryError::Invalid("fields: no properties".into()));
    };
    if p.len() > 50 {
        return Err(MemoryError::Invalid("fields: at most 50".into()));
    }
    Ok(json!({"type": "object", "properties": props}))
}

/// One document's extracted fields (`null` where the document doesn't say).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExtractedRow {
    pub document_id: String,
    pub source: String,
    pub title: Option<String>,
    pub fields: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The text a model reads to fill `schema` from a document: all of it when
/// it fits, else its start and the passages that best match each field.
async fn extraction_text(k: &Knowledge, doc: &Document, schema: &Value) -> Result<String> {
    let text = k.document_text(&doc.id).await?;
    if text.len() <= WHOLE_CHARS {
        return Ok(text);
    }
    let mut out: String = text.chars().take(WHOLE_CHARS / 2).collect();
    let props = schema["properties"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    for (name, p) in &props {
        let q = format!("{name} {}", p["description"].as_str().unwrap_or_default());
        let hits = k
            .search(
                std::slice::from_ref(&doc.collection),
                &q,
                40,
                crate::SearchMode::Hybrid,
            )
            .await
            .unwrap_or_default();
        for h in hits.into_iter().filter(|h| h.document_id == doc.id).take(3) {
            if seen.insert(h.chunk_id) {
                out.push_str("\n\n[…]\n\n");
                out.push_str(&h.text);
            }
        }
    }
    Ok(out)
}

/// Fill `schema` (see [`fields_schema`]) from each document, four at a time.
/// Documents `access` may not find are left out.
pub async fn extract(
    k: &Knowledge,
    models: &Arc<ModelRegistry>,
    model: &str,
    docs: Vec<Document>,
    schema: &Value,
    access: Option<&Access>,
    spent: &mut Spent,
) -> Vec<ExtractedRow> {
    let docs: Vec<Document> = docs
        .into_iter()
        .filter(|d| access.is_none_or(|a| a.allows(&d.metadata["acl"])))
        .collect();
    let props: Vec<String> = schema["properties"]
        .as_object()
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default();
    let results: Vec<(ExtractedRow, Spent)> = futures::stream::iter(docs)
        .map(|doc| {
            let (models, props) = (models.clone(), props.clone());
            async move {
                let mut s = Spent::default();
                let mut row = ExtractedRow {
                    document_id: doc.id.clone(),
                    source: doc.source.clone(),
                    title: doc.title.clone(),
                    fields: props.iter().map(|p| (p.clone(), Value::Null)).collect(),
                    error: None,
                };
                let r = async {
                    let text = extraction_text(k, &doc, schema).await?;
                    let answer = ask(
                        &models,
                        model,
                        "You extract fields from documents into JSON. Use only what the document states; use null for anything it doesn't. Copy names, dates and amounts exactly. Reply with only the JSON object.",
                        format!(
                            "Fill this JSON Schema from the document.\n\nSchema:\n{}\n\nDocument ({}):\n{text}",
                            serde_json::to_string_pretty(schema).unwrap_or_default(),
                            doc.title.as_deref().unwrap_or(&doc.source)
                        ),
                        true,
                        &mut s,
                    )
                    .await?;
                    let start = answer.find('{').unwrap_or(0);
                    let end = answer.rfind('}').map(|e| e + 1).unwrap_or(answer.len());
                    serde_json::from_str::<Map<String, Value>>(&answer[start..end])
                        .map_err(|_| MemoryError::Invalid(format!("the model didn't answer in JSON: {answer}")))
                }
                .await;
                match r {
                    Ok(v) => {
                        for p in &props {
                            if let Some(x) = v.get(p) {
                                row.fields.insert(p.clone(), x.clone());
                            }
                        }
                    }
                    Err(e) => row.error = Some(e.to_string()),
                }
                (row, s)
            }
        })
        .buffer_unordered(4)
        .collect()
        .await;
    let mut rows = Vec::with_capacity(results.len());
    for (row, s) in results {
        spent.usage += s.usage;
        spent.cost += s.cost;
        rows.push(row);
    }
    rows.sort_by(|a, b| a.source.cmp(&b.source));
    rows
}

/// Rows as CSV: source, title, then each field (lists and objects as JSON).
pub fn rows_csv(rows: &[ExtractedRow], schema: &Value) -> String {
    let props: Vec<String> = schema["properties"]
        .as_object()
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default();
    let cell = |s: &str| {
        if s.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_owned()
        }
    };
    let mut out = String::from("source,title");
    for p in &props {
        out.push(',');
        out.push_str(&cell(p));
    }
    out.push_str(",error\n");
    for r in rows {
        out.push_str(&cell(&r.source));
        out.push(',');
        out.push_str(&cell(r.title.as_deref().unwrap_or("")));
        for p in &props {
            out.push(',');
            let v = match r.fields.get(p) {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(s)) => s.clone(),
                Some(v) => v.to_string(),
            };
            out.push_str(&cell(&v));
        }
        out.push(',');
        out.push_str(&cell(r.error.as_deref().unwrap_or("")));
        out.push('\n');
    }
    out
}

/// Rows as a Markdown table, for a model to read.
pub fn rows_markdown(rows: &[ExtractedRow], schema: &Value) -> String {
    let props: Vec<String> = schema["properties"]
        .as_object()
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default();
    let esc = |s: &str| s.replace('|', "\\|").replace('\n', " ");
    let mut out = format!(
        "| document | {} |\n|",
        props.iter().map(|p| esc(p)).collect::<Vec<_>>().join(" | ")
    );
    for _ in 0..=props.len() {
        out.push_str(" --- |");
    }
    out.push('\n');
    for r in rows {
        let cells: Vec<String> = props
            .iter()
            .map(|p| match r.fields.get(p) {
                None | Some(Value::Null) => match &r.error {
                    Some(_) => "(error)".into(),
                    None => "—".into(),
                },
                Some(Value::String(s)) => esc(s),
                Some(v) => esc(&v.to_string()),
            })
            .collect();
        out.push_str(&format!(
            "| {} | {} |\n",
            esc(r.title.as_deref().unwrap_or(&r.source)),
            cells.join(" | ")
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejoins_overlapping_chunks() {
        let text = rejoin_chunks(
            [
                "The term is two years and",
                "two years and the fee is 4,000.",
            ]
            .into_iter(),
        );
        assert_eq!(text, "The term is two years and the fee is 4,000.");
        assert_eq!(rejoin_chunks(["a", "b"].into_iter()), "a\n\nb");
    }

    #[test]
    fn schemas_from_maps() {
        let s =
            fields_schema(&json!({"party": "Who signs", "amount": {"type": "number"}})).unwrap();
        assert_eq!(s["properties"]["party"]["description"], "Who signs");
        assert_eq!(s["properties"]["amount"]["type"], "number");
        let full = json!({"type": "object", "properties": {"x": {"type": "string"}}});
        assert_eq!(
            fields_schema(&full).unwrap()["properties"],
            full["properties"]
        );
        assert!(fields_schema(&json!({})).is_err());
        assert!(fields_schema(&json!("x")).is_err());
    }

    #[test]
    fn csv_and_markdown() {
        let schema = fields_schema(&json!({"party": "", "fee": ""})).unwrap();
        let rows = vec![ExtractedRow {
            document_id: "d1".into(),
            source: "a.pdf".into(),
            title: Some("Lease, \"main\"".into()),
            fields: [
                ("party".to_owned(), json!("Acme")),
                ("fee".to_owned(), json!(4200)),
            ]
            .into_iter()
            .collect(),
            error: None,
        }];
        let csv = rows_csv(&rows, &schema);
        assert!(csv.starts_with("source,title,"), "{csv}");
        assert!(csv.contains("a.pdf,\"Lease, \"\"main\"\"\""), "{csv}");
        assert!(csv.contains("4200"), "{csv}");
        let md = rows_markdown(&rows, &schema);
        assert!(md.contains("Acme"), "{md}");
    }

    #[test]
    fn long_text_splits_into_pieces() {
        let text = vec!["word ".repeat(400); 20].join("\n\n");
        let p = pieces(&text);
        assert!(
            p.len() > 1 && p.iter().all(|x| x.len() <= PIECE_CHARS + 10),
            "{}",
            p.len()
        );
        assert_eq!(pieces("short"), ["short"]);
    }
}
