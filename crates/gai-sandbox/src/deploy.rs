//! Model servers in containers: Ollama, vLLM and llama.cpp, started and
//! stopped through Docker or Podman.
//!
//! Each deployment is one container named `gai-model-<name>`, labelled
//! `gai.deployment=<name>`, with its port published on the host and a
//! named volume for downloaded weights, so a restart doesn't download them
//! again.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use async_trait::async_trait;
use bollard::Docker;
use bollard::errors::Error as DockerError;
use bollard::models::{
    ContainerCreateBody, DeviceRequest, HostConfig, Mount, MountType, PortBinding, RestartPolicy,
    RestartPolicyNameEnum,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, LogsOptionsBuilder,
    RemoveContainerOptionsBuilder, StopContainerOptionsBuilder, UploadToContainerOptionsBuilder,
};
use futures::StreamExt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The kind of model server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ServerKind {
    /// Ollama; `model` names models to pull (`qwen3:8b`).
    Ollama,
    /// vLLM (needs an NVIDIA GPU); `model` is a Hugging Face id.
    Vllm,
    /// llama.cpp's server; `model` is a Hugging Face GGUF repo
    /// (`ggml-org/gemma-3-1b-it-GGUF`, optionally `:Q4_K_M`) or a URL to a
    /// `.gguf` file.
    Llamacpp,
}

impl ServerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ollama => "ollama",
            Self::Vllm => "vllm",
            Self::Llamacpp => "llamacpp",
        }
    }

    /// The port the server listens on inside its container.
    pub fn port(self) -> u16 {
        match self {
            Self::Ollama => 11434,
            Self::Vllm => 8000,
            Self::Llamacpp => 8080,
        }
    }

    fn default_image(self, gpu: bool) -> &'static str {
        match (self, gpu) {
            (Self::Ollama, _) => "docker.io/ollama/ollama:latest",
            (Self::Vllm, _) => "docker.io/vllm/vllm-openai:latest",
            (Self::Llamacpp, false) => "ghcr.io/ggml-org/llama.cpp:server",
            (Self::Llamacpp, true) => "ghcr.io/ggml-org/llama.cpp:server-cuda",
        }
    }

    /// Where the server keeps downloaded weights.
    fn cache_dir(self) -> &'static str {
        match self {
            Self::Ollama => "/root/.ollama",
            Self::Vllm | Self::Llamacpp => "/root/.cache",
        }
    }
}

