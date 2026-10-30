//! `.gaibundle` files: assistants and everything they need, in one zip, to
//! share or move between gai servers.
//!
//! ```text
//! bundle.toml                    what is inside, and what it needs (env vars, models)
//! agents/<name>.yaml
//! workflows/<name>.yaml
//! triggers/<name>.yaml           schedules, webhooks and file watchers (imported turned off)
//! personas/<name>.yaml
//! prompts/<name>.yaml           prompt templates (latest version), labels in prompts/<name>.labels
//! skills/<name>/SKILL.md         plus the skill's files
//! mcp/<name>.json                MCP server configs, secrets replaced by ${VARS}
//! memory/blocks.json             memory blocks the agents start with
//! knowledge/<name>/collection.json
//! knowledge/<name>/<n>.md        the collection's documents, re-embedded on import
//! ```
//!
//! Secrets never go into a bundle: literal header and environment values of
//! MCP servers become `${VAR}` references, and `bundle.toml` lists the
//! variables (and the provider keys the agents' models need) so the
//! importer can set them.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Write};

use gai_agent::AgentSpec;
use gai_store::NewBlock;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Error, Gai};

/// The bundle format this build writes and reads.
pub const FORMAT: u32 = 1;
/// Largest bundle accepted, and largest single file in one.
const MAX_BUNDLE: u64 = 200 << 20;
const MAX_FILE: u64 = 50 << 20;

/// What to put in a bundle.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct ExportOptions {
    /// Shown when the bundle is imported.
    pub name: Option<String>,
    pub description: Option<String>,
    /// Stored agents (latest versions).
    pub agents: Vec<String>,
    pub workflows: Vec<String>,
    pub personas: Vec<String>,
    /// Prompt templates.
    pub prompts: Vec<String>,
    pub skills: Vec<String>,
    /// MCP servers added through the API.
    pub mcp_servers: Vec<String>,
    /// Knowledge collections, with their documents.
    pub knowledge: Vec<String>,
    /// Triggers: schedules, webhooks and file watchers, with what they run.
    pub triggers: Vec<String>,
    /// Everything stored (knowledge only when also listed or `knowledge_all`).
    pub all: bool,
    /// With `all`: every knowledge collection too.
    pub knowledge_all: bool,
    /// Leave out what the chosen items use (delegates, personas, skills,
    /// knowledge, MCP servers, agents in workflows).
    pub no_dependencies: bool,
    /// Mask personal data (emails, phone and card numbers, ...) in memory
    /// blocks and knowledge documents.
    pub redact_pii: bool,
}

/// `bundle.toml`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Manifest {
    pub format: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The gai version that wrote the bundle.
    pub gai_version: String,
    pub created_at: String,
    #[serde(default)]
    pub contents: Contents,
    #[serde(default)]
    pub requires: Requires,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct Contents {
    pub agents: Vec<String>,
    pub workflows: Vec<String>,
    pub personas: Vec<String>,
    pub prompts: Vec<String>,
    pub skills: Vec<String>,
    pub mcp_servers: Vec<String>,
    pub knowledge: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<String>,
    pub memory_blocks: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct Requires {
    /// Environment variables the bundle's items read (API keys, tokens).
    pub env: Vec<EnvVar>,
    /// Models the agents name.
    pub models: Vec<String>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct EnvVar {
    pub name: String,
    /// What needs it.
    pub used_by: String,
}

/// How to import.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct ImportOptions {
    /// Leave items that already exist alone. Otherwise an import saves a
    /// new version of them (nothing is lost: earlier versions stay).
    pub skip_existing: bool,
}

/// What an import did.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ImportReport {
    pub manifest: Manifest,
    /// `kind/name` (and `@vN` for versioned items) of what was saved.
    pub installed: Vec<String>,
    /// Items left alone because they exist (with `skip_existing`).
    pub skipped: Vec<String>,
    /// Required environment variables that are not set on this server.
    pub missing_env: Vec<EnvVar>,
    /// Models the agents use that this server can't reach.
    pub missing_models: Vec<String>,
    pub warnings: Vec<String>,
}

fn invalid(m: impl Into<String>) -> Error {
    Error::Bundle(m.into())
}

