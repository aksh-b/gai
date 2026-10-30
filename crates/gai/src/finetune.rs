//! Fine-tuning on your own rated runs.
//!
//! A training set is built from runs people rated thumbs up: each example
//! is what the model was sent on the run's last model call (system prompt,
//! messages and tools) followed by its answer, in the chat format OpenAI
//! and Together take. A job sends it to a provider's fine-tuning API, or
//! trains a LoRA adapter locally in a container, and the result is added
//! to the catalog like any other model.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use gai_core::{ContentBlock, Message, Role};
use gai_llm::{ChatRequest, ModelConfig, ModelRegistry};
use gai_store::{FeedbackFilter, SharedStore, StepKind};
use schemars::JsonSchema;
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Error;

const PREFIX: &str = "finetune.";

/// Which rated runs make the training set.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct DatasetFilter {
    /// Only runs of this agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Only runs rated since then.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<DateTime<Utc>>,
    /// At most this many examples (default 2000).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Leave out the tool definitions (examples still show tool calls).
    pub without_tools: bool,
}

#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
pub struct DatasetStats {
    pub examples: usize,
    /// Rated runs that couldn't be used, and why.
    pub skipped: Vec<String>,
}

/// Build the training set: JSON Lines, one conversation per line.
pub async fn build_dataset(
    store: &SharedStore,
    filter: &DatasetFilter,
) -> Result<(String, DatasetStats), Error> {
    let limit = filter.limit.unwrap_or(2000).min(50_000);
    let rated = store
        .list_feedback(FeedbackFilter {
            agent: filter.agent.clone(),
            rating: Some(1),
            limit: limit * 2,
            ..Default::default()
        })
        .await?;
    let mut out = String::new();
    let mut stats = DatasetStats::default();
    let mut seen = std::collections::HashSet::new();
    for f in rated {
        if stats.examples >= limit as usize {
            break;
        }
        if filter.since.is_some_and(|s| f.created_at < s) || !seen.insert(f.run_id.clone()) {
            continue;
        }
        match example(store, &f.run_id, !filter.without_tools).await {
            Ok(line) => {
                out.push_str(&line.to_string());
                out.push('\n');
                stats.examples += 1;
            }
            Err(why) => stats.skipped.push(format!("{}: {why}", f.run_id)),
        }
    }
    Ok((out, stats))
}

/// One run as a training example.
async fn example(store: &SharedStore, run_id: &str, with_tools: bool) -> Result<Value, String> {
    let steps = store.list_steps(run_id).await.map_err(|e| e.to_string())?;
    let last = steps
        .iter()
        .rev()
        .find(|s| s.kind == StepKind::Model && s.output.is_some())
        .ok_or("no model call was recorded")?;
    let input = last.input.as_ref().ok_or("no model call was recorded")?;
    let req: ChatRequest = serde_json::from_value(input["request"].clone())
        .map_err(|_| "the request wasn't recorded (record_requests is off)")?;
    let reply: Vec<ContentBlock> =
        serde_json::from_value(last.output.as_ref().expect("checked")["content"].clone())
            .map_err(|e| e.to_string())?;
    let mut messages = Vec::new();
    if let Some(s) = req.system.as_deref().filter(|s| !s.is_empty()) {
        messages.push(json!({"role": "system", "content": s}));
    }
    for m in req.messages.iter().chain(std::iter::once(&Message {
        role: Role::Assistant,
        content: reply,
    })) {
        messages.extend(openai_messages(m)?);
    }
    if messages.last().is_none_or(|m| m["role"] != "assistant") {
        return Err("it doesn't end with an answer".into());
    }
    let mut line = json!({ "messages": messages });
    if with_tools && !req.tools.is_empty() {
        line["tools"] = req
            .tools
            .iter()
            .map(|t| {
                json!({"type": "function", "function": {
                    "name": t.name, "description": t.description, "parameters": t.input_schema,
                }})
            })
            .collect();
    }
    Ok(line)
}