/// A model server to run.
///
/// ```yaml
/// name: qwen
/// server: vllm
/// model: Qwen/Qwen3-8B
/// gpu: true
/// context: 32768
/// tools: true
/// env: { HF_TOKEN: "${HF_TOKEN}" }
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeploySpec {
    /// Letters, digits and `-`; the models show up as `deploy-<name>/<model>`.
    pub name: String,
    pub server: ServerKind,
    /// What to serve (see [`ServerKind`]). Optional for Ollama.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Use the NVIDIA GPUs (all of them).
    #[serde(default)]
    pub gpu: bool,
    /// Host port; a free one is picked when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Context window in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<u32>,
    /// Whether the models can call tools (default: yes for Ollama and
    /// llama.cpp; for vLLM it turns on its tool parser).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<bool>,
    /// vLLM's `--tool-call-parser` (default `hermes`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_parser: Option<String>,
    /// Another image than the usual one for this server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Extra command-line arguments for the server.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Environment for the server. `${VAR}` is read from gai's own
    /// environment, so tokens stay out of the spec.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl DeploySpec {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty()
            || self.name.len() > 40
            || !self
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(
                "a deployment's name is lowercase letters, digits and - (at most 40)".into(),
            );
        }
        if self.model.is_none() && self.server != ServerKind::Ollama {
            return Err(format!("{} needs a model", self.server.as_str()));
        }
        if self.server == ServerKind::Vllm && !self.gpu {
            return Err("vLLM needs a GPU: set gpu: true (or use llama.cpp or Ollama)".into());
        }
        if self.port == Some(0) {
            return Err("port can't be 0".into());
        }
        Ok(())
    }

    pub fn container(&self) -> String {
        format!("gai-model-{}", self.name)
    }

    pub fn image(&self) -> String {
        self.image
            .clone()
            .unwrap_or_else(|| self.server.default_image(self.gpu).into())
    }

    /// The server's command-line arguments.
    pub fn command(&self) -> Vec<String> {
        let port = self.server.port().to_string();
        let mut out: Vec<String> = Vec::new();
        match self.server {
            ServerKind::Ollama => {}
            ServerKind::Vllm => {
                let model = self.model.clone().unwrap_or_default();
                out.extend(["--model".into(), model.clone()]);
                out.extend(["--served-model-name".into(), model]);
                out.extend(["--host".into(), "0.0.0.0".into(), "--port".into(), port]);
                if let Some(c) = self.context {
                    out.extend(["--max-model-len".into(), c.to_string()]);
                }
                if self.tools == Some(true) {
                    out.push("--enable-auto-tool-choice".into());
                    out.extend([
                        "--tool-call-parser".into(),
                        self.tool_parser.clone().unwrap_or_else(|| "hermes".into()),
                    ]);
                }
            }
            ServerKind::Llamacpp => {
                let model = self.model.clone().unwrap_or_default();
                if model.starts_with("http://") || model.starts_with("https://") {
                    // Downloaded once into the weights volume.
                    let file = model
                        .split('?')
                        .next()
                        .and_then(|u| u.rsplit('/').next())
                        .filter(|f| !f.is_empty())
                        .unwrap_or("model.gguf")
                        .to_owned();
                    out.extend(["-m".into(), format!("/root/.cache/llama.cpp/{file}")]);
                    out.extend(["--model-url".into(), model]);
                } else {
                    out.extend(["-hf".into(), model]);
                }
                out.extend(["--host".into(), "0.0.0.0".into(), "--port".into(), port]);
                if let Some(c) = self.context {
                    out.extend(["-c".into(), c.to_string()]);
                }
                if self.gpu {
                    out.extend(["-ngl".into(), "999".into()]);
                }
                if self.tools != Some(false) {
                    out.push("--jinja".into());
                }
            }
        }
        out.extend(self.args.iter().cloned());
        out
    }

    /// `env` with `${VAR}` filled in from this process's environment.
    pub fn resolved_env(&self) -> Result<Vec<String>, String> {
        let mut out = Vec::new();
        for (k, v) in &self.env {
            out.push(format!("{k}={}", expand(v)?));
        }
        if self.server == ServerKind::Ollama {
            out.push("OLLAMA_HOST=0.0.0.0".into());
            if let Some(c) = self.context {
                out.push(format!("OLLAMA_CONTEXT_LENGTH={c}"));
            }
        }
        Ok(out)
    }

    /// The volume for downloaded weights, shared by deployments of one kind.
    pub fn volume(&self) -> String {
        format!("gai-models-{}", self.server.as_str())
    }

    /// Whether the models can be sent tools.
    pub fn tool_calls(&self) -> bool {
        self.tools.unwrap_or(self.server != ServerKind::Vllm)
    }
}

fn expand(v: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = v;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let end = after
            .find('}')
            .ok_or_else(|| format!("unclosed ${{ in {v:?}"))?;
        let var = &after[..end];
        out.push_str(&std::env::var(var).map_err(|_| format!("set {var} for the model server"))?);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[derive(Debug, thiserror::Error)]
pub enum DeployError {
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Failed(String),
    #[error("{0}")]
    Invalid(String),
}

/// What the engine says about a deployment's container.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContainerStatus {
    /// `created`, `running`, `exited`, `restarting`, ...
    pub state: String,
    pub running: bool,
    /// The host port the server is published on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub image: String,
}

/// A one-off container: a training run, say. Its files are copied in
/// before it starts; its volume keeps what it writes.
#[derive(Clone, Debug, PartialEq)]
pub struct JobSpec {
    /// Letters, digits and `-`; the container is `gai-job-<name>`.
    pub name: String,
    pub image: String,
    pub cmd: Vec<String>,
    pub env: Vec<String>,
    /// Absolute paths in the container, and their contents.
    pub files: Vec<(String, Vec<u8>)>,
    /// A named volume and where it is mounted.
    pub volume: Option<(String, String)>,
    pub gpu: bool,
}

impl JobSpec {
    pub fn container(&self) -> String {
        format!("gai-job-{}", self.name)
    }
}

