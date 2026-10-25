//! Tools for whole documents: summaries, comparisons, extracting fields
//! into a table, and writing Markdown, Word or PDF files. Documents are
//! knowledge documents (by id, source or title) or files in the workspace.

use std::sync::Arc;

use gai_core::Usage;
use gai_llm::ModelRegistry;
use gai_memory::docs::{self, Length, Spent};
use gai_memory::{Access, Knowledge};
use gai_store::{NewBlob, SharedStore};
use gai_tools::{SharedTool, ToolCtx, ToolError, ToolOutput, typed_tool};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

/// The tools this module makes, built per run like `rag_search`.
pub(crate) const DOCUMENT_TOOLS: [&str; 4] = [
    "summarize_document",
    "compare_documents",
    "extract_fields",
    "write_document",
];

fn failed(e: impl std::fmt::Display) -> ToolError {
    ToolError::Failed(e.to_string())
}

#[derive(Clone)]
struct Ctx {
    knowledge: Arc<Knowledge>,
    models: Arc<ModelRegistry>,
    store: SharedStore,
    model: String,
    /// The agent's knowledge collections; empty means all of them.
    collections: Vec<String>,
}

impl Ctx {
    async fn collections(&self) -> Result<Vec<String>, ToolError> {
        if !self.collections.is_empty() {
            return Ok(self.collections.clone());
        }
        Ok(self
            .store
            .list_collections()
            .await
            .map_err(failed)?
            .into_iter()
            .map(|c| c.name)
            .filter(|n| !n.contains(':'))
            .collect())
    }

    /// A document's title and text: from a collection, a workspace file,
    /// or the first collection that has a document by that name.
    async fn open(
        &self,
        tc: &ToolCtx,
        access: Option<&Access>,
        collection: Option<&str>,
        document: &str,
    ) -> Result<(String, String), ToolError> {
        let allowed = self.collections().await?;
        if let Some(c) = collection {
            if !allowed.iter().any(|a| a == c) {
                return Err(ToolError::InvalidInput(format!(
                    "unknown collection {c:?}; available: {}",
                    allowed.join(", ")
                )));
            }
            return self.in_collection(access, c, document).await;
        }
        if let Ok(path) = tc.resolve_path(document)
            && path.is_file()
        {
            let bytes = std::fs::read(&path).map_err(failed)?;
            let loaded = self
                .knowledge
                .load(document, None, bytes)
                .await
                .map_err(failed)?;
            return Ok((
                loaded.title.unwrap_or_else(|| document.to_owned()),
                loaded.text,
            ));
        }
        for c in &allowed {
            if let Ok(found) = self.in_collection(access, c, document).await {
                return Ok(found);
            }
        }
        Err(ToolError::InvalidInput(format!(
            "no document {document:?} in the workspace or in {}",
            allowed.join(", ")
        )))
    }

    async fn in_collection(
        &self,
        access: Option<&Access>,
        collection: &str,
        document: &str,
    ) -> Result<(String, String), ToolError> {
        let d = self
            .knowledge
            .find_document(collection, document)
            .await
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        if access.is_some_and(|a| !a.allows(&d.metadata["acl"])) {
            return Err(ToolError::InvalidInput(format!(
                "no document {document:?} in {collection}"
            )));
        }
        let text = self.knowledge.document_text(&d.id).await.map_err(failed)?;
        Ok((d.title.clone().unwrap_or(d.source), text))
    }

    async fn record(&self, tc: &ToolCtx, spent: &Spent) {
        if spent.usage == Usage::default() && spent.cost == 0.0 {
            return;
        }
        let run = (!tc.run_id.is_empty()).then_some(tc.run_id.as_str());
        if let Err(e) = self
            .store
            .record_usage(run, &self.model, &spent.usage, Some(spent.cost))
            .await
        {
            tracing::warn!(error = %e, "could not record document tool usage");
        }
    }
}