/// A message in OpenAI's chat format: tool results become `tool` messages.
fn openai_messages(m: &Message) -> Result<Vec<Value>, String> {
    let mut text = String::new();
    let mut calls = Vec::new();
    let mut results = Vec::new();
    for b in &m.content {
        match b {
            ContentBlock::Text { text: t } => text.push_str(t),
            ContentBlock::ToolUse { id, name, input } => calls.push(json!({
                "id": id, "type": "function",
                "function": {"name": name, "arguments": input.to_string()},
            })),
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => {
                let t: String = content.iter().filter_map(ContentBlock::as_text).collect();
                results.push(json!({"role": "tool", "tool_call_id": tool_use_id, "content": t}));
            }
            ContentBlock::Image { .. } | ContentBlock::Audio { .. } => {
                return Err("it has images or audio".into());
            }
            _ => {}
        }
    }
    let mut out = results;
    match m.role {
        Role::Assistant => {
            let mut a = json!({"role": "assistant", "content": text});
            if !calls.is_empty() {
                a["tool_calls"] = json!(calls);
            }
            out.push(a);
        }
        Role::User if !text.is_empty() => out.push(json!({"role": "user", "content": text})),
        Role::User => {}
    }
    Ok(out)
}

/// Where the training happens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Trainer {
    /// OpenAI's fine-tuning API (needs the openai key).
    Openai,
    /// Together's fine-tuning API (needs TOGETHER_API_KEY).
    Together,
    /// A LoRA adapter trained here, in a container on the GPU, then served
    /// with vLLM.
    Local,
}

/// A fine-tuning job to start.
///
/// ```yaml
/// name: support-v2
/// trainer: openai
/// base_model: gpt-4.1-mini-2025-04-14
/// dataset: { agent: support }
/// epochs: 3
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FineTuneSpec {
    /// Letters, digits and `-`; also the model's suffix.
    pub name: String,
    pub trainer: Trainer,
    /// The model to start from: the provider's id (`gpt-4.1-mini-2025-04-14`,
    /// `meta-llama/Meta-Llama-3.1-8B-Instruct-Reference`), or for `local` a
    /// Hugging Face id.
    pub base_model: String,
    #[serde(default)]
    pub dataset: DatasetFilter,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epochs: Option<u32>,
    /// OpenAI's learning rate multiplier, or the local learning rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learning_rate: Option<f64>,
    /// LoRA rank (Together and local; default 16).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lora_rank: Option<u32>,
}

