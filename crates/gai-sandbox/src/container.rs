//! A fresh Docker/Podman container per call.
//!
//! The container has no network (unless `network = true`), capped memory,
//! CPU and process count, no capabilities, and runs as uid 1000. The
//! workspace is copied in as a tar archive and copied back out when the code
//! finishes; nothing on the host is mounted, so this also works when gai
//! runs in a container with only the engine's socket passed through.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use bollard::Docker;
use bollard::errors::Error as DockerError;
use bollard::models::{ContainerCreateBody, HostConfig};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, DownloadFromContainerOptionsBuilder,
    KillContainerOptionsBuilder, LogsOptionsBuilder, RemoveContainerOptionsBuilder,
    UploadToContainerOptionsBuilder,
};
use futures::StreamExt;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::local::lossy;
use crate::workspace::{Snapshot, hash, safe_join};
use crate::{ExecRequest, ExecResult, Language, Sandbox, SandboxError, SandboxSettings};

const UID: u64 = 1000;
const WORKDIR: &str = "workspace";
const SCRIPT_DIR: &str = "gai";

pub struct ContainerSandbox {
    settings: SandboxSettings,
    docker: Result<Docker, String>,
    /// Images known to be present.
    ready: Mutex<HashSet<String>>,
}

/// The engine address to use: configured, `DOCKER_HOST`, or the first
/// socket that exists.
fn engine_address(settings: &SandboxSettings) -> Option<String> {
    if let Some(e) = &settings.engine {
        return Some(e.clone());
    }
    if let Ok(h) = std::env::var("DOCKER_HOST") {
        return Some(h);
    }
    if cfg!(windows) {
        return Some("npipe:////./pipe/docker_engine".into());
    }
    let mut candidates = vec![
        "/var/run/docker.sock".to_owned(),
        "/run/podman/podman.sock".to_owned(),
    ];
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        candidates.push(format!("{dir}/podman/podman.sock"));
        candidates.push(format!("{dir}/docker.sock"));
    }
    candidates
        .into_iter()
        .find(|p| Path::new(p).exists())
        .map(|p| format!("unix://{p}"))
}

fn failed(e: impl std::fmt::Display) -> SandboxError {
    SandboxError::Failed(e.to_string())
}

impl ContainerSandbox {
    /// Never fails: without a reachable engine, calls explain what is
    /// missing instead.
    pub fn new(settings: SandboxSettings) -> Result<Self, SandboxError> {
        let docker = match engine_address(&settings) {
            None => Err("no container engine found: start Docker or Podman, set DOCKER_HOST, or set sandbox.engine (or sandbox.backend = \"local\" to run code without isolation)".to_owned()),
            Some(addr) => Docker::connect_with_host(&addr)
                .map(|d| {
                    // Long enough for the slowest call, and for image pulls.
                    let secs = settings.max_timeout_secs.max(600) + 60;
                    d.with_timeout(Duration::from_secs(secs))
                })
                .map_err(|e| format!("container engine at {addr}: {e}")),
        };
        Ok(Self {
            settings,
            docker,
            ready: Mutex::new(HashSet::new()),
        })
    }

    fn docker(&self) -> Result<&Docker, SandboxError> {
        self.docker
            .as_ref()
            .map_err(|e| SandboxError::Unavailable(e.clone()))
    }

    fn image(&self, language: Language) -> &str {
        match language {
            Language::Python => &self.settings.images.python,
            Language::Node => &self.settings.images.node,
            Language::Bash => &self.settings.images.bash,
        }
    }

    /// Pull `image` unless the engine already has it.
    async fn ensure_image(&self, docker: &Docker, image: &str) -> Result<(), SandboxError> {
        let mut ready = self.ready.lock().await;
        if ready.contains(image) {
            return Ok(());
        }
        match docker.inspect_image(image).await {
            Ok(_) => {}
            Err(DockerError::DockerResponseServerError {
                status_code: 404, ..
            }) => {
                tracing::info!(image, "pulling sandbox image");
                let opts = CreateImageOptionsBuilder::new().from_image(image).build();
                let mut pull = docker.create_image(Some(opts), None, None);
                while let Some(step) = pull.next().await {
                    step.map_err(|e| {
                        SandboxError::Unavailable(format!("could not pull {image}: {e}"))
                    })?;
                }
            }
            Err(e) => {
                return Err(SandboxError::Unavailable(format!("container engine: {e}")));
            }
        }
        ready.insert(image.to_owned());
        Ok(())
    }
}