#[derive(Deserialize, JsonSchema)]
struct SummarizeArgs {
    /// The document: its title, source or id in knowledge, or a file path
    /// in the workspace.
    document: String,
    /// Only look in this knowledge collection.
    #[serde(default)]
    collection: Option<String>,
    /// What the summary should cover, e.g. "payment terms and deadlines".
    #[serde(default)]
    focus: Option<String>,
    #[serde(default)]
    length: Option<Length>,
}

#[derive(Deserialize, JsonSchema)]
struct DocArg {
    document: String,
    #[serde(default)]
    collection: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct CompareArgs {
    /// The first document (title, source, id, or workspace path).
    a: DocArg,
    /// The second document.
    b: DocArg,
    /// What matters most, e.g. "liability and termination".
    #[serde(default)]
    focus: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct ExtractArgs {
    /// The knowledge collection to read.
    collection: String,
    /// Only these documents (titles, sources or ids); default: all of them
    /// (up to 200).
    #[serde(default)]
    documents: Vec<String>,
    /// The fields: `{"name": "what it is"}`, or a JSON Schema with
    /// `properties`.
    fields: Value,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Format {
    #[default]
    Markdown,
    Docx,
    Pdf,
}

#[derive(Deserialize, JsonSchema)]
struct WriteArgs {
    /// The whole document in Markdown: headings, lists, tables, **bold**.
    content: String,
    /// markdown (default), docx or pdf.
    #[serde(default)]
    format: Format,
    /// The file name, without folders; defaults to one from the title.
    #[serde(default)]
    filename: Option<String>,
}

fn slug(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let out = out.trim_matches('-').chars().take(60).collect::<String>();
    if out.is_empty() {
        "document".into()
    } else {
        out
    }
}

/// The document tools named in `wanted`, using `model` for their model
/// calls.
pub(crate) fn document_tools(
    knowledge: Arc<Knowledge>,
    models: Arc<ModelRegistry>,
    store: SharedStore,
    model: Option<String>,
    collections: Vec<String>,
    wanted: &[&str],
) -> Result<Vec<SharedTool>, String> {
    let needs_model = wanted.iter().any(|w| *w != "write_document");
    let model = match model {
        Some(m) => m,
        None if needs_model => {
            return Err(format!(
                "{} need a model: give the agent one or set a default model",
                wanted.join(", ")
            ));
        }
        None => String::new(),
    };
    let ctx = Ctx {
        knowledge,
        models,
        store,
        model,
        collections,
    };
    let mut out: Vec<SharedTool> = Vec::new();
    if wanted.contains(&"summarize_document") {
        let c = ctx.clone();
        out.push(Arc::new(
            typed_tool(
                "summarize_document",
                "Summarize a whole document, however long: a knowledge document (by title, source or id) or a file in the workspace (PDFs, Word-exported text, scans read by OCR). Give a focus to get what matters.",
                move |tc, a: SummarizeArgs| {
                    let c = c.clone();
                    async move {
                        let access = gai_memory::tools::run_access(&c.store, &tc.run_id).await;
                        let (title, text) = c.open(&tc, access.as_ref(), a.collection.as_deref(), &a.document).await?;
                        let mut spent = Spent::default();
                        let r = docs::summarize(
                            &c.models,
                            &c.model,
                            &title,
                            &text,
                            a.focus.as_deref(),
                            a.length.unwrap_or_default(),
                            &mut spent,
                        )
                        .await;
                        c.record(&tc, &spent).await;
                        Ok(ToolOutput::text(format!("# Summary of {title}\n\n{}", r.map_err(failed)?)))
                    }
                },
            )
            .idempotent(),
        ));
    }
    if wanted.contains(&"compare_documents") {
        let c = ctx.clone();
        out.push(Arc::new(
            typed_tool(
                "compare_documents",
                "Compare two documents (knowledge documents or workspace files): what is the same, what differs with both wordings, and what only one has. Good for contract versions and competing offers.",
                move |tc, a: CompareArgs| {
                    let c = c.clone();
                    async move {
                        let access = gai_memory::tools::run_access(&c.store, &tc.run_id).await;
                        let da = c.open(&tc, access.as_ref(), a.a.collection.as_deref(), &a.a.document).await?;
                        let db = c.open(&tc, access.as_ref(), a.b.collection.as_deref(), &a.b.document).await?;
                        let mut spent = Spent::default();
                        let r = docs::compare(
                            &c.models,
                            &c.model,
                            (&da.0, &da.1),
                            (&db.0, &db.1),
                            a.focus.as_deref(),
                            &mut spent,
                        )
                        .await;
                        c.record(&tc, &spent).await;
                        Ok(ToolOutput::text(r.map_err(failed)?))
                    }
                },
            )
            .idempotent(),
        ));
    }
    if wanted.contains(&"extract_fields") {
        let c = ctx.clone();
        out.push(Arc::new(
            typed_tool(
                "extract_fields",
                "Read fields out of every document in a knowledge collection (or the ones named) into a table: e.g. {\"party\": \"who the other party is\", \"end_date\": \"when it ends\"}. Returns the table and saves it as a CSV file in the workspace.",
                move |tc, a: ExtractArgs| {
                    let c = c.clone();
                    async move {
                        if !c.collections().await?.contains(&a.collection) {
                            return Err(ToolError::InvalidInput(format!("unknown collection {:?}", a.collection)));
                        }
                        let schema = docs::fields_schema(&a.fields).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
                        let docs = if a.documents.is_empty() {
                            c.store.list_documents(&a.collection, 200, 0).await.map_err(failed)?
                        } else {
                            let mut v = Vec::new();
                            for d in &a.documents {
                                v.push(
                                    c.knowledge
                                        .find_document(&a.collection, d)
                                        .await
                                        .map_err(|e| ToolError::InvalidInput(e.to_string()))?,
                                );
                            }
                            v
                        };
                        let access = gai_memory::tools::run_access(&c.store, &tc.run_id).await;
                        let mut spent = Spent::default();
                        let rows = docs::extract(&c.knowledge, &c.models, &c.model, docs, &schema, access.as_ref(), &mut spent).await;
                        c.record(&tc, &spent).await;
                        let csv = docs::rows_csv(&rows, &schema);
                        let rel = format!("documents/{}-fields.csv", slug(&a.collection));
                        let saved = tc
                            .resolve_path(&rel)
                            .ok()
                            .and_then(|p| {
                                std::fs::create_dir_all(p.parent()?).ok()?;
                                std::fs::write(&p, &csv).ok()
                            })
                            .map(|_| format!("\n\nSaved as {rel}."))
                            .unwrap_or_default();
                        Ok(ToolOutput::text(format!(
                            "{} documents:\n\n{}{saved}",
                            rows.len(),
                            docs::rows_markdown(&rows, &schema)
                        )))
                    }
                },
            )
            .idempotent(),
        ));
    }
    if wanted.contains(&"write_document") {
        let store = ctx.store.clone();
        out.push(Arc::new(typed_tool(
            "write_document",
            "Write a finished document (report, letter, summary, table) from Markdown and save it as a file the user can download: markdown, docx (Word) or pdf.",
            move |tc, a: WriteArgs| {
                let store = store.clone();
                async move {
                    if a.content.trim().is_empty() {
                        return Err(ToolError::InvalidInput("content is empty".into()));
                    }
                    let blocks = crate::render::parse(&a.content);
                    let title = crate::render::title_of(&blocks, "Document");
                    let (bytes, media_type, ext) = match a.format {
                        Format::Markdown => (a.content.clone().into_bytes(), "text/markdown", "md"),
                        Format::Docx => (
                            crate::render::docx(&a.content, &title).map_err(failed)?,
                            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                            "docx",
                        ),
                        Format::Pdf => (crate::render::pdf(&a.content, &title), "application/pdf", "pdf"),
                    };
                    let stem = a
                        .filename
                        .as_deref()
                        .map(|f| f.rsplit(['/', '\\']).next().unwrap_or(f))
                        .map(|f| f.rsplit_once('.').map(|(s, _)| s).unwrap_or(f).to_owned())
                        .unwrap_or_else(|| slug(&title));
                    let rel = format!("documents/{}.{ext}", slug(&stem));
                    let path = tc.resolve_path(&rel)?;
                    if let Some(dir) = path.parent() {
                        std::fs::create_dir_all(dir).map_err(failed)?;
                    }
                    std::fs::write(&path, &bytes).map_err(failed)?;
                    let blob = store
                        .put_blob(NewBlob {
                            media_type: media_type.into(),
                            run_id: (!tc.run_id.is_empty()).then(|| tc.run_id.clone()),
                            source: Some("write_document".into()),
                            metadata: json!({"title": title, "filename": format!("{}.{ext}", slug(&stem))}),
                            data: bytes.clone(),
                            ..Default::default()
                        })
                        .await
                        .map_err(failed)?;
                    Ok(ToolOutput::text(format!(
                        "Wrote {title:?} as {rel} ({} bytes, file {}). The user can download it from this chat.",
                        bytes.len(),
                        blob.id
                    )))
                }
            },
        )));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use gai_llm::{
        LlmConfig, LlmError, MockModel, ModelInfo, Provider, SharedChatModel, text_response,
    };
    use gai_memory::{DocumentInput, KnowledgeSettings, NewCollection};
    use gai_store::SqliteStore;

    use super::*;

    struct Scripted(Arc<MockModel>);

    #[async_trait]
    impl Provider for Scripted {
        fn name(&self) -> &str {
            "script"
        }
        fn chat(&self, _info: ModelInfo) -> Result<SharedChatModel, LlmError> {
            Ok(self.0.clone())
        }
    }

    async fn setup(
        script: Arc<MockModel>,
    ) -> (Vec<SharedTool>, ToolCtx, tempfile::TempDir, SharedStore) {
        let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
        let models = Arc::new(
            ModelRegistry::builder()
                .config(LlmConfig::default())
                .provider("script", Arc::new(Scripted(script)))
                .build()
                .unwrap(),
        );
        let k = Arc::new(Knowledge::new(
            store.clone(),
            models.clone(),
            KnowledgeSettings::default(),
        ));
        k.create_collection(NewCollection {
            name: "contracts".into(),
            ..Default::default()
        })
        .await
        .unwrap();
        for (source, text) in [
            (
                "acme-2024.md",
                "# Acme supply agreement 2024\n\nTerm: 12 months. Fee: 4,000 EUR per month. Governing law: Delaware.",
            ),
            (
                "acme-2025.md",
                "# Acme supply agreement 2025\n\nTerm: 24 months. Fee: 4,500 EUR per month. Governing law: Delaware.",
            ),
        ] {
            k.ingest(
                "contracts",
                DocumentInput {
                    source: source.into(),
                    title: text
                        .lines()
                        .next()
                        .map(|l| l.trim_start_matches("# ").to_owned()),
                    text: text.into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        }
        let tools = document_tools(
            k,
            models,
            store.clone(),
            Some("script/m".into()),
            vec![],
            &DOCUMENT_TOOLS,
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolCtx::for_workspace(dir.path());
        (tools, ctx, dir, store)
    }

    fn tool<'a>(tools: &'a [SharedTool], name: &str) -> &'a SharedTool {
        tools.iter().find(|t| t.spec().name == name).unwrap()
    }

    fn text(o: &ToolOutput) -> String {
        o.content.iter().filter_map(|b| b.as_text()).collect()
    }

    #[tokio::test]
    async fn summarize_compare_extract_and_write() {
        let script = Arc::new(MockModel::scripted(vec![
            text_response("A 12-month supply deal at 4,000 EUR a month."),
            text_response("## Different\n| topic | A | B |\n| term | 12 months | 24 months |"),
            text_response(r#"{"term": "12 months", "fee": "4,000 EUR"}"#),
            text_response(r#"{"term": "24 months", "fee": "4,500 EUR", "extra": 1}"#),
        ]));
        let (tools, ctx, dir, _store) = setup(script.clone()).await;

        let out = tool(&tools, "summarize_document")
            .call(
                &ctx,
                json!({"document": "acme supply agreement 2024", "focus": "fees"}),
            )
            .await
            .unwrap();
        assert!(text(&out).contains("4,000 EUR a month"));
        let asked = script.requests().lock().unwrap()[0].clone();
        let prompt: String = asked.messages[0]
            .content
            .iter()
            .filter_map(|b| b.as_text())
            .collect();
        assert!(
            prompt.contains("Fee: 4,000 EUR") && prompt.contains("Focus on: fees"),
            "{prompt}"
        );

        let out = tool(&tools, "compare_documents")
            .call(&ctx, json!({"a": {"document": "acme-2024.md"}, "b": {"document": "acme-2025", "collection": "contracts"}}))
            .await
            .unwrap();
        assert!(text(&out).contains("24 months"));

        let out = tool(&tools, "extract_fields")
            .call(&ctx, json!({"collection": "contracts", "fields": {"term": "how long it runs", "fee": "the monthly fee"}}))
            .await
            .unwrap();
        let t = text(&out);
        assert!(
            t.contains("2 documents") && t.contains("| fee | term |") && t.contains("24 months"),
            "{t}"
        );
        let csv =
            std::fs::read_to_string(dir.path().join("documents/contracts-fields.csv")).unwrap();
        assert_eq!(csv.lines().count(), 3, "{csv}");
        assert!(!csv.contains("extra"), "only the asked fields: {csv}");

        // A workspace file works as a document too; unknown ones don't.
        std::fs::write(
            dir.path().join("notes.md"),
            "# Notes\n\nCall Acme on Monday.",
        )
        .unwrap();
        let err = tool(&tools, "summarize_document")
            .call(&ctx, json!({"document": "missing.pdf"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no document"), "{err}");

        for (format, ext, magic) in [
            ("pdf", "pdf", b"%PDF".as_slice()),
            ("docx", "docx", b"PK".as_slice()),
            ("markdown", "md", b"# ".as_slice()),
        ] {
            let out = tool(&tools, "write_document")
                .call(
                    &ctx,
                    json!({"content": "# Renewal memo\n\n- Term doubles\n", "format": format}),
                )
                .await
                .unwrap();
            assert!(
                text(&out).contains(&format!("documents/renewal-memo.{ext}")),
                "{}",
                text(&out)
            );
            let bytes =
                std::fs::read(dir.path().join(format!("documents/renewal-memo.{ext}"))).unwrap();
            assert!(bytes.starts_with(magic), "{format}");
        }
    }

    #[test]
    fn needs_a_model_except_for_writing() {
        let models = Arc::new(
            ModelRegistry::builder()
                .config(LlmConfig::default())
                .build()
                .unwrap(),
        );
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let store: SharedStore =
            rt.block_on(async { Arc::new(SqliteStore::temporary().await.unwrap()) as SharedStore });
        let k = Arc::new(Knowledge::new(
            store.clone(),
            models.clone(),
            KnowledgeSettings::default(),
        ));
        assert!(
            document_tools(
                k.clone(),
                models.clone(),
                store.clone(),
                None,
                vec![],
                &["summarize_document"]
            )
            .is_err()
        );
        assert_eq!(
            document_tools(k, models, store, None, vec![], &["write_document"])
                .unwrap()
                .len(),
            1
        );
    }
}