/// Starts and stops model servers. [`DockerEngine`] is the real one.
#[async_trait]
pub trait ModelEngine: Send + Sync {
    /// Create the container if needed (again, when the spec changed) and
    /// start it. Pulls the image first, which can take a while.
    async fn start(&self, spec: &DeploySpec, bind: &str) -> Result<(), DeployError>;
    async fn stop(&self, spec: &DeploySpec) -> Result<(), DeployError>;
    /// Stop and delete the container (the weights volume stays).
    async fn remove(&self, spec: &DeploySpec) -> Result<(), DeployError>;
    async fn status(&self, spec: &DeploySpec) -> Result<Option<ContainerStatus>, DeployError>;
    async fn logs(&self, spec: &DeploySpec, lines: usize) -> Result<String, DeployError>;
    /// Whether the engine can give containers NVIDIA GPUs.
    async fn gpu_runtime(&self) -> bool;

    /// Start a job (replacing a finished one of the same name).
    async fn start_job(&self, _job: &JobSpec) -> Result<(), DeployError> {
        Err(DeployError::Unavailable("this engine runs no jobs".into()))
    }

    /// A job's container: running, or exited with its code.
    async fn job_status(&self, _name: &str) -> Result<Option<ContainerStatus>, DeployError> {
        Ok(None)
    }

    async fn job_logs(&self, _name: &str, _lines: usize) -> Result<String, DeployError> {
        Ok(String::new())
    }

    /// Stop a job and delete its container.
    async fn remove_job(&self, _name: &str) -> Result<(), DeployError> {
        Ok(())
    }
}

pub struct DockerEngine {
    docker: Result<Docker, String>,
}

impl DockerEngine {
    /// `engine` as in `[sandbox] engine`; else `DOCKER_HOST` or the usual
    /// sockets.
    pub fn new(engine: Option<&str>) -> Self {
        let docker = match crate::container::find_engine(engine) {
            None => Err(
                "no container engine found: start Docker or Podman, or set DOCKER_HOST".to_owned(),
            ),
            Some(addr) => Docker::connect_with_host(&addr)
                // Pulling a vLLM image is several gigabytes.
                .map(|d| d.with_timeout(Duration::from_secs(3600)))
                .map_err(|e| format!("container engine at {addr}: {e}")),
        };
        Self { docker }
    }

    fn docker(&self) -> Result<&Docker, DeployError> {
        self.docker
            .as_ref()
            .map_err(|e| DeployError::Unavailable(e.clone()))
    }
}

fn failed(e: impl std::fmt::Display) -> DeployError {
    DeployError::Failed(e.to_string())
}

fn not_found(e: &DockerError) -> bool {
    matches!(
        e,
        DockerError::DockerResponseServerError {
            status_code: 404,
            ..
        }
    )
}

/// Pull `image` unless the engine has it.
async fn ensure_image(docker: &Docker, image: &str) -> Result<(), DeployError> {
    match docker.inspect_image(image).await {
        Ok(_) => Ok(()),
        Err(e) if not_found(&e) => {
            tracing::info!(image, "pulling an image");
            let opts = CreateImageOptionsBuilder::new().from_image(image).build();
            let mut pull = docker.create_image(Some(opts), None, None);
            while let Some(step) = pull.next().await {
                step.map_err(|e| DeployError::Failed(format!("could not pull {image}: {e}")))?;
            }
            Ok(())
        }
        Err(e) => Err(DeployError::Unavailable(format!("container engine: {e}"))),
    }
}

/// The last `lines` lines a container wrote.
async fn logs_of(docker: &Docker, container: &str, lines: usize) -> Result<String, DeployError> {
    let opts = LogsOptionsBuilder::new()
        .stdout(true)
        .stderr(true)
        .tail(&lines.to_string())
        .build();
    let mut out = String::new();
    let mut s = docker.logs(container, Some(opts));
    while let Some(chunk) = s.next().await {
        match chunk {
            Ok(c) => out.push_str(&String::from_utf8_lossy(&c.into_bytes())),
            Err(e) if not_found(&e) => return Ok(String::new()),
            Err(e) => return Err(failed(e)),
        }
    }
    Ok(out)
}