/// A tar of the workspace files in `snap` (under `workspace/`) and the
/// script (under `gai/`), owned by the sandbox user.
fn pack(root: &Path, snap: &Snapshot, language: Language, code: &str) -> std::io::Result<Vec<u8>> {
    let mut tar = tar::Builder::new(Vec::new());
    let mut dirs = BTreeSet::from([WORKDIR.to_owned(), SCRIPT_DIR.to_owned()]);
    for rel in snap.files.keys() {
        let mut parts: Vec<&str> = rel.split('/').collect();
        parts.pop();
        let mut d = WORKDIR.to_owned();
        for p in parts {
            d = format!("{d}/{p}");
            dirs.insert(d.clone());
        }
    }
    // BTreeSet order puts parents before children.
    for d in &dirs {
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Directory);
        h.set_mode(0o755);
        h.set_uid(UID);
        h.set_gid(UID);
        h.set_size(0);
        tar.append_data(&mut h, format!("{d}/"), std::io::empty())?;
    }
    let mut add = |path: String, data: &[u8]| {
        let mut h = tar::Header::new_gnu();
        h.set_mode(0o644);
        h.set_uid(UID);
        h.set_gid(UID);
        h.set_size(data.len() as u64);
        tar.append_data(&mut h, path, data)
    };
    for rel in snap.files.keys() {
        let data = std::fs::read(root.join(rel))?;
        add(format!("{WORKDIR}/{rel}"), &data)?;
    }
    add(
        format!("{SCRIPT_DIR}/{}", language.script_name()),
        code.as_bytes(),
    )?;
    tar.into_inner()
}

/// Write files from the container's workspace tar that differ from `before`
/// into `root`, and delete files of `before` it no longer has.
fn unpack(
    root: &Path,
    before: &Snapshot,
    archive: &[u8],
) -> std::io::Result<(Vec<String>, Vec<String>)> {
    let mut changed = Vec::new();
    let mut seen = HashSet::new();
    let mut ar = tar::Archive::new(archive);
    for entry in ar.entries()? {
        let mut entry = entry?;
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }
        let path = entry.path()?.to_string_lossy().replace('\\', "/");
        let Some(rel) = path.strip_prefix(&format!("{WORKDIR}/")) else {
            continue;
        };
        let rel = rel.to_owned();
        let Some(dest) = safe_join(root, &rel) else {
            continue;
        };
        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;
        seen.insert(rel.clone());
        if before
            .files
            .get(&rel)
            .is_some_and(|f| f.hash == hash(&data))
        {
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, &data)?;
        changed.push(rel);
    }
    let mut deleted = Vec::new();
    for rel in before.files.keys() {
        if !seen.contains(rel) {
            if let Some(p) = safe_join(root, rel) {
                let _ = std::fs::remove_file(p);
            }
            deleted.push(rel.clone());
        }
    }
    changed.sort();
    Ok((changed, deleted))
}

/// Removes the container however the call ends.
struct Remove {
    docker: Docker,
    id: String,
}

impl Remove {
    async fn now(mut self) {
        let id = std::mem::take(&mut self.id);
        let opts = RemoveContainerOptionsBuilder::new().force(true).build();
        if let Err(e) = self.docker.remove_container(&id, Some(opts)).await {
            tracing::warn!(container = %id, error = %e, "could not remove sandbox container");
        }
    }
}

impl Drop for Remove {
    fn drop(&mut self) {
        let (docker, id) = (self.docker.clone(), std::mem::take(&mut self.id));
        if id.is_empty() {
            return;
        }
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                let opts = RemoveContainerOptionsBuilder::new().force(true).build();
                if let Err(e) = docker.remove_container(&id, Some(opts)).await {
                    tracing::warn!(container = %id, error = %e, "could not remove sandbox container");
                }
            });
        }
    }
}

