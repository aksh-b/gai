//! Run code that agents write, away from the gai process.
//!
//! A [`Sandbox`] runs a snippet of Python, JavaScript or bash against a
//! session's workspace and reports what it printed and which files it
//! changed. The [`container`] backend starts a fresh container per call
//! (Docker or Podman, no network, CPU/memory/time limits) and copies the
//! workspace in and out, so it works even when gai itself runs in a
//! container. The [`local`] backend runs the interpreter directly and is for
//! development only.
//!
//! [`code_exec`] wraps a sandbox as the `code_exec` tool; files the code
//! writes are handed to an [`ArtifactSink`] (the blob store, in gai).

#[cfg(feature = "container")]
pub mod container;
#[cfg(feature = "container")]
pub mod deploy;
pub mod local;
mod tool;
mod workspace;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

pub use tool::{ArtifactSink, CodeExec, code_exec};
pub use workspace::Snapshot;

/// ```toml
/// [sandbox]
/// backend = "container"        # or "local" (no isolation, dev only) or "off"
/// # engine = "unix:///run/podman/podman.sock"   # default: DOCKER_HOST, then the usual sockets
/// memory_mb = 512
/// cpus = 1.0
/// timeout_secs = 60
/// network = false
/// [sandbox.images]
/// python = "docker.io/library/python:3.13-slim"
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct SandboxSettings {
    pub backend: Backend,
    /// Docker-compatible API address (`unix:///…`, `tcp://…`, `npipe://…`).
    /// Defaults to `DOCKER_HOST`, then `/var/run/docker.sock`, then the
    /// Podman sockets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    pub images: Images,
    pub memory_mb: u64,
    pub cpus: f64,
    /// Default time limit for one call; a call may ask for up to
    /// `max_timeout_secs`.
    pub timeout_secs: u64,
    pub max_timeout_secs: u64,
    /// Give containers network access (off by default).
    pub network: bool,
    /// Largest workspace copied into a container, in megabytes. Bigger files
    /// are left out (and said so).
    pub max_workspace_mb: u64,
}

impl Default for SandboxSettings {
    fn default() -> Self {
        Self {
            backend: Backend::Container,
            engine: None,
            images: Images::default(),
            memory_mb: 512,
            cpus: 1.0,
            timeout_secs: 60,
            max_timeout_secs: 600,
            network: false,
            max_workspace_mb: 100,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// A fresh Docker/Podman container per call.
    #[default]
    Container,
    /// The interpreter runs as the gai process, in the workspace. No
    /// isolation: calls need approval by default.
    Local,
    /// No `code_exec` tool.
    Off,
}

/// Container images per language.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Images {
    pub python: String,
    pub node: String,
    pub bash: String,
}

impl Default for Images {
    fn default() -> Self {
        let python = "docker.io/library/python:3.13-slim".to_owned();
        Self {
            node: "docker.io/library/node:24-slim".into(),
            // The Python image has bash, so scripts can call python too.
            bash: python.clone(),
            python,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    Python,
    #[serde(alias = "javascript", alias = "js")]
    Node,
    #[serde(alias = "sh", alias = "shell")]
    Bash,
}

impl Language {
    pub fn as_str(self) -> &'static str {
        match self {
            Language::Python => "python",
            Language::Node => "node",
            Language::Bash => "bash",
        }
    }

    /// File name the code is saved under before it runs.
    pub fn script_name(self) -> &'static str {
        match self {
            Language::Python => "main.py",
            Language::Node => "main.js",
            Language::Bash => "main.sh",
        }
    }

    /// The interpreter, as found on `PATH` (in the image or on the host).
    pub fn interpreter(self) -> &'static str {
        match self {
            Language::Python => "python3",
            Language::Node => "node",
            Language::Bash => "bash",
        }
    }
}

/// One snippet to run.
#[derive(Clone, Debug)]
pub struct ExecRequest {
    pub language: Language,
    pub code: String,
    /// The session workspace: copied in, and changes copied back.
    pub workspace: PathBuf,
    pub timeout: Duration,
    /// For container labels and logs.
    pub run_id: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExecResult {
    /// `None` when the process was killed (timeout, out of memory).
    pub exit_code: Option<i64>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    /// Workspace files the code created or changed, relative to the
    /// workspace, `/`-separated.
    pub changed: Vec<String>,
    /// Workspace files the code deleted.
    pub deleted: Vec<String>,
    /// Workspace files not copied into the container (too large).
    pub skipped: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Failed(String),
    #[error("cancelled")]
    Cancelled,
}

#[async_trait]
pub trait Sandbox: Send + Sync {
    /// `container` or `local`.
    fn kind(&self) -> &'static str;

    /// Whether code is kept away from the gai process and its network.
    fn isolated(&self) -> bool;

    async fn exec(
        &self,
        req: ExecRequest,
        cancel: CancellationToken,
    ) -> Result<ExecResult, SandboxError>;
}

pub type SharedSandbox = Arc<dyn Sandbox>;

/// The sandbox `settings` ask for, or `None` when it is off. Does not
/// contact the container engine yet; the first call does.
pub fn from_settings(settings: &SandboxSettings) -> Result<Option<SharedSandbox>, SandboxError> {
    match settings.backend {
        Backend::Off => Ok(None),
        Backend::Local => Ok(Some(Arc::new(local::LocalSandbox::new(settings.clone())))),
        #[cfg(feature = "container")]
        Backend::Container => Ok(Some(Arc::new(container::ContainerSandbox::new(
            settings.clone(),
        )?))),
        #[cfg(not(feature = "container"))]
        Backend::Container => Err(SandboxError::Unavailable(
            "this build of gai has no container sandbox (feature `sandbox`); set sandbox.backend = \"local\" or \"off\"".into(),
        )),
    }
}