/// Write a bundle with the chosen items.
pub async fn export(gai: &Gai, opts: &ExportOptions) -> Result<Vec<u8>, Error> {
    let rt = gai.runtime().await?;
    let store = rt.store();
    let mut want = Selection::default();
    if opts.all {
        want.agents
            .extend(store.list_agents().await?.into_iter().map(|a| a.name));
        want.workflows
            .extend(store.list_workflows().await?.into_iter().map(|w| w.name));
        want.personas
            .extend(store.list_personas().await?.into_iter().map(|p| p.name));
        want.prompts.extend(
            store
                .list_items(gai_agent::PROMPT_KIND)
                .await?
                .into_iter()
                .map(|p| p.name),
        );
        want.skills
            .extend(store.list_skills().await?.into_iter().map(|s| s.name));
        want.mcp
            .extend(store.list_mcp_servers().await?.into_iter().map(|(n, _)| n));
        want.triggers
            .extend(store.list_triggers().await?.into_iter().map(|t| t.name));
        if opts.knowledge_all {
            want.knowledge
                .extend(store.list_collections().await?.into_iter().map(|c| c.name));
        }
    }
    want.agents.extend(opts.agents.iter().cloned());
    want.workflows.extend(opts.workflows.iter().cloned());
    want.personas.extend(opts.personas.iter().cloned());
    want.prompts.extend(opts.prompts.iter().cloned());
    want.skills.extend(opts.skills.iter().cloned());
    want.mcp.extend(opts.mcp_servers.iter().cloned());
    want.knowledge.extend(opts.knowledge.iter().cloned());
    want.triggers.extend(opts.triggers.iter().cloned());
    if want.is_empty() {
        return Err(invalid("nothing to export: name some items, or pass `all`"));
    }

    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut manifest = Manifest {
        format: FORMAT,
        name: opts.name.clone().unwrap_or_else(|| "gai bundle".into()),
        description: opts.description.clone(),
        gai_version: env!("CARGO_PKG_VERSION").into(),
        created_at: chrono::Utc::now().to_rfc3339(),
        ..Default::default()
    };
    let mut env: BTreeSet<EnvVar> = BTreeSet::new();
    let mut models: BTreeSet<String> = BTreeSet::new();
    let deps = !opts.no_dependencies;
    let redact = |text: &str| -> String {
        if opts.redact_pii {
            gai_agent::safety::mask_pii(text, &gai_agent::safety::PiiKind::ALL).0
        } else {
            text.to_owned()
        }
    };

    // Triggers first: they pull in what they run. Webhook secrets stay
    // here; the importing server makes its own.
    for name in &want.triggers {
        let rec = store
            .list_triggers()
            .await?
            .into_iter()
            .find(|t| &t.name == name)
            .ok_or_else(|| Error::NotFound(format!("trigger {name}")))?;
        let spec = gai_triggers::Triggers::spec_of(&rec)
            .map_err(|e| invalid(format!("trigger {name}: {e}")))?;
        if deps {
            let base = |r: &str| r.split('@').next().unwrap_or(r).to_owned();
            match &spec.target {
                gai_triggers::Target::Agent(a) if !a.contains('/') => {
                    want.agents.insert(base(a));
                }
                gai_triggers::Target::Workflow(w) if !w.contains('/') => {
                    want.workflows.insert(base(w));
                }
                _ => {}
            }
            for var in [&spec.deliver.webhook_secret_env, &spec.deliver.slack_env]
                .into_iter()
                .flatten()
            {
                env.insert(EnvVar {
                    name: var.clone(),
                    used_by: format!("trigger/{name}"),
                });
            }
        }
        files.insert(format!("triggers/{name}.yaml"), yaml(&rec.spec)?);
        manifest.contents.triggers.push(name.clone());
    }

    // Workflows next: they can pull in agents.
    let mut done_workflows = BTreeSet::new();
    let mut queue: Vec<String> = want.workflows.iter().cloned().collect();
    while let Some(name) = queue.pop() {
        if !done_workflows.insert(name.clone()) {
            continue;
        }
        let w = store
            .get_workflow(&name, None)
            .await?
            .ok_or_else(|| Error::NotFound(format!("workflow {name}")))?;
        if deps {
            collect_refs(&w.spec, "agent", &mut want.agents);
            let mut nested = BTreeSet::new();
            collect_refs(&w.spec, "workflow", &mut nested);
            queue.extend(nested);
        }
        files.insert(format!("workflows/{name}.yaml"), yaml(&w.spec)?);
        manifest.contents.workflows.push(name);
    }

    let mut done_agents = BTreeSet::new();
    let mut queue: Vec<String> = want.agents.iter().cloned().collect();
    while let Some(name) = queue.pop() {
        if !done_agents.insert(name.clone()) {
            continue;
        }
        let a = store
            .get_agent(&name, None)
            .await?
            .ok_or_else(|| Error::NotFound(format!("agent {name}")))?;
        let spec: AgentSpec = serde_json::from_value(a.spec.clone())
            .map_err(|e| invalid(format!("agent {name}: {e}")))?;
        if let Some(m) = &spec.model {
            models.insert(m.clone());
        }
        if deps {
            for d in &spec.delegates {
                if let Some(n) = delegate_name(d) {
                    queue.push(n);
                }
            }
            if let Some(p) = &spec.persona {
                let p = p.split('@').next().unwrap_or(p);
                if p != gai_agent::NO_PERSONA {
                    want.personas.insert(p.to_owned());
                }
            }
            if let Some(p) = &spec.prompt {
                want.prompts
                    .insert(p.split('@').next().unwrap_or(p).to_owned());
            }
            want.skills
                .extend(spec.skills.iter().filter(|s| !s.contains('*')).cloned());
            want.knowledge.extend(spec.knowledge.iter().cloned());
            for t in &spec.tools {
                if let Some(server) = t.strip_prefix("mcp__").and_then(|r| r.split("__").next()) {
                    want.mcp.insert(server.to_owned());
                }
            }
        }
        files.insert(format!("agents/{name}.yaml"), yaml(&a.spec)?);
        manifest.contents.agents.push(name);
    }

    for name in &want.personas {
        let Some(p) = store.get_persona(name, None).await? else {
            if want.explicit_persona(opts, name) {
                return Err(Error::NotFound(format!("persona {name}")));
            }
            continue;
        };
        files.insert(format!("personas/{name}.yaml"), yaml(&p.spec)?);
        manifest.contents.personas.push(name.clone());
    }

    for name in &want.prompts {
        let (Some(item), Some(p)) = (
            store.item(gai_agent::PROMPT_KIND, name).await?,
            store.get_item(gai_agent::PROMPT_KIND, name, None).await?,
        ) else {
            if opts.prompts.iter().any(|x| x == name) {
                return Err(Error::NotFound(format!("prompt {name}")));
            }
            continue;
        };
        files.insert(format!("prompts/{name}.yaml"), yaml(&p.spec)?);
        if !item.labels.is_empty() {
            let labels: Vec<&str> = item.labels.keys().map(String::as_str).collect();
            files.insert(
                format!("prompts/{name}.labels"),
                labels.join("\n").into_bytes(),
            );
        }
        manifest.contents.prompts.push(name.clone());
    }

    let skills = rt.skills();
    for name in &want.skills {
        let Some(skill) = skills.get(name).await? else {
            if opts.skills.contains(name) {
                return Err(Error::NotFound(format!("skill {name}")));
            }
            continue;
        };
        files.insert(
            format!("skills/{name}/SKILL.md"),
            skill.raw.clone().into_bytes(),
        );
        for rel in &skill.files {
            files.insert(format!("skills/{name}/{rel}"), skill.read_file(rel)?);
        }
        manifest.contents.skills.push(name.clone());
    }

    let stored_mcp: BTreeMap<String, Value> = store.list_mcp_servers().await?.into_iter().collect();
    for name in &want.mcp {
        let Some(cfg) = stored_mcp.get(name) else {
            if opts.mcp_servers.contains(name) {
                return Err(Error::NotFound(format!("MCP server {name}")));
            }
            // Servers from gai.toml are the importer's own business.
            continue;
        };
        let cfg = scrub_mcp(name, cfg, &mut env);
        files.insert(
            format!("mcp/{name}.json"),
            serde_json::to_vec_pretty(&cfg).expect("json"),
        );
        manifest.contents.mcp_servers.push(name.clone());
    }

    // Agent-scoped memory blocks, as the agents start with them.
    let mut blocks = Vec::new();
    for name in &manifest.contents.agents {
        for b in store.list_blocks(Some(&format!("agent:{name}"))).await? {
            blocks.push(NewBlock {
                scope: b.scope,
                label: b.label,
                description: b.description,
                value: redact(&b.value),
                limit: b.limit,
                read_only: b.read_only,
            });
        }
    }
    if !blocks.is_empty() {
        manifest.contents.memory_blocks = blocks.len();
        files.insert(
            "memory/blocks.json".into(),
            serde_json::to_vec_pretty(&blocks).expect("json"),
        );
    }

    for name in &want.knowledge {
        let Some(c) = store.get_collection(name).await? else {
            if opts.knowledge.contains(name) {
                return Err(Error::NotFound(format!("collection {name}")));
            }
            continue;
        };
        let meta = serde_json::json!({
            "name": c.name,
            "description": c.description,
            "embedding_model": c.embedding_model,
            "chunk_tokens": c.chunk_tokens,
            "chunk_overlap": c.chunk_overlap,
            "metadata": c.metadata,
        });
        files.insert(
            format!("knowledge/{name}/collection.json"),
            serde_json::to_vec_pretty(&meta).expect("json"),
        );
        let mut offset = 0;
        let mut n = 0;
        loop {
            let docs = store.list_documents(name, 200, offset).await?;
            if docs.is_empty() {
                break;
            }
            offset += docs.len() as u32;
            for d in docs {
                let chunks = store.document_chunks(&d.id).await?;
                let text = redact(&gai_memory::docs::rejoin_chunks(
                    chunks.iter().map(|c| c.text.as_str()),
                ));
                let front = serde_json::json!({"source": d.source, "title": d.title, "metadata": d.metadata});
                n += 1;
                files.insert(
                    format!("knowledge/{name}/{n:05}.md"),
                    format!("<!-- gai: {front} -->\n{text}").into_bytes(),
                );
            }
        }
        manifest.contents.knowledge.push(name.clone());
    }

    // Provider keys the agents' models need.
    for m in &models {
        let provider = m.split('/').next().unwrap_or_default();
        if let Some(var) = gai
            .models()
            .providers()
            .find(|(n, _)| *n == provider)
            .and_then(|(_, p)| p.api_key_env.clone())
        {
            env.insert(EnvVar {
                name: var,
                used_by: format!("models from {provider}"),
            });
        }
    }
    manifest.requires = Requires {
        env: env.into_iter().collect(),
        models: models.into_iter().collect(),
    };
    for list in [
        &mut manifest.contents.agents,
        &mut manifest.contents.workflows,
    ] {
        list.sort();
    }

    let mut out = Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut out);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("bundle.toml", opts).map_err(zip_err)?;
        zip.write_all(
            toml::to_string_pretty(&manifest)
                .map_err(|e| invalid(e.to_string()))?
                .as_bytes(),
        )
        .map_err(|e| invalid(e.to_string()))?;
        for (path, data) in &files {
            zip.start_file(path, opts).map_err(zip_err)?;
            zip.write_all(data).map_err(|e| invalid(e.to_string()))?;
        }
        zip.finish().map_err(zip_err)?;
    }
    Ok(out.into_inner())
}