#[async_trait]
impl Sandbox for ContainerSandbox {
    fn kind(&self) -> &'static str {
        "container"
    }

    fn isolated(&self) -> bool {
        true
    }

    async fn exec(
        &self,
        req: ExecRequest,
        cancel: CancellationToken,
    ) -> Result<ExecResult, SandboxError> {
        let docker = self.docker()?;
        let image = self.image(req.language).to_owned();
        tokio::select! {
            r = self.ensure_image(docker, &image) => r?,
            _ = cancel.cancelled() => return Err(SandboxError::Cancelled),
        }

        tokio::fs::create_dir_all(&req.workspace)
            .await
            .map_err(failed)?;
        let budget = self.settings.max_workspace_mb << 20;
        let (root, language, code) = (req.workspace.clone(), req.language, req.code.clone());
        let (before, archive) = tokio::task::spawn_blocking(move || {
            let snap = Snapshot::take(&root, budget)?;
            let tar = pack(&root, &snap, language, &code)?;
            Ok::<_, std::io::Error>((snap, tar))
        })
        .await
        .map_err(failed)?
        .map_err(|e| failed(format!("reading the workspace: {e}")))?;

        let s = &self.settings;
        let host = HostConfig {
            memory: Some((s.memory_mb << 20) as i64),
            memory_swap: Some((s.memory_mb << 20) as i64),
            nano_cpus: Some((s.cpus * 1e9) as i64),
            pids_limit: Some(256),
            network_mode: Some(if s.network { "bridge" } else { "none" }.into()),
            cap_drop: Some(vec!["ALL".into()]),
            security_opt: Some(vec!["no-new-privileges".into()]),
            tmpfs: Some(HashMap::from([(
                "/tmp".to_owned(),
                "rw,exec,size=256m".to_owned(),
            )])),
            init: Some(true),
            ..Default::default()
        };
        let body = ContainerCreateBody {
            image: Some(image.clone()),
            cmd: Some(vec![
                req.language.interpreter().into(),
                format!("/{SCRIPT_DIR}/{}", req.language.script_name()),
            ]),
            entrypoint: Some(Vec::new()),
            working_dir: Some(format!("/{WORKDIR}")),
            user: Some(format!("{UID}:{UID}")),
            env: Some(vec![
                "HOME=/tmp".into(),
                "PYTHONDONTWRITEBYTECODE=1".into(),
                "PYTHONUNBUFFERED=1".into(),
                "MPLBACKEND=Agg".into(),
                "MPLCONFIGDIR=/tmp".into(),
            ]),
            network_disabled: Some(!s.network),
            labels: Some(HashMap::from([
                ("gai.sandbox".to_owned(), "1".to_owned()),
                ("gai.run".to_owned(), req.run_id.clone()),
            ])),
            host_config: Some(host),
            ..Default::default()
        };
        let name = gai_core::new_id("gai-sandbox");
        let created = docker
            .create_container(
                Some(CreateContainerOptionsBuilder::new().name(&name).build()),
                body,
            )
            .await
            .map_err(|e| failed(format!("could not create the sandbox container: {e}")))?;
        let remove = Remove {
            docker: docker.clone(),
            id: created.id.clone(),
        };
        let result = self
            .run(docker, &created.id, req, before, archive, cancel)
            .await;
        remove.now().await;
        result
    }
}

