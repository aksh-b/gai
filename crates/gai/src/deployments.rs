//! Model servers gai runs: specs kept in the database, containers started
//! through the engine, and their models added to the catalog as the
//! provider `deploy-<name>` once the server answers.

use std::sync::Arc;
use std::time::Duration;

use gai_llm::{Capabilities, ModelConfig, ModelRegistry, ProviderConfig};
use gai_sandbox::deploy::{ContainerStatus, DeploySpec, Gpu, JobSpec, ModelEngine, ServerKind};
use gai_store::SharedStore;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{DeploySettings, Error};

const PREFIX: &str = "deploy.";

/// What is kept per deployment.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Record {
    spec: DeploySpec,
    /// The host port it was last published on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
}

/// Whether the server answers, and what it serves.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct Health {
    pub ok: bool,
    pub models: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct DeploymentView {
    pub spec: DeploySpec,
    /// The models' provider: pick them as `deploy-<name>/<model>`.
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// The container, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<ContainerStatus>,
    pub health: Health,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct GpuStatus {
    /// NVIDIA GPUs on this machine (from nvidia-smi).
    pub gpus: Vec<Gpu>,
    /// Whether the container engine can hand GPUs to containers.
    pub engine_gpu: bool,
}

#[derive(Clone)]
pub struct Deployments {
    store: SharedStore,
    models: Arc<ModelRegistry>,
    engine: Arc<dyn ModelEngine>,
    settings: DeploySettings,
    http: reqwest::Client,
}

pub fn provider_name(deployment: &str) -> String {
    format!("deploy-{deployment}")
}

impl Deployments {
    pub fn new(
        store: SharedStore,
        models: Arc<ModelRegistry>,
        engine: Arc<dyn ModelEngine>,
        settings: DeploySettings,
    ) -> Self {
        Self {
            store,
            models,
            engine,
            settings,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("http client"),
        }
    }

    pub fn parse(value: Value) -> Result<DeploySpec, Error> {
        let spec: DeploySpec = serde_json::from_value(value)
            .map_err(|e| Error::Invalid(format!("deployment: {e}")))?;
        spec.validate().map_err(Error::Invalid)?;
        Ok(spec)
    }

    async fn records(&self) -> Result<Vec<Record>, Error> {
        Ok(self
            .store
            .list_settings(PREFIX)
            .await?
            .into_iter()
            .filter_map(|(_, v)| serde_json::from_str(&v).ok())
            .collect())
    }

    async fn record(&self, name: &str) -> Result<Record, Error> {
        let v = self
            .store
            .get_setting(&format!("{PREFIX}{name}"))
            .await?
            .ok_or_else(|| Error::NotFound(format!("no deployment {name}")))?;
        serde_json::from_str(&v).map_err(|e| Error::Invalid(e.to_string()))
    }

    async fn put(&self, r: &Record) -> Result<(), Error> {
        self.store
            .put_setting(
                &format!("{PREFIX}{}", r.spec.name),
                &serde_json::to_string(r).expect("record"),
            )
            .await?;
        Ok(())
    }

    fn base_url(&self, spec: &DeploySpec, port: u16) -> String {
        let root = format!("http://{}:{port}", self.settings.host);
        match spec.server {
            ServerKind::Ollama | ServerKind::Vllm | ServerKind::Llamacpp => format!("{root}/v1"),
        }
    }

    /// Put the provider in the catalog (at startup, from the last known
    /// port, so agents naming its models resolve before it is checked).
    fn register(&self, spec: &DeploySpec, port: u16) {
        let mut cfg = ProviderConfig::new("openai", &self.base_url(spec, port));
        cfg.default_capabilities = Some(Capabilities {
            tools: spec.tool_calls(),
            context_window: spec.context,
            ..Default::default()
        });
        // Local servers are slow to load a model on the first request.
        cfg.timeout_secs = Some(600);
        if let Err(e) = self.models.add_provider(&provider_name(&spec.name), cfg) {
            tracing::warn!(deployment = %spec.name, error = %e, "could not add a deployment's provider");
        }
    }

    /// Register every saved deployment's provider. Call once at startup.
    pub async fn restore(&self) -> Result<(), Error> {
        for r in self.records().await? {
            if let Some(port) = r.spec.port.or(r.port) {
                self.register(&r.spec, port);
            }
        }
        Ok(())
    }

    /// Save a deployment and start it.
    pub async fn deploy(&self, spec: DeploySpec) -> Result<DeploymentView, Error> {
        spec.validate().map_err(Error::Invalid)?;
        let port = self.record(&spec.name).await.ok().and_then(|r| r.port);
        self.put(&Record {
            spec: spec.clone(),
            port,
        })
        .await?;
        self.start(&spec.name).await
    }

    /// Start (or restart after a change) a saved deployment. Downloading
    /// the image and the model goes on in the background; [`get`](Self::get)
    /// says when the server answers.
    pub async fn start(&self, name: &str) -> Result<DeploymentView, Error> {
        let mut r = self.record(name).await?;
        self.engine
            .start(&r.spec, &self.settings.bind)
            .await
            .map_err(|e| Error::Invalid(e.to_string()))?;
        if let Ok(Some(status)) = self.engine.status(&r.spec).await
            && let Some(port) = status.host_port
        {
            r.port = Some(port);
            self.put(&r).await?;
            self.register(&r.spec, port);
            if r.spec.server == ServerKind::Ollama
                && let Some(model) = r.spec.model.clone()
            {
                let me = self.clone();
                let spec = r.spec.clone();
                tokio::spawn(async move { me.ollama_pull(&spec, port, &model).await });
            }
        }
        self.get(name).await
    }

    /// Ollama pulls models on request: wait for it to answer, then pull.
    async fn ollama_pull(&self, spec: &DeploySpec, port: u16, model: &str) {
        let root = format!("http://{}:{port}", self.settings.host);
        for _ in 0..120 {
            if self
                .http
                .get(format!("{root}/api/tags"))
                .send()
                .await
                .is_ok()
            {
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        for m in model.split(',').map(str::trim).filter(|m| !m.is_empty()) {
            let r = self
                .http
                .post(format!("{root}/api/pull"))
                .timeout(Duration::from_secs(6 * 3600))
                .json(&json!({"model": m, "stream": false}))
                .send()
                .await;
            match r {
                Ok(r) if r.status().is_success() => {
                    tracing::info!(deployment = %spec.name, model = m, "pulled");
                }
                Ok(r) => {
                    tracing::warn!(deployment = %spec.name, model = m, status = %r.status(), "pull failed")
                }
                Err(e) => {
                    tracing::warn!(deployment = %spec.name, model = m, error = %e, "pull failed")
                }
            }
        }
    }

    pub async fn stop(&self, name: &str) -> Result<DeploymentView, Error> {
        let r = self.record(name).await?;
        self.engine
            .stop(&r.spec)
            .await
            .map_err(|e| Error::Invalid(e.to_string()))?;
        self.get(name).await
    }

    /// Stop and delete the container, forget the deployment and its models.
    /// Downloaded weights stay in their volume for next time.
    pub async fn delete(&self, name: &str) -> Result<bool, Error> {
        let Ok(r) = self.record(name).await else {
            return Ok(false);
        };
        self.engine
            .remove(&r.spec)
            .await
            .map_err(|e| Error::Invalid(e.to_string()))?;
        self.models.remove_provider(&provider_name(name));
        Ok(self
            .store
            .delete_setting(&format!("{PREFIX}{name}"))
            .await?)
    }

    pub async fn logs(&self, name: &str, lines: usize) -> Result<String, Error> {
        let r = self.record(name).await?;
        self.engine
            .logs(&r.spec, lines.clamp(1, 5000))
            .await
            .map_err(|e| Error::Invalid(e.to_string()))
    }

    pub async fn list(&self) -> Result<Vec<DeploymentView>, Error> {
        let mut out = Vec::new();
        for r in self.records().await? {
            out.push(self.view(r).await);
        }
        Ok(out)
    }

    pub async fn get(&self, name: &str) -> Result<DeploymentView, Error> {
        let r = self.record(name).await?;
        Ok(self.view(r).await)
    }

    /// The container's state and the server's health; models it serves
    /// are added to the catalog.
    async fn view(&self, r: Record) -> DeploymentView {
        let container = match self.engine.status(&r.spec).await {
            Ok(c) => c,
            Err(e) => {
                return DeploymentView {
                    provider: provider_name(&r.spec.name),
                    base_url: None,
                    container: None,
                    health: Health {
                        ok: false,
                        models: Vec::new(),
                        error: Some(e.to_string()),
                    },
                    spec: r.spec,
                };
            }
        };
        let port = container.as_ref().and_then(|c| c.host_port).or(r.port);
        let running = container.as_ref().is_some_and(|c| c.running);
        let health = match port {
            Some(p) if running => self.health(&r.spec, p).await,
            _ => Health {
                ok: false,
                models: Vec::new(),
                error: Some(if container.is_none() {
                    "not started".into()
                } else {
                    "stopped".into()
                }),
            },
        };
        if health.ok
            && let Some(p) = port
        {
            if r.port != Some(p) {
                self.register(&r.spec, p);
                let _ = self
                    .put(&Record {
                        spec: r.spec.clone(),
                        port: Some(p),
                    })
                    .await;
            }
            for m in &health.models {
                let caps = Capabilities {
                    tools: r.spec.tool_calls(),
                    context_window: r.spec.context,
                    ..Default::default()
                };
                let _ = self.models.add_model(
                    &provider_name(&r.spec.name),
                    ModelConfig::chat(m, caps, None),
                );
            }
        }
        DeploymentView {
            provider: provider_name(&r.spec.name),
            base_url: port.map(|p| self.base_url(&r.spec, p)),
            container,
            health,
            spec: r.spec,
        }
    }

    async fn health(&self, spec: &DeploySpec, port: u16) -> Health {
        let url = format!("{}/models", self.base_url(spec, port));
        let r = match self.http.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                return Health {
                    ok: false,
                    models: Vec::new(),
                    error: Some(if e.is_connect() || e.is_timeout() {
                        "starting (or downloading the model)".into()
                    } else {
                        e.to_string()
                    }),
                };
            }
        };
        if !r.status().is_success() {
            return Health {
                ok: false,
                models: Vec::new(),
                error: Some(format!("{url} answered {}", r.status())),
            };
        }
        let v: Value = r.json().await.unwrap_or_default();
        let models = v["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| m["id"].as_str().map(str::to_owned))
            .collect();
        Health {
            ok: true,
            models,
            error: None,
        }
    }

    /// Run a one-off container (a training job) on the same engine.
    pub async fn start_job(&self, job: &JobSpec) -> Result<(), Error> {
        self.engine
            .start_job(job)
            .await
            .map_err(|e| Error::Invalid(e.to_string()))
    }

    pub async fn job_status(&self, name: &str) -> Result<Option<ContainerStatus>, Error> {
        self.engine
            .job_status(name)
            .await
            .map_err(|e| Error::Invalid(e.to_string()))
    }

    pub async fn job_logs(&self, name: &str, lines: usize) -> Result<String, Error> {
        self.engine
            .job_logs(name, lines)
            .await
            .map_err(|e| Error::Invalid(e.to_string()))
    }

    pub async fn remove_job(&self, name: &str) -> Result<(), Error> {
        self.engine
            .remove_job(name)
            .await
            .map_err(|e| Error::Invalid(e.to_string()))
    }

    pub async fn gpus(&self) -> GpuStatus {
        GpuStatus {
            gpus: gai_sandbox::deploy::gpus().await,
            engine_gpu: self.engine.gpu_runtime().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use gai_sandbox::deploy::DeployError;

    use super::*;

    /// "Runs" a model server by pointing at a mock HTTP server.
    struct FakeEngine {
        port: u16,
        running: Mutex<bool>,
    }

    #[async_trait]
    impl ModelEngine for FakeEngine {
        async fn start(&self, _: &DeploySpec, bind: &str) -> Result<(), DeployError> {
            assert_eq!(bind, "127.0.0.1");
            *self.running.lock().unwrap() = true;
            Ok(())
        }
        async fn stop(&self, _: &DeploySpec) -> Result<(), DeployError> {
            *self.running.lock().unwrap() = false;
            Ok(())
        }
        async fn remove(&self, s: &DeploySpec) -> Result<(), DeployError> {
            self.stop(s).await
        }
        async fn status(&self, _: &DeploySpec) -> Result<Option<ContainerStatus>, DeployError> {
            let running = *self.running.lock().unwrap();
            Ok(Some(ContainerStatus {
                state: if running { "running" } else { "exited" }.into(),
                running,
                host_port: Some(self.port),
                ..Default::default()
            }))
        }
        async fn logs(&self, _: &DeploySpec, _: usize) -> Result<String, DeployError> {
            Ok("listening".into())
        }
        async fn gpu_runtime(&self) -> bool {
            false
        }
    }

    #[tokio::test]
    async fn a_deployed_server_becomes_a_provider() {
        use wiremock::matchers::{method, path};
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(json!({"data": [{"id": "ggml-org/gemma-3-1b-it-GGUF"}]})),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw(
                [
                    json!({"id": "x", "object": "chat.completion.chunk", "created": 0, "model": "m",
                        "choices": [{"index": 0, "delta": {"role": "assistant", "content": "hello from gemma"}, "finish_reason": null}]}),
                    json!({"id": "x", "object": "chat.completion.chunk", "created": 0, "model": "m",
                        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]}),
                ]
                .iter()
                .map(|c| format!("data: {c}\n\n"))
                .chain(["data: [DONE]\n\n".to_owned()])
                .collect::<String>(),
                "text/event-stream",
            ))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let store: SharedStore = Arc::new(
            gai_store::SqliteStore::open(&dir.path().join("gai.db"))
                .await
                .unwrap(),
        );
        let models = Arc::new(ModelRegistry::from_config(Default::default()).unwrap());
        let port = server.address().port();
        let engine = Arc::new(FakeEngine {
            port,
            running: Mutex::new(false),
        });
        let d = Deployments::new(
            store.clone(),
            models.clone(),
            engine,
            DeploySettings::default(),
        );

        let spec = Deployments::parse(json!({
            "name": "gemma", "server": "llamacpp", "model": "ggml-org/gemma-3-1b-it-GGUF", "context": 8192
        }))
        .unwrap();
        let v = d.deploy(spec).await.unwrap();
        assert!(v.health.ok, "{:?}", v.health);
        assert_eq!(v.provider, "deploy-gemma");
        assert_eq!(
            v.base_url.as_deref(),
            Some(format!("http://127.0.0.1:{port}/v1").as_str())
        );
        let info = models
            .resolve("deploy-gemma/ggml-org/gemma-3-1b-it-GGUF")
            .unwrap();
        assert!(info.capabilities.tools);
        assert_eq!(info.capabilities.context_window, Some(8192));
        let reply = models
            .chat("deploy-gemma/ggml-org/gemma-3-1b-it-GGUF")
            .unwrap()
            .complete(gai_llm::ChatRequest::new(vec![gai_core::Message::user(
                "hi",
            )]))
            .await
            .unwrap();
        assert_eq!(reply.text(), "hello from gemma");

        // After a restart the provider is back before anything is checked.
        let fresh = Arc::new(ModelRegistry::from_config(Default::default()).unwrap());
        let again = Deployments::new(
            store.clone(),
            fresh.clone(),
            Arc::new(FakeEngine {
                port,
                running: Mutex::new(false),
            }),
            DeploySettings::default(),
        );
        again.restore().await.unwrap();
        assert!(fresh.resolve("deploy-gemma/anything").is_ok());

        let v = d.stop("gemma").await.unwrap();
        assert!(!v.health.ok);
        assert_eq!(v.health.error.as_deref(), Some("stopped"));
        assert_eq!(d.logs("gemma", 50).await.unwrap(), "listening");
        assert!(d.delete("gemma").await.unwrap());
        assert!(models.resolve("deploy-gemma/x").is_err());
        assert!(d.list().await.unwrap().is_empty());
    }
}