impl FineTuneSpec {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty()
            || self.name.len() > 40
            || !self
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(
                "a fine-tune's name is lowercase letters, digits and - (at most 40)".into(),
            );
        }
        if self.base_model.trim().is_empty() {
            return Err("say which base_model to start from".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Preparing,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn finished(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct FineTuneJob {
    pub spec: FineTuneSpec,
    pub status: JobStatus,
    pub examples: usize,
    /// The provider's job id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    /// The model to pick once it succeeded (`openai/ft:...`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Training jobs and the models they make.
#[derive(Clone)]
pub struct FineTunes {
    store: SharedStore,
    models: Arc<ModelRegistry>,
    http: reqwest::Client,
    /// For local jobs: the engine and the model servers.
    #[cfg(feature = "sandbox")]
    local: Option<crate::deployments::Deployments>,
}

/// Saves models a job made so they survive restarts.
pub type AddModel<'a> = &'a (
        dyn Fn(&str, ModelConfig) -> futures::future::BoxFuture<'a, Result<(), Error>> + Send + Sync
    );

impl FineTunes {
    pub fn new(store: SharedStore, models: Arc<ModelRegistry>) -> Self {
        Self {
            store,
            models,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(300))
                .build()
                .expect("http client"),
            #[cfg(feature = "sandbox")]
            local: None,
        }
    }

    #[cfg(feature = "sandbox")]
    pub fn with_local(mut self, d: Option<crate::deployments::Deployments>) -> Self {
        self.local = d;
        self
    }

    pub fn parse(value: Value) -> Result<FineTuneSpec, Error> {
        let spec: FineTuneSpec =
            serde_json::from_value(value).map_err(|e| Error::Invalid(format!("fine-tune: {e}")))?;
        spec.validate().map_err(Error::Invalid)?;
        Ok(spec)
    }

    async fn put(&self, job: &FineTuneJob) -> Result<(), Error> {
        self.store
            .put_setting(
                &format!("{PREFIX}{}", job.spec.name),
                &serde_json::to_string(job).expect("job"),
            )
            .await?;
        Ok(())
    }

    pub async fn get(&self, name: &str) -> Result<FineTuneJob, Error> {
        let v = self
            .store
            .get_setting(&format!("{PREFIX}{name}"))
            .await?
            .ok_or_else(|| Error::NotFound(format!("no fine-tune {name}")))?;
        serde_json::from_str(&v).map_err(|e| Error::Invalid(e.to_string()))
    }

    pub async fn list(&self) -> Result<Vec<FineTuneJob>, Error> {
        let mut out: Vec<FineTuneJob> = self
            .store
            .list_settings(PREFIX)
            .await?
            .into_iter()
            .filter_map(|(_, v)| serde_json::from_str(&v).ok())
            .collect();
        out.sort_by_key(|j| std::cmp::Reverse(j.created_at));
        Ok(out)
    }

    /// Forget a finished job (the model it made stays).
    pub async fn delete(&self, name: &str) -> Result<bool, Error> {
        if let Ok(j) = self.get(name).await
            && !j.status.finished()
        {
            return Err(Error::Invalid(format!(
                "{name} is still running; cancel it first"
            )));
        }
        Ok(self
            .store
            .delete_setting(&format!("{PREFIX}{name}"))
            .await?)
    }

    fn key(&self, provider: &str) -> Result<(String, String), Error> {
        let (base, key) = self
            .models
            .credentials(provider)
            .ok_or_else(|| Error::Invalid(format!("no {provider} provider is configured")))?;
        let key = key.ok_or_else(|| Error::Invalid(format!("add a {provider} key first")))?;
        Ok((
            base.trim_end_matches('/').to_owned(),
            key.expose_secret().to_owned(),
        ))
    }

    /// Build the training set and start the job.
    pub async fn start(&self, spec: FineTuneSpec) -> Result<FineTuneJob, Error> {
        spec.validate().map_err(Error::Invalid)?;
        if let Ok(j) = self.get(&spec.name).await
            && !j.status.finished()
        {
            return Err(Error::Invalid(format!("{} is already running", spec.name)));
        }
        let (data, stats) = build_dataset(&self.store, &spec.dataset).await?;
        let min = if spec.trainer == Trainer::Openai {
            10
        } else {
            1
        };
        if stats.examples < min {
            return Err(Error::Invalid(format!(
                "only {} usable rated runs (at least {min} needed): rate more good answers with thumbs up{}",
                stats.examples,
                stats
                    .skipped
                    .first()
                    .map(|s| format!("; skipped {s}"))
                    .unwrap_or_default()
            )));
        }
        let now = Utc::now();
        let mut job = FineTuneJob {
            spec,
            status: JobStatus::Preparing,
            examples: stats.examples,
            job_id: None,
            model: None,
            error: None,
            created_at: now,
            updated_at: now,
        };
        self.put(&job).await?;
        let started = match job.spec.trainer {
            Trainer::Openai => self.start_openai(&job.spec, data).await,
            Trainer::Together => self.start_together(&job.spec, data).await,
            Trainer::Local => self.start_local(&job.spec, data).await,
        };
        match started {
            Ok(id) => {
                job.job_id = Some(id);
                job.status = JobStatus::Running;
            }
            Err(e) => {
                job.status = JobStatus::Failed;
                job.error = Some(e.to_string());
            }
        }
        job.updated_at = Utc::now();
        self.put(&job).await?;
        Ok(job)
    }

    async fn start_openai(&self, spec: &FineTuneSpec, data: String) -> Result<String, Error> {
        let (base, key) = self.key("openai")?;
        let file = reqwest::multipart::Part::bytes(data.into_bytes())
            .file_name(format!("{}.jsonl", spec.name))
            .mime_str("application/jsonl")
            .expect("valid mime");
        let form = reqwest::multipart::Form::new()
            .text("purpose", "fine-tune")
            .part("file", file);
        let up: Value = self
            .call(
                self.http
                    .post(format!("{base}/files"))
                    .bearer_auth(&key)
                    .multipart(form),
            )
            .await?;
        let file_id = up["id"].as_str().ok_or_else(|| bad("no file id"))?;
        let mut hp = serde_json::Map::new();
        if let Some(e) = spec.epochs {
            hp.insert("n_epochs".into(), json!(e));
        }
        if let Some(l) = spec.learning_rate {
            hp.insert("learning_rate_multiplier".into(), json!(l));
        }
        let mut body =
            json!({"training_file": file_id, "model": spec.base_model, "suffix": spec.name});
        if !hp.is_empty() {
            body["method"] = json!({"type": "supervised", "supervised": {"hyperparameters": hp}});
        }
        let job: Value = self
            .call(
                self.http
                    .post(format!("{base}/fine_tuning/jobs"))
                    .bearer_auth(&key)
                    .json(&body),
            )
            .await?;
        Ok(job["id"]
            .as_str()
            .ok_or_else(|| bad("no job id"))?
            .to_owned())
    }

    async fn start_together(&self, spec: &FineTuneSpec, data: String) -> Result<String, Error> {
        let (base, key) = self.key("together")?;
        let name = format!("{}.jsonl", spec.name);
        let file = reqwest::multipart::Part::bytes(data.into_bytes())
            .file_name(name.clone())
            .mime_str("application/jsonl")
            .expect("valid mime");
        let form = reqwest::multipart::Form::new()
            .text("purpose", "fine-tune")
            .text("file_name", name)
            .part("file", file);
        let up: Value = self
            .call(
                self.http
                    .post(format!("{base}/files/upload"))
                    .bearer_auth(&key)
                    .multipart(form),
            )
            .await?;
        let file_id = up["id"].as_str().ok_or_else(|| bad("no file id"))?;
        let mut body = json!({
            "training_file": file_id,
            "model": spec.base_model,
            "suffix": spec.name,
            "lora": true,
            "lora_r": spec.lora_rank.unwrap_or(16),
        });
        if let Some(e) = spec.epochs {
            body["n_epochs"] = json!(e);
        }
        if let Some(l) = spec.learning_rate {
            body["learning_rate"] = json!(l);
        }
        let job: Value = self
            .call(
                self.http
                    .post(format!("{base}/fine-tunes"))
                    .bearer_auth(&key)
                    .json(&body),
            )
            .await?;
        Ok(job["id"]
            .as_str()
            .ok_or_else(|| bad("no job id"))?
            .to_owned())
    }

    #[cfg(not(feature = "sandbox"))]
    async fn start_local(&self, _: &FineTuneSpec, _: String) -> Result<String, Error> {
        Err(Error::Invalid(
            "local training needs gai built with the sandbox feature".into(),
        ))
    }

    #[cfg(feature = "sandbox")]
    async fn start_local(&self, spec: &FineTuneSpec, data: String) -> Result<String, Error> {
        let d = self.local.as_ref().ok_or_else(|| {
            Error::Invalid("local training runs in the default organization".into())
        })?;
        d.start_job(&local_job(spec, data)).await?;
        Ok(format!("gai-job-ft-{}", spec.name))
    }

    async fn call(&self, rb: reqwest::RequestBuilder) -> Result<Value, Error> {
        let r = rb.send().await.map_err(|e| Error::Invalid(e.to_string()))?;
        let status = r.status();
        let body: Value = r.json().await.unwrap_or_default();
        if !status.is_success() {
            let msg = body["error"]["message"]
                .as_str()
                .or(body["error"].as_str())
                .or(body["message"].as_str())
                .unwrap_or("no details");
            return Err(Error::Invalid(format!(
                "the provider answered {status}: {msg}"
            )));
        }
        Ok(body)
    }

    /// Check unfinished jobs with their providers; jobs that succeeded add
    /// their model with `add_model`. Returns the jobs that changed.
    pub async fn poll(&self, add_model: AddModel<'_>) -> Result<Vec<FineTuneJob>, Error> {
        let mut changed = Vec::new();
        for mut job in self.list().await? {
            if job.status != JobStatus::Running {
                continue;
            }
            let Some(id) = job.job_id.clone() else {
                continue;
            };
            let before = (job.status, job.model.clone(), job.error.clone());
            let r = match job.spec.trainer {
                Trainer::Openai => self.poll_openai(&mut job, &id, add_model).await,
                Trainer::Together => self.poll_together(&mut job, &id, add_model).await,
                Trainer::Local => self.poll_local(&mut job).await,
            };
            if let Err(e) = r {
                tracing::warn!(job = %job.spec.name, error = %e, "could not check a fine-tune");
                continue;
            }
            if (job.status, job.model.clone(), job.error.clone()) != before {
                job.updated_at = Utc::now();
                self.put(&job).await?;
                changed.push(job);
            }
        }
        Ok(changed)
    }

    async fn poll_openai(
        &self,
        job: &mut FineTuneJob,
        id: &str,
        add: AddModel<'_>,
    ) -> Result<(), Error> {
        let (base, key) = self.key("openai")?;
        let v = self
            .call(
                self.http
                    .get(format!("{base}/fine_tuning/jobs/{id}"))
                    .bearer_auth(&key),
            )
            .await?;
        match v["status"].as_str().unwrap_or_default() {
            "succeeded" => {
                let m = v["fine_tuned_model"]
                    .as_str()
                    .ok_or_else(|| bad("no model"))?;
                self.finish(job, "openai", m, add).await?;
            }
            "failed" => {
                job.status = JobStatus::Failed;
                job.error = Some(v["error"]["message"].as_str().unwrap_or("it failed").into());
            }
            "cancelled" => job.status = JobStatus::Cancelled,
            _ => {}
        }
        Ok(())
    }

    async fn poll_together(
        &self,
        job: &mut FineTuneJob,
        id: &str,
        add: AddModel<'_>,
    ) -> Result<(), Error> {
        let (base, key) = self.key("together")?;
        let v = self
            .call(
                self.http
                    .get(format!("{base}/fine-tunes/{id}"))
                    .bearer_auth(&key),
            )
            .await?;
        match v["status"].as_str().unwrap_or_default() {
            "completed" => {
                let m = v["output_name"].as_str().ok_or_else(|| bad("no model"))?;
                self.finish(job, "together", m, add).await?;
            }
            "error" | "failed" => {
                job.status = JobStatus::Failed;
                job.error = Some(
                    v["events"]
                        .as_array()
                        .and_then(|e| e.last())
                        .and_then(|e| e["message"].as_str())
                        .unwrap_or("it failed")
                        .into(),
                );
            }
            "cancelled" | "user_error" => job.status = JobStatus::Cancelled,
            _ => {}
        }
        Ok(())
    }

    #[cfg(not(feature = "sandbox"))]
    async fn poll_local(&self, _: &mut FineTuneJob) -> Result<(), Error> {
        Ok(())
    }

    /// A finished local job becomes a vLLM deployment serving the base
    /// model with the adapter, as `deploy-ft-<name>/<name>`.
    #[cfg(feature = "sandbox")]
    async fn poll_local(&self, job: &mut FineTuneJob) -> Result<(), Error> {
        let Some(d) = &self.local else { return Ok(()) };
        let name = format!("ft-{}", job.spec.name);
        let Some(status) = d.job_status(&name).await? else {
            job.status = JobStatus::Failed;
            job.error = Some("its container is gone".into());
            return Ok(());
        };
        if status.running {
            return Ok(());
        }
        if status.exit_code != Some(0) {
            let logs = d.job_logs(&name, 30).await.unwrap_or_default();
            job.status = JobStatus::Failed;
            job.error = Some(format!(
                "training exited with {}: {}",
                status.exit_code.unwrap_or(-1),
                logs.trim().lines().last().unwrap_or_default()
            ));
            return Ok(());
        }
        let serve = gai_sandbox::deploy::DeploySpec {
            name: name.clone(),
            server: gai_sandbox::deploy::ServerKind::Vllm,
            model: Some(job.spec.base_model.clone()),
            gpu: true,
            port: None,
            context: None,
            tools: None,
            tool_parser: None,
            image: None,
            args: vec![
                "--enable-lora".into(),
                "--max-lora-rank".into(),
                job.spec.lora_rank.unwrap_or(16).max(16).to_string(),
                "--lora-modules".into(),
                format!("{}=/root/.cache/gai-lora/{}", job.spec.name, job.spec.name),
            ],
            env: Default::default(),
        };
        d.deploy(serve).await?;
        job.status = JobStatus::Succeeded;
        job.model = Some(format!(
            "{}/{}",
            crate::deployments::provider_name(&name),
            job.spec.name
        ));
        Ok(())
    }

    async fn finish(
        &self,
        job: &mut FineTuneJob,
        provider: &str,
        model: &str,
        add: AddModel<'_>,
    ) -> Result<(), Error> {
        let base = self
            .models
            .resolve(&format!("{provider}/{}", job.spec.base_model))
            .ok();
        let caps = base
            .as_ref()
            .map(|i| i.capabilities.clone())
            .unwrap_or_default();
        add(provider, ModelConfig::chat(model, caps, None)).await?;
        job.status = JobStatus::Succeeded;
        job.model = Some(format!("{provider}/{model}"));
        Ok(())
    }

    /// Ask the provider to stop a running job.
    pub async fn cancel(&self, name: &str) -> Result<FineTuneJob, Error> {
        let mut job = self.get(name).await?;
        if job.status.finished() {
            return Ok(job);
        }
        if let Some(id) = &job.job_id {
            match job.spec.trainer {
                Trainer::Openai => {
                    let (base, key) = self.key("openai")?;
                    self.call(
                        self.http
                            .post(format!("{base}/fine_tuning/jobs/{id}/cancel"))
                            .bearer_auth(&key),
                    )
                    .await?;
                }
                Trainer::Together => {
                    let (base, key) = self.key("together")?;
                    self.call(
                        self.http
                            .post(format!("{base}/fine-tunes/{id}/cancel"))
                            .bearer_auth(&key),
                    )
                    .await?;
                }
                Trainer::Local =>
                {
                    #[cfg(feature = "sandbox")]
                    if let Some(d) = &self.local {
                        d.remove_job(&format!("ft-{name}")).await?;
                    }
                }
            }
        }
        job.status = JobStatus::Cancelled;
        job.updated_at = Utc::now();
        self.put(&job).await?;
        Ok(job)
    }
}

fn bad(what: &str) -> Error {
    Error::Invalid(format!("the provider's answer had {what}"))
}

/// The training script for local jobs (transformers, peft and trl).
#[cfg(feature = "sandbox")]
const TRAIN_PY: &str = include_str!("train_lora.py");

/// A local LoRA training job: the base model and adapters share vLLM's
/// cache volume, so the deployment that serves it finds both.
#[cfg(feature = "sandbox")]
pub fn local_job(spec: &FineTuneSpec, data: String) -> gai_sandbox::deploy::JobSpec {
    let dir = format!("/root/.cache/gai-lora/{}", spec.name);
    let mut env = vec![
        format!("GAI_BASE_MODEL={}", spec.base_model),
        format!("GAI_OUT={dir}"),
        format!("GAI_EPOCHS={}", spec.epochs.unwrap_or(3)),
        format!("GAI_LR={}", spec.learning_rate.unwrap_or(2e-4)),
        format!("GAI_LORA_RANK={}", spec.lora_rank.unwrap_or(16)),
        "HF_HOME=/root/.cache/huggingface".into(),
    ];
    if let Ok(t) = std::env::var("HF_TOKEN") {
        env.push(format!("HF_TOKEN={t}"));
    }
    gai_sandbox::deploy::JobSpec {
        name: format!("ft-{}", spec.name),
        image: "docker.io/pytorch/pytorch:latest".into(),
        cmd: vec![
            "sh".into(),
            "-c".into(),
            "pip install --quiet 'transformers>=4.45' 'peft>=0.13' 'trl>=0.12' datasets accelerate && python /gai/train_lora.py".into(),
        ],
        env,
        files: vec![
            ("/gai/train.jsonl".into(), data.into_bytes()),
            ("/gai/train_lora.py".into(), TRAIN_PY.as_bytes().to_vec()),
        ],
        volume: Some(("gai-models-vllm".into(), "/root/.cache".into())),
        gpu: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_calls_become_openai_messages() {
        let m = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::text("Let me check."),
                ContentBlock::ToolUse {
                    id: "c1".into(),
                    name: "calculator".into(),
                    input: json!({"expression": "2+2"}),
                },
            ],
        };
        let out = openai_messages(&m).unwrap();
        assert_eq!(out[0]["role"], "assistant");
        assert_eq!(
            out[0]["tool_calls"][0]["function"]["arguments"],
            "{\"expression\":\"2+2\"}"
        );
        let r = Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "c1".into(),
                content: vec![ContentBlock::text("4")],
                is_error: false,
            }],
        };
        assert_eq!(
            openai_messages(&r).unwrap(),
            [json!({"role": "tool", "tool_call_id": "c1", "content": "4"})]
        );
        let spec =
            FineTunes::parse(json!({"name": "Bad Name", "trainer": "openai", "base_model": "x"}));
        assert!(spec.is_err());
    }

    #[cfg(feature = "sandbox")]
    #[test]
    fn local_jobs_train_into_vllms_cache() {
        let spec = FineTunes::parse(json!({
            "name": "support", "trainer": "local", "base_model": "Qwen/Qwen2.5-1.5B-Instruct", "epochs": 2
        }))
        .unwrap();
        let job = local_job(&spec, "{}\n".into());
        assert_eq!(job.container(), "gai-job-ft-support");
        assert!(job.gpu);
        assert_eq!(
            job.volume,
            Some(("gai-models-vllm".into(), "/root/.cache".into()))
        );
        assert!(
            job.env
                .contains(&"GAI_OUT=/root/.cache/gai-lora/support".to_owned())
        );
        assert!(job.env.contains(&"GAI_EPOCHS=2".to_owned()));
        let files: Vec<&str> = job.files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(files, ["/gai/train.jsonl", "/gai/train_lora.py"]);
        assert!(job.cmd[2].ends_with("python /gai/train_lora.py"));
    }

    /// Rated runs become a training set, OpenAI trains on it (a fake one
    /// here), and the model it made can be picked like any other.
    #[tokio::test]
    async fn trains_on_rated_runs_and_adds_the_model() {
        use wiremock::matchers::{body_partial_json, method, path};
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(method("POST"))
            .and(path("/v1/files"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(json!({"id": "file-1"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        wiremock::Mock::given(method("POST"))
            .and(path("/v1/fine_tuning/jobs"))
            .and(body_partial_json(json!({"training_file": "file-1", "model": "gpt-4.1-mini-2025-04-14", "suffix": "support"})))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(json!({"id": "ftjob-1", "status": "queued"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        wiremock::Mock::given(method("GET"))
            .and(path("/v1/fine_tuning/jobs/ftjob-1"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "id": "ftjob-1", "status": "succeeded",
                "fine_tuned_model": "ft:gpt-4.1-mini-2025-04-14:acme:support:abc123"
            })))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let mut openai = gai_llm::ProviderConfig::new("openai", &format!("{}/v1", server.uri()));
        openai.api_key = Some("sk-test".into());
        let mut config = crate::Config {
            data_dir: Some(dir.path().to_owned()),
            default_model: Some("mock/echo".into()),
            ..Default::default()
        };
        config.llm.providers.insert("openai".into(), openai);
        let gai = crate::Gai::from_config(config, None).unwrap();
        let store = gai.runtime().await.unwrap().store().clone();
        let agent = crate::agent::AgentSpec::new("support").system("You help customers.");
        for i in 0..12 {
            let out = gai
                .agent_spec(agent.clone())
                .run(format!("question {i}"))
                .await
                .unwrap();
            let run = store.get_run(&out.run_id).await.unwrap().unwrap();
            let rating = if i < 10 { 1 } else { -1 };
            store
                .put_feedback(&run, "ana", rating, None, &[])
                .await
                .unwrap();
        }

        let filter = DatasetFilter {
            agent: Some("support".into()),
            ..Default::default()
        };
        let (data, stats) = build_dataset(&store, &filter).await.unwrap();
        assert_eq!(stats.examples, 10, "{:?}", stats.skipped);
        let first: Value = serde_json::from_str(data.lines().next().unwrap()).unwrap();
        assert_eq!(first["messages"][0]["role"], "system");
        assert!(
            first["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains("You help customers.")
        );
        assert_eq!(first["messages"][1]["role"], "user");
        assert_eq!(
            first["messages"].as_array().unwrap().last().unwrap()["role"],
            "assistant"
        );

        let ft = gai.fine_tunes().await.unwrap();
        let job = ft
            .start(
                FineTunes::parse(json!({
                    "name": "support", "trainer": "openai",
                    "base_model": "gpt-4.1-mini-2025-04-14", "dataset": {"agent": "support"}
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(job.status, JobStatus::Running, "{:?}", job.error);
        assert_eq!(job.examples, 10);
        assert_eq!(job.job_id.as_deref(), Some("ftjob-1"));

        let changed = gai.poll_fine_tunes().await.unwrap();
        assert_eq!(changed.len(), 1);
        let model = changed[0].model.clone().unwrap();
        assert_eq!(
            model,
            "openai/ft:gpt-4.1-mini-2025-04-14:acme:support:abc123"
        );
        assert!(gai.models().resolve(&model).is_ok());
        assert!(
            gai.models()
                .catalog()
                .iter()
                .any(|e| e.info.model == "ft:gpt-4.1-mini-2025-04-14:acme:support:abc123")
        );
        assert_eq!(
            ft.get("support").await.unwrap().status,
            JobStatus::Succeeded
        );
        assert!(
            ft.start(
                FineTunes::parse(json!({
                    "name": "tiny", "trainer": "openai", "base_model": "gpt-4.1-mini-2025-04-14",
                    "dataset": {"agent": "nobody"}
                }))
                .unwrap()
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("rate more")
        );
    }
}