/// Read a bundle's manifest and check what it needs, without importing.
pub async fn inspect(gai: &Gai, bytes: &[u8]) -> Result<ImportReport, Error> {
    let files = unzip(bytes)?;
    let manifest = read_manifest(&files)?;
    let mut report = ImportReport {
        manifest,
        ..Default::default()
    };
    needs(gai, &mut report);
    Ok(report)
}

/// Install a bundle's items.
pub async fn import(gai: &Gai, bytes: &[u8], opts: &ImportOptions) -> Result<ImportReport, Error> {
    let files = unzip(bytes)?;
    let manifest = read_manifest(&files)?;
    let rt = gai.runtime().await?;
    let store = rt.store();
    let note = format!("imported from bundle {:?}", manifest.name);
    let mut report = ImportReport {
        manifest,
        ..Default::default()
    };
    let text = |path: &str| -> Result<&str, Error> {
        std::str::from_utf8(&files[path]).map_err(|_| invalid(format!("{path} is not UTF-8")))
    };
    let in_dir = |dir: &str, ext: &str| -> Vec<(String, String)> {
        files
            .keys()
            .filter_map(|p| {
                let rest = p.strip_prefix(dir)?.strip_suffix(ext)?;
                (!rest.contains('/')).then(|| (rest.to_owned(), p.clone()))
            })
            .collect()
    };

    // Personas, skills and MCP servers first: agents refer to them.
    for (name, path) in in_dir("personas/", ".yaml") {
        let spec: gai_agent::PersonaSpec =
            serde_yaml::from_str(text(&path)?).map_err(|e| invalid(format!("{path}: {e}")))?;
        if opts.skip_existing && store.get_persona(&name, None).await?.is_some() {
            report.skipped.push(format!("persona/{name}"));
            continue;
        }
        let v = store
            .put_persona(
                &name,
                &serde_json::to_value(&spec).expect("json"),
                Some(&note),
            )
            .await?;
        report.installed.push(format!("persona/{name}@v{v}"));
    }

    // Prompt templates, before the agents that use them. The labels they
    // had point at the imported version, so `name@prod` keeps working.
    for (name, path) in in_dir("prompts/", ".yaml") {
        let spec: gai_agent::PromptSpec =
            serde_yaml::from_str(text(&path)?).map_err(|e| invalid(format!("{path}: {e}")))?;
        spec.validate()
            .map_err(|e| invalid(format!("{path}: {e}")))?;
        if opts.skip_existing
            && store
                .get_item(gai_agent::PROMPT_KIND, &name, None)
                .await?
                .is_some()
        {
            report.skipped.push(format!("prompt/{name}"));
            continue;
        }
        let v = store
            .put_item(
                gai_agent::PROMPT_KIND,
                &name,
                &serde_json::to_value(&spec).expect("json"),
                Some(&note),
            )
            .await?;
        if let Some(labels) = files.get(&format!("prompts/{name}.labels")) {
            for label in String::from_utf8_lossy(labels)
                .lines()
                .filter(|l| !l.trim().is_empty())
            {
                store
                    .set_item_label(gai_agent::PROMPT_KIND, &name, label.trim(), Some(v))
                    .await?;
            }
        }
        report.installed.push(format!("prompt/{name}@v{v}"));
    }

    let skill_names: BTreeSet<String> = files
        .keys()
        .filter_map(|p| p.strip_prefix("skills/")?.strip_suffix("/SKILL.md"))
        .filter(|n| !n.contains('/'))
        .map(str::to_owned)
        .collect();
    for name in skill_names {
        if opts.skip_existing && rt.skills().get(&name).await?.is_some() {
            report.skipped.push(format!("skill/{name}"));
            continue;
        }
        let prefix = format!("skills/{name}/");
        let mut extra = BTreeMap::new();
        for (p, data) in &files {
            if let Some(rel) = p.strip_prefix(&prefix)
                && rel != "SKILL.md"
            {
                match String::from_utf8(data.clone()) {
                    Ok(s) => {
                        extra.insert(rel.to_owned(), s);
                    }
                    Err(_) => report
                        .warnings
                        .push(format!("skill {name}: {rel} is not text and was left out")),
                }
            }
        }
        match rt
            .skills()
            .save(text(&format!("{prefix}SKILL.md"))?, extra)
            .await
        {
            Ok((n, v)) => report.installed.push(format!("skill/{n}@v{v}")),
            Err(e) => report.warnings.push(format!("skill {name}: {e}")),
        }
    }

    for (name, path) in in_dir("mcp/", ".json") {
        let cfg: gai_tools::mcp::McpServerConfig =
            serde_json::from_slice(&files[&path]).map_err(|e| invalid(format!("{path}: {e}")))?;
        cfg.validate()
            .map_err(|e| invalid(format!("{path}: {e}")))?;
        let existing = store
            .list_mcp_servers()
            .await?
            .into_iter()
            .any(|(n, _)| n == name);
        if existing && opts.skip_existing {
            report.skipped.push(format!("mcp/{name}"));
            continue;
        }
        store
            .put_mcp_server(&name, &serde_json::to_value(&cfg).expect("json"))
            .await?;
        rt.register_mcp(gai_tools::mcp::McpServer::new(&name, cfg, "api"));
        report.installed.push(format!("mcp/{name}"));
    }

    for (name, path) in in_dir("agents/", ".yaml") {
        let mut spec: AgentSpec =
            serde_yaml::from_str(text(&path)?).map_err(|e| invalid(format!("{path}: {e}")))?;
        spec.name = name.clone();
        if opts.skip_existing && store.get_agent(&name, None).await?.is_some() {
            report.skipped.push(format!("agent/{name}"));
            continue;
        }
        let v = rt.save_agent(&spec, Some(&note)).await?;
        report.installed.push(format!("agent/{name}@v{v}"));
    }

    let wf = gai.workflows().await?;
    for (name, path) in in_dir("workflows/", ".yaml") {
        let mut spec: gai_orchestra::WorkflowSpec =
            serde_yaml::from_str(text(&path)?).map_err(|e| invalid(format!("{path}: {e}")))?;
        spec.name = name.clone();
        if opts.skip_existing && store.get_workflow(&name, None).await?.is_some() {
            report.skipped.push(format!("workflow/{name}"));
            continue;
        }
        let v = wf.save(&spec, Some(&note)).await?;
        report.installed.push(format!("workflow/{name}@v{v}"));
    }

    // Triggers come in turned off: a schedule shouldn't start firing, or a
    // file watcher start reading folders, before someone looks at it.
    let triggers = gai.triggers().await?;
    for (name, path) in in_dir("triggers/", ".yaml") {
        let mut spec = gai_triggers::TriggerSpec::parse(text(&path)?)
            .map_err(|e| invalid(format!("{path}: {e}")))?;
        spec.name = name.clone();
        let existing = triggers.get(&name).await.ok();
        if opts.skip_existing && existing.is_some() {
            report.skipped.push(format!("trigger/{name}"));
            continue;
        }
        if let gai_triggers::Source::File { path: dir, .. } = &spec.source {
            report.warnings.push(format!(
                "trigger {name} watches {dir}: check that folder exists here"
            ));
        }
        let saved = match &existing {
            Some(rec) => triggers.update(&rec.id, spec).await,
            None => triggers.create(spec).await,
        }
        .map_err(|e| invalid(format!("trigger {name}: {e}")))?;
        triggers
            .set_enabled(&saved.id, false)
            .await
            .map_err(|e| invalid(format!("trigger {name}: {e}")))?;
        if saved.kind == "webhook" && existing.is_none() {
            report.warnings.push(format!(
                "trigger {name} has a new webhook address and secret: update whatever calls it"
            ));
        }
        report.installed.push(format!("trigger/{name}"));
    }
    if report.installed.iter().any(|i| i.starts_with("trigger/")) {
        report
            .warnings
            .push("triggers are imported turned off: turn them on under Schedules".into());
    }

    if let Some(data) = files.get("memory/blocks.json") {
        let blocks: Vec<NewBlock> = serde_json::from_slice(data)
            .map_err(|e| invalid(format!("memory/blocks.json: {e}")))?;
        for b in blocks {
            let label = format!("memory/{}/{}", b.scope, b.label);
            if store.find_block(&b.scope, &b.label).await?.is_some() {
                // A block's value is the agent's own memory by now.
                report.skipped.push(label);
                continue;
            }
            store.create_block(b, Some("bundle")).await?;
            report.installed.push(label);
        }
    }

    let knowledge = rt.knowledge();
    for (name, path) in files
        .keys()
        .filter_map(|p| {
            let rest = p
                .strip_prefix("knowledge/")?
                .strip_suffix("/collection.json")?;
            (!rest.contains('/')).then(|| (rest.to_owned(), p.clone()))
        })
        .collect::<Vec<_>>()
    {
        let meta: Value =
            serde_json::from_slice(&files[&path]).map_err(|e| invalid(format!("{path}: {e}")))?;
        if store.get_collection(&name).await?.is_none() {
            let model = meta["embedding_model"].as_str().map(str::to_owned);
            // The bundle's embedding model may not exist here; fall back to ours.
            let model = model.filter(|m| gai.models().embedding(m).is_ok());
            if model.is_none() && meta["embedding_model"].is_string() {
                report.warnings.push(format!(
                    "collection {name}: embedding with {} instead of {}",
                    knowledge.default_embedding_model(),
                    meta["embedding_model"].as_str().unwrap_or_default()
                ));
            }
            knowledge
                .create_collection(gai_memory::NewCollection {
                    name: name.clone(),
                    description: meta["description"].as_str().map(str::to_owned),
                    embedding_model: model,
                    chunk_tokens: meta["chunk_tokens"].as_u64().map(|v| v as u32),
                    chunk_overlap: meta["chunk_overlap"].as_u64().map(|v| v as u32),
                    metadata: meta["metadata"].clone(),
                })
                .await?;
        } else if opts.skip_existing {
            report.skipped.push(format!("knowledge/{name}"));
            continue;
        }
        let prefix = format!("knowledge/{name}/");
        let mut docs = 0;
        for (p, data) in &files {
            let Some(rel) = p.strip_prefix(&prefix) else {
                continue;
            };
            if !rel.ends_with(".md") || rel.contains('/') {
                continue;
            }
            let raw = String::from_utf8_lossy(data);
            let (front, body) = split_front(&raw);
            let source = front["source"].as_str().unwrap_or(rel).to_owned();
            match knowledge
                .ingest(
                    &name,
                    gai_memory::DocumentInput {
                        source: source.clone(),
                        title: front["title"].as_str().map(str::to_owned),
                        text: body.to_owned(),
                        metadata: front["metadata"].clone(),
                    },
                )
                .await
            {
                Ok(_) => docs += 1,
                Err(e) => report.warnings.push(format!("{name}: {source}: {e}")),
            }
        }
        report
            .installed
            .push(format!("knowledge/{name} ({docs} documents)"));
    }

    needs(gai, &mut report);
    Ok(report)
}