/// A fingerprint of what the container was made from.
fn fingerprint(spec: &DeploySpec, bind: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(serde_json::to_vec(spec).expect("spec"));
    h.update(bind.as_bytes());
    h.finalize()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[async_trait]
impl ModelEngine for DockerEngine {
    async fn start(&self, spec: &DeploySpec, bind: &str) -> Result<(), DeployError> {
        let docker = self.docker()?;
        let name = spec.container();
        let print = fingerprint(spec, bind);
        match docker.inspect_container(&name, None).await {
            Ok(c) => {
                let same = c
                    .config
                    .as_ref()
                    .and_then(|c| c.labels.as_ref())
                    .and_then(|l| l.get("gai.spec"))
                    .is_some_and(|p| *p == print);
                if same {
                    if c.state.and_then(|s| s.running) != Some(true) {
                        docker.start_container(&name, None).await.map_err(failed)?;
                    }
                    return Ok(());
                }
                let opts = RemoveContainerOptionsBuilder::new().force(true).build();
                docker
                    .remove_container(&name, Some(opts))
                    .await
                    .map_err(failed)?;
            }
            Err(e) if not_found(&e) => {}
            Err(e) => return Err(DeployError::Unavailable(format!("container engine: {e}"))),
        }
        let image = spec.image();
        ensure_image(docker, &image).await?;
        let port = format!("{}/tcp", spec.server.port());
        let bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::from([(
            port.clone(),
            Some(vec![PortBinding {
                host_ip: Some(bind.to_owned()),
                host_port: Some(spec.port.map(|p| p.to_string()).unwrap_or_default()),
            }]),
        )]);
        let labels = HashMap::from([
            ("gai.deployment".to_owned(), spec.name.clone()),
            ("gai.server".to_owned(), spec.server.as_str().to_owned()),
            ("gai.spec".to_owned(), print),
        ]);
        let host = HostConfig {
            port_bindings: Some(bindings),
            mounts: Some(vec![Mount {
                target: Some(spec.server.cache_dir().into()),
                source: Some(spec.volume()),
                typ: Some(MountType::VOLUME),
                ..Default::default()
            }]),
            restart_policy: Some(RestartPolicy {
                name: Some(RestartPolicyNameEnum::UNLESS_STOPPED),
                maximum_retry_count: None,
            }),
            // vLLM's workers share memory through /dev/shm.
            ipc_mode: (spec.server == ServerKind::Vllm).then(|| "host".to_owned()),
            device_requests: spec.gpu.then(|| {
                vec![DeviceRequest {
                    count: Some(-1),
                    capabilities: Some(vec![vec!["gpu".to_owned()]]),
                    ..Default::default()
                }]
            }),
            ..Default::default()
        };
        let cmd = spec.command();
        let body = ContainerCreateBody {
            image: Some(image),
            cmd: (!cmd.is_empty()).then_some(cmd),
            env: Some(spec.resolved_env().map_err(DeployError::Invalid)?),
            exposed_ports: Some(vec![port]),
            labels: Some(labels),
            host_config: Some(host),
            ..Default::default()
        };
        let opts = CreateContainerOptionsBuilder::new().name(&name).build();
        docker
            .create_container(Some(opts), body)
            .await
            .map_err(failed)?;
        docker.start_container(&name, None).await.map_err(failed)?;
        Ok(())
    }

    async fn stop(&self, spec: &DeploySpec) -> Result<(), DeployError> {
        let opts = StopContainerOptionsBuilder::new().t(20).build();
        match self
            .docker()?
            .stop_container(&spec.container(), Some(opts))
            .await
        {
            Ok(()) => Ok(()),
            Err(e) if not_found(&e) => Ok(()),
            Err(DockerError::DockerResponseServerError {
                status_code: 304, ..
            }) => Ok(()),
            Err(e) => Err(failed(e)),
        }
    }

    async fn remove(&self, spec: &DeploySpec) -> Result<(), DeployError> {
        let opts = RemoveContainerOptionsBuilder::new().force(true).build();
        match self
            .docker()?
            .remove_container(&spec.container(), Some(opts))
            .await
        {
            Ok(()) => Ok(()),
            Err(e) if not_found(&e) => Ok(()),
            Err(e) => Err(failed(e)),
        }
    }

    async fn status(&self, spec: &DeploySpec) -> Result<Option<ContainerStatus>, DeployError> {
        let c = match self
            .docker()?
            .inspect_container(&spec.container(), None)
            .await
        {
            Ok(c) => c,
            Err(e) if not_found(&e) => return Ok(None),
            Err(e) => return Err(DeployError::Unavailable(format!("container engine: {e}"))),
        };
        let state = c.state.unwrap_or_default();
        let key = format!("{}/tcp", spec.server.port());
        let host_port = c
            .network_settings
            .and_then(|n| n.ports)
            .and_then(|p| p.get(&key).cloned().flatten())
            .and_then(|b| b.into_iter().find_map(|b| b.host_port?.parse().ok()));
        Ok(Some(ContainerStatus {
            state: state
                .status
                .map(|s| s.to_string())
                .unwrap_or_else(|| "unknown".into()),
            running: state.running.unwrap_or(false),
            host_port,
            started_at: state.started_at,
            exit_code: state.exit_code,
            error: state.error.filter(|e| !e.is_empty()),
            image: c.config.and_then(|c| c.image).unwrap_or_default(),
        }))
    }

    async fn logs(&self, spec: &DeploySpec, lines: usize) -> Result<String, DeployError> {
        logs_of(self.docker()?, &spec.container(), lines).await
    }

    async fn start_job(&self, job: &JobSpec) -> Result<(), DeployError> {
        let docker = self.docker()?;
        let name = job.container();
        self.remove_job(&job.name).await?;
        ensure_image(docker, &job.image).await?;
        let host = HostConfig {
            mounts: job.volume.as_ref().map(|(v, target)| {
                vec![Mount {
                    target: Some(target.clone()),
                    source: Some(v.clone()),
                    typ: Some(MountType::VOLUME),
                    ..Default::default()
                }]
            }),
            ipc_mode: Some("host".into()),
            device_requests: job.gpu.then(|| {
                vec![DeviceRequest {
                    count: Some(-1),
                    capabilities: Some(vec![vec!["gpu".to_owned()]]),
                    ..Default::default()
                }]
            }),
            ..Default::default()
        };
        let body = ContainerCreateBody {
            image: Some(job.image.clone()),
            cmd: Some(job.cmd.clone()),
            env: Some(job.env.clone()),
            labels: Some(HashMap::from([("gai.job".to_owned(), job.name.clone())])),
            host_config: Some(host),
            ..Default::default()
        };
        let opts = CreateContainerOptionsBuilder::new().name(&name).build();
        docker
            .create_container(Some(opts), body)
            .await
            .map_err(failed)?;
        if !job.files.is_empty() {
            let mut tar = tar::Builder::new(Vec::new());
            for (path, data) in &job.files {
                let mut h = tar::Header::new_gnu();
                h.set_mode(0o644);
                h.set_size(data.len() as u64);
                tar.append_data(&mut h, path.trim_start_matches('/'), data.as_slice())
                    .map_err(failed)?;
            }
            let archive = tar.into_inner().map_err(failed)?;
            docker
                .upload_to_container(
                    &name,
                    Some(UploadToContainerOptionsBuilder::new().path("/").build()),
                    bollard::body_full(archive.into()),
                )
                .await
                .map_err(|e| failed(format!("could not copy the job's files in: {e}")))?;
        }
        docker.start_container(&name, None).await.map_err(failed)?;
        Ok(())
    }

    async fn job_status(&self, name: &str) -> Result<Option<ContainerStatus>, DeployError> {
        let c = match self
            .docker()?
            .inspect_container(&format!("gai-job-{name}"), None)
            .await
        {
            Ok(c) => c,
            Err(e) if not_found(&e) => return Ok(None),
            Err(e) => return Err(DeployError::Unavailable(format!("container engine: {e}"))),
        };
        let state = c.state.unwrap_or_default();
        Ok(Some(ContainerStatus {
            state: state
                .status
                .map(|s| s.to_string())
                .unwrap_or_else(|| "unknown".into()),
            running: state.running.unwrap_or(false),
            host_port: None,
            started_at: state.started_at,
            exit_code: state.exit_code,
            error: state.error.filter(|e| !e.is_empty()),
            image: c.config.and_then(|c| c.image).unwrap_or_default(),
        }))
    }

    async fn job_logs(&self, name: &str, lines: usize) -> Result<String, DeployError> {
        logs_of(self.docker()?, &format!("gai-job-{name}"), lines).await
    }

    async fn remove_job(&self, name: &str) -> Result<(), DeployError> {
        let opts = RemoveContainerOptionsBuilder::new().force(true).build();
        match self
            .docker()?
            .remove_container(&format!("gai-job-{name}"), Some(opts))
            .await
        {
            Ok(()) => Ok(()),
            Err(e) if not_found(&e) => Ok(()),
            Err(e) => Err(failed(e)),
        }
    }

    async fn gpu_runtime(&self) -> bool {
        let Ok(docker) = self.docker() else {
            return false;
        };
        match docker.info().await {
            Ok(info) => {
                info.runtimes
                    .is_some_and(|r| r.keys().any(|k| k.contains("nvidia")))
                    || info.cdi_spec_dirs.is_some_and(|d| {
                        d.iter()
                            .any(|d| std::path::Path::new(d).join("nvidia.yaml").exists())
                    })
            }
            Err(_) => false,
        }
    }
}

/// One GPU, from `nvidia-smi`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Gpu {
    pub index: u32,
    pub name: String,
    pub memory_total_mb: u64,
    pub memory_used_mb: u64,
    /// Percent busy.
    pub utilization: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature_c: Option<u32>,
}