impl ContainerSandbox {
    /// Copy in, start, wait, collect output and copy back.
    async fn run(
        &self,
        docker: &Docker,
        id: &str,
        req: ExecRequest,
        before: Snapshot,
        archive: Vec<u8>,
        cancel: CancellationToken,
    ) -> Result<ExecResult, SandboxError> {
        let budget = self.settings.max_workspace_mb << 20;

        docker
            .upload_to_container(
                id,
                Some(UploadToContainerOptionsBuilder::new().path("/").build()),
                bollard::body_full(archive.into()),
            )
            .await
            .map_err(|e| failed(format!("could not copy the workspace in: {e}")))?;
        docker
            .start_container(id, None)
            .await
            .map_err(|e| failed(format!("could not start the sandbox: {e}")))?;

        let mut wait = docker.wait_container(id, None);
        let finished = tokio::select! {
            r = wait.next() => Some(match r {
                Some(Ok(w)) => w.status_code,
                // bollard reports a non-zero exit as an error.
                Some(Err(DockerError::DockerContainerWaitError { code, .. })) => code,
                Some(Err(e)) => return Err(failed(format!("waiting for the sandbox: {e}"))),
                None => return Err(failed("the sandbox stopped answering")),
            }),
            _ = tokio::time::sleep(req.timeout) => None,
            _ = cancel.cancelled() => return Err(SandboxError::Cancelled),
        };
        if finished.is_none() {
            let _ = docker
                .kill_container(
                    id,
                    Some(KillContainerOptionsBuilder::new().signal("KILL").build()),
                )
                .await;
        }

        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut logs = docker.logs(
            id,
            Some(LogsOptionsBuilder::new().stdout(true).stderr(true).build()),
        );
        while let Some(chunk) = logs.next().await {
            match chunk {
                Ok(bollard::container::LogOutput::StdOut { message }) => {
                    if out.len() < crate::local::MAX_STREAM_BYTES {
                        out.extend_from_slice(&message);
                    }
                }
                Ok(bollard::container::LogOutput::StdErr { message }) => {
                    if err.len() < crate::local::MAX_STREAM_BYTES {
                        err.extend_from_slice(&message);
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "could not read sandbox output");
                    break;
                }
            }
        }

        // Copy the workspace back, bounded so a runaway program cannot fill
        // the host's disk.
        let limit = (budget as usize).saturating_mul(2).max(64 << 20);
        let mut tar = Vec::new();
        let mut stream = docker.download_from_container(
            id,
            Some(
                DownloadFromContainerOptionsBuilder::new()
                    .path(&format!("/{WORKDIR}"))
                    .build(),
            ),
        );
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|e| failed(format!("could not copy the workspace out: {e}")))?;
            if tar.len() + chunk.len() > limit {
                return Err(failed(format!(
                    "the program left more than {} MB in the workspace; nothing was copied back",
                    limit >> 20
                )));
            }
            tar.extend_from_slice(&chunk);
        }
        let root = req.workspace.clone();
        let skipped = before.skipped.clone();
        let (changed, deleted) = tokio::task::spawn_blocking(move || unpack(&root, &before, &tar))
            .await
            .map_err(failed)?
            .map_err(|e| failed(format!("could not update the workspace: {e}")))?;

        Ok(ExecResult {
            exit_code: finished,
            stdout: lossy(out),
            stderr: lossy(err),
            timed_out: finished.is_none(),
            changed,
            deleted,
            skipped,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_and_unpack_round_trip_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/in.csv"), "a,b").unwrap();
        std::fs::write(root.join("old.txt"), "bye").unwrap();
        let snap = Snapshot::take(root, u64::MAX).unwrap();
        let packed = pack(root, &snap, Language::Python, "print(1)").unwrap();
        let names: Vec<String> = tar::Archive::new(packed.as_slice())
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"workspace/data/".to_owned()), "{names:?}");
        assert!(names.contains(&"workspace/data/in.csv".to_owned()));
        assert!(names.contains(&"gai/main.py".to_owned()));

        // What the container hands back: in.csv unchanged, old.txt gone, a
        // new chart, and a path trying to escape.
        let mut back = tar::Builder::new(Vec::new());
        let mut put = |p: &str, d: &[u8]| {
            let mut h = tar::Header::new_gnu();
            h.set_mode(0o644);
            h.set_size(d.len() as u64);
            back.append_data(&mut h, p, d).unwrap();
        };
        put("workspace/data/in.csv", b"a,b");
        put("workspace/out/chart.png", b"PNG");
        let mut h = tar::Header::new_gnu();
        h.set_mode(0o644);
        h.set_size(1);
        // append_data refuses `..`, so write the name into the header directly.
        h.as_gnu_mut().unwrap().name[..22].copy_from_slice(b"workspace/../../escape");
        h.set_cksum();
        back.append(&h, &b"x"[..]).unwrap();
        let back = back.into_inner().unwrap();

        let (changed, deleted) = unpack(root, &snap, &back).unwrap();
        assert_eq!(changed, ["out/chart.png"]);
        assert_eq!(deleted, ["old.txt"]);
        assert_eq!(std::fs::read(root.join("out/chart.png")).unwrap(), b"PNG");
        assert!(!root.join("old.txt").exists());
        assert!(!root.parent().unwrap().join("escape").exists());
    }
}