/// Fill in which required variables and models are missing here.
fn needs(gai: &Gai, report: &mut ImportReport) {
    report.missing_env = report
        .manifest
        .requires
        .env
        .iter()
        .filter(|v| std::env::var_os(&v.name).is_none_or(|s| s.is_empty()))
        .cloned()
        .collect();
    report.missing_models = report
        .manifest
        .requires
        .models
        .iter()
        .filter(|m| gai.models().chat(m).is_err())
        .cloned()
        .collect();
}

#[derive(Default)]
struct Selection {
    agents: BTreeSet<String>,
    workflows: BTreeSet<String>,
    personas: BTreeSet<String>,
    prompts: BTreeSet<String>,
    skills: BTreeSet<String>,
    mcp: BTreeSet<String>,
    knowledge: BTreeSet<String>,
    triggers: BTreeSet<String>,
}

impl Selection {
    fn is_empty(&self) -> bool {
        self.agents.is_empty()
            && self.workflows.is_empty()
            && self.personas.is_empty()
            && self.prompts.is_empty()
            && self.skills.is_empty()
            && self.mcp.is_empty()
            && self.knowledge.is_empty()
            && self.triggers.is_empty()
    }

    fn explicit_persona(&self, opts: &ExportOptions, name: &str) -> bool {
        opts.personas.iter().any(|p| p == name)
    }
}