/// The NVIDIA GPUs on this machine; empty when there are none or
/// `nvidia-smi` isn't installed.
pub async fn gpus() -> Vec<Gpu> {
    let out = tokio::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=index,name,memory.total,memory.used,utilization.gpu,temperature.gpu",
            "--format=csv,noheader,nounits",
        ])
        .output();
    match tokio::time::timeout(Duration::from_secs(5), out).await {
        Ok(Ok(o)) if o.status.success() => parse_gpus(&String::from_utf8_lossy(&o.stdout)),
        _ => Vec::new(),
    }
}

fn parse_gpus(csv: &str) -> Vec<Gpu> {
    csv.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').map(str::trim).collect();
            if f.len() < 5 {
                return None;
            }
            Some(Gpu {
                index: f[0].parse().ok()?,
                name: f[1].to_owned(),
                memory_total_mb: f[2].parse().ok()?,
                memory_used_mb: f[3].parse().ok()?,
                utilization: f[4].parse().unwrap_or(0),
                temperature_c: f.get(5).and_then(|t| t.parse().ok()),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(yaml: &str) -> DeploySpec {
        let s: DeploySpec = parse(yaml);
        s.validate().unwrap();
        s
    }

    fn parse(yaml: &str) -> DeploySpec {
        // Specs arrive as JSON from the API; this keeps the tests readable.
        let v: serde_json::Value = yaml
            .lines()
            .filter_map(|l| l.split_once(": "))
            .map(|(k, v)| {
                let v = v.trim();
                let val = match v {
                    "true" => serde_json::Value::Bool(true),
                    "false" => serde_json::Value::Bool(false),
                    n if n.parse::<u64>().is_ok() => serde_json::json!(n.parse::<u64>().unwrap()),
                    s => serde_json::Value::String(s.into()),
                };
                (k.trim().to_owned(), val)
            })
            .collect::<serde_json::Map<_, _>>()
            .into();
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn vllm_command() {
        let s = spec(
            "name: qwen\nserver: vllm\nmodel: Qwen/Qwen3-8B\ngpu: true\ncontext: 32768\ntools: true",
        );
        assert_eq!(
            s.command().join(" "),
            "--model Qwen/Qwen3-8B --served-model-name Qwen/Qwen3-8B --host 0.0.0.0 --port 8000 --max-model-len 32768 --enable-auto-tool-choice --tool-call-parser hermes"
        );
        assert_eq!(s.image(), "docker.io/vllm/vllm-openai:latest");
        assert!(s.tool_calls());
        assert_eq!(s.container(), "gai-model-qwen");
    }

    #[test]
    fn llamacpp_command() {
        let s = spec(
            "name: gemma\nserver: llamacpp\nmodel: ggml-org/gemma-3-1b-it-GGUF\ncontext: 8192",
        );
        assert_eq!(
            s.command().join(" "),
            "-hf ggml-org/gemma-3-1b-it-GGUF --host 0.0.0.0 --port 8080 -c 8192 --jinja"
        );
        assert_eq!(s.image(), "ghcr.io/ggml-org/llama.cpp:server");
        let g = spec("name: g\nserver: llamacpp\nmodel: https://example.com/m.gguf\ngpu: true");
        assert!(g.command().join(" ").starts_with(
            "-m /root/.cache/llama.cpp/m.gguf --model-url https://example.com/m.gguf"
        ));
        assert!(g.command().contains(&"-ngl".to_owned()));
        assert_eq!(g.image(), "ghcr.io/ggml-org/llama.cpp:server-cuda");
    }

    #[test]
    fn ollama_and_checks() {
        let s = spec("name: local\nserver: ollama\ncontext: 16384");
        assert!(s.command().is_empty());
        let env = s.resolved_env().unwrap();
        assert!(env.contains(&"OLLAMA_CONTEXT_LENGTH=16384".to_owned()));
        assert_eq!(s.volume(), "gai-models-ollama");
        let bad = parse("name: q\nserver: vllm\nmodel: x");
        assert!(bad.validate().unwrap_err().contains("GPU"));
        let bad = parse("name: Q Q\nserver: ollama");
        assert!(bad.validate().is_err());
        let bad = parse("name: q\nserver: llamacpp");
        assert!(bad.validate().is_err());
    }

    #[test]
    fn env_from_gais_environment() {
        let mut s = spec("name: q\nserver: llamacpp\nmodel: a/b");
        s.env
            .insert("HF_TOKEN".into(), "${GAI_TEST_DEPLOY_TOKEN_UNSET}".into());
        assert!(
            s.resolved_env()
                .unwrap_err()
                .contains("GAI_TEST_DEPLOY_TOKEN_UNSET")
        );
        s.env.insert("HF_TOKEN".into(), "${PATH}".into());
        assert!(s.resolved_env().unwrap()[0].starts_with("HF_TOKEN=/"));
    }

    /// Starts llama.cpp's server with a tiny model on the real engine
    /// (`scripts/sandbox-test.sh serves_a_tiny_model`). It reaches the
    /// published port through the engine's host, as gai in a container does.
    #[tokio::test]
    #[ignore = "needs a container engine and the network"]
    async fn serves_a_tiny_model() {
        let host = std::env::var("GAI_DEPLOY_TEST_HOST")
            .unwrap_or_else(|_| "host.containers.internal".into());
        let s = spec(
            "name: test-tiny\nserver: llamacpp\nmodel: https://huggingface.co/ggml-org/models/resolve/main/tinyllamas/stories260K.gguf\ncontext: 256\ntools: false",
        );
        let engine = DockerEngine::new(None);
        engine.remove(&s).await.unwrap();
        engine.start(&s, "0.0.0.0").await.unwrap();
        // Starting again with the same spec keeps the container.
        engine.start(&s, "0.0.0.0").await.unwrap();
        let status = engine.status(&s).await.unwrap().expect("a container");
        assert!(status.running, "{status:?}");
        let port = status.host_port.expect("a published port");
        let url = format!("{host}:{port}");
        let mut served = String::new();
        for _ in 0..90 {
            if let Some((200, body)) = http_get(&url, "/v1/models").await {
                served = body;
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        let logs = engine.logs(&s, 40).await.unwrap();
        assert!(served.contains("stories260K"), "{served}\n{logs}");
        engine.stop(&s).await.unwrap();
        assert!(!engine.status(&s).await.unwrap().unwrap().running);
        engine.remove(&s).await.unwrap();
        assert!(engine.status(&s).await.unwrap().is_none());
    }

    /// A plain HTTP/1.1 GET: the status code and body.
    async fn http_get(hostport: &str, path: &str) -> Option<(u16, String)> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut s = tokio::net::TcpStream::connect(hostport).await.ok()?;
        let req = format!("GET {path} HTTP/1.1\r\nHost: {hostport}\r\nConnection: close\r\n\r\n");
        s.write_all(req.as_bytes()).await.ok()?;
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await.ok()?;
        let text = String::from_utf8_lossy(&buf).into_owned();
        let code = text.split(' ').nth(1)?.parse().ok()?;
        let (_, body) = text.split_once("\r\n\r\n")?;
        Some((code, body.to_owned()))
    }

    #[test]
    fn nvidia_smi_output() {
        let g = parse_gpus(
            "0, NVIDIA GeForce RTX 4090, 24564, 1234, 7, 41\n1, NVIDIA A100-SXM4-80GB, 81920, 0, 0, [N/A]\n",
        );
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].name, "NVIDIA GeForce RTX 4090");
        assert_eq!(g[0].memory_used_mb, 1234);
        assert_eq!(g[0].temperature_c, Some(41));
        assert_eq!(g[1].temperature_c, None);
    }
}