/// Names of stored items a workflow uses under `key` (`agent: name` on a
/// node, as a string).
fn collect_refs(v: &Value, key: &str, out: &mut BTreeSet<String>) {
    match v {
        Value::Object(m) => {
            for (k, child) in m {
                if k == key
                    && let Some(s) = child.as_str()
                {
                    out.insert(s.split('@').next().unwrap_or(s).to_owned());
                }
                collect_refs(child, key, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|c| collect_refs(c, key, out)),
        _ => {}
    }
}

/// The stored agent a delegate refers to (not remote ones or files).
fn delegate_name(d: &gai_agent::DelegateRef) -> Option<String> {
    let s = match d {
        gai_agent::DelegateRef::Agent(s) => s,
        gai_agent::DelegateRef::Spec(d) => &d.agent,
        gai_agent::DelegateRef::Remote(_) => return None,
    };
    let s = s.split('@').next().unwrap_or(s);
    (!s.contains('/') && !s.contains('.')).then(|| s.to_owned())
}

/// An MCP config with literal secrets swapped for `${VARS}`, noting every
/// variable it reads.
fn scrub_mcp(server: &str, cfg: &Value, env: &mut BTreeSet<EnvVar>) -> Value {
    let mut cfg = cfg.clone();
    let prefix = format!("GAI_MCP_{}", envish(server));
    for field in ["env", "headers"] {
        let Some(map) = cfg.get_mut(field).and_then(Value::as_object_mut) else {
            continue;
        };
        for (k, v) in map.iter_mut() {
            let Some(s) = v.as_str() else { continue };
            let vars = vars_in(s);
            if vars.is_empty() && !s.is_empty() {
                let var = format!("{prefix}_{}", envish(k));
                env.insert(EnvVar {
                    name: var.clone(),
                    used_by: format!("MCP server {server} ({field} {k})"),
                });
                *v = Value::String(format!("${{{var}}}"));
            } else {
                for var in vars {
                    env.insert(EnvVar {
                        name: var,
                        used_by: format!("MCP server {server} ({field} {k})"),
                    });
                }
            }
        }
    }
    cfg
}

fn envish(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// `${VAR}` names in a string.
fn vars_in(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find("${") {
        rest = &rest[i + 2..];
        match rest.find('}') {
            Some(j) => {
                out.push(rest[..j].to_owned());
                rest = &rest[j + 1..];
            }
            None => break,
        }
    }
    out
}

fn split_front(raw: &str) -> (Value, &str) {
    if let Some(rest) = raw.strip_prefix("<!-- gai: ")
        && let Some(end) = rest.find(" -->\n")
        && let Ok(v) = serde_json::from_str(&rest[..end])
    {
        return (v, &rest[end + 5..]);
    }
    (Value::Null, raw)
}

fn yaml(v: &Value) -> Result<Vec<u8>, Error> {
    serde_yaml::to_string(v)
        .map(String::into_bytes)
        .map_err(|e| invalid(e.to_string()))
}

fn zip_err(e: zip::result::ZipError) -> Error {
    invalid(format!("zip: {e}"))
}

/// The bundle's files by path. Refuses paths that would escape a folder.
fn unzip(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, Error> {
    if bytes.len() as u64 > MAX_BUNDLE {
        return Err(invalid("the bundle is larger than 200 MB"));
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| invalid(format!("not a .gaibundle (zip) file: {e}")))?;
    let mut out = BTreeMap::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(zip_err)?;
        if f.is_dir() {
            continue;
        }
        let name = f.name().replace('\\', "/");
        if name.starts_with('/') || name.split('/').any(|p| p == ".." || p.is_empty()) {
            return Err(invalid(format!("bad path in bundle: {name}")));
        }
        if f.size() > MAX_FILE {
            return Err(invalid(format!("{name} is larger than 50 MB")));
        }
        let mut data = Vec::with_capacity(f.size() as usize);
        (&mut f)
            .take(MAX_FILE + 1)
            .read_to_end(&mut data)
            .map_err(|e| invalid(format!("{name}: {e}")))?;
        total += data.len() as u64;
        if total > MAX_BUNDLE {
            return Err(invalid("the bundle unpacks to more than 200 MB"));
        }
        out.insert(name, data);
    }
    Ok(out)
}

fn read_manifest(files: &BTreeMap<String, Vec<u8>>) -> Result<Manifest, Error> {
    let raw = files
        .get("bundle.toml")
        .ok_or_else(|| invalid("not a gai bundle: no bundle.toml"))?;
    let m: Manifest = toml::from_str(&String::from_utf8_lossy(raw))
        .map_err(|e| invalid(format!("bundle.toml: {e}")))?;
    if m.format > FORMAT {
        return Err(invalid(format!(
            "this bundle uses format {} from gai {}; this gai reads format {FORMAT}",
            m.format, m.gai_version
        )));
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejoins_overlapping_chunks() {
        let chunks = [
            "The quick brown fox jumps",
            "brown fox jumps over the lazy dog.",
        ];
        assert_eq!(
            gai_memory::docs::rejoin_chunks(chunks.into_iter()),
            "The quick brown fox jumps over the lazy dog."
        );
        assert_eq!(
            gai_memory::docs::rejoin_chunks(["one.", "two."].into_iter()),
            "one.\n\ntwo."
        );
    }

    #[test]
    fn mcp_secrets_become_variables() {
        let mut env = BTreeSet::new();
        let cfg = serde_json::json!({
            "url": "https://mcp.example.com",
            "headers": {"Authorization": "Bearer sk-live-123", "X-Team": "${TEAM_ID}"},
            "env": {"EMPTY": ""}
        });
        let out = scrub_mcp("my-server", &cfg, &mut env);
        assert_eq!(
            out["headers"]["Authorization"],
            "${GAI_MCP_MY_SERVER_AUTHORIZATION}"
        );
        assert_eq!(out["headers"]["X-Team"], "${TEAM_ID}");
        assert!(!out.to_string().contains("sk-live"));
        let names: Vec<_> = env.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["GAI_MCP_MY_SERVER_AUTHORIZATION", "TEAM_ID"]);
    }

    #[test]
    fn front_matter_round_trips() {
        let (v, body) = split_front("<!-- gai: {\"source\":\"a.md\"} -->\nhello");
        assert_eq!(v["source"], "a.md");
        assert_eq!(body, "hello");
        let (v, body) = split_front("plain");
        assert!(v.is_null());
        assert_eq!(body, "plain");
    }
}
