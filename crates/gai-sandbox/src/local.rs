//! Runs code as the gai process, in the workspace. No isolation at all:
//! for development, or machines without a container engine.

use std::process::Stdio;

use async_trait::async_trait;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::workspace::Snapshot;
use crate::{ExecRequest, ExecResult, Sandbox, SandboxError, SandboxSettings};

/// Most output kept per stream.
pub(crate) const MAX_STREAM_BYTES: usize = 1 << 20;

pub struct LocalSandbox {
    _settings: SandboxSettings,
}

impl LocalSandbox {
    pub fn new(settings: SandboxSettings) -> Self {
        Self {
            _settings: settings,
        }
    }
}

pub(crate) fn lossy(mut bytes: Vec<u8>) -> String {
    bytes.truncate(MAX_STREAM_BYTES);
    String::from_utf8_lossy(&bytes).into_owned()
}

fn failed(e: impl std::fmt::Display) -> SandboxError {
    SandboxError::Failed(e.to_string())
}

#[async_trait]
impl Sandbox for LocalSandbox {
    fn kind(&self) -> &'static str {
        "local"
    }

    fn isolated(&self) -> bool {
        false
    }

    async fn exec(
        &self,
        req: ExecRequest,
        cancel: CancellationToken,
    ) -> Result<ExecResult, SandboxError> {
        tokio::fs::create_dir_all(&req.workspace)
            .await
            .map_err(failed)?;
        let root = req.workspace.clone();
        let before = tokio::task::spawn_blocking(move || Snapshot::take(&root, u64::MAX))
            .await
            .map_err(failed)?
            .map_err(failed)?;

        // The script lives outside the workspace so it is not reported as a
        // file the code wrote.
        let script = std::env::temp_dir().join(format!(
            "{}-{}",
            gai_core::new_id("gai-sandbox"),
            req.language.script_name()
        ));
        tokio::fs::write(&script, &req.code).await.map_err(failed)?;
        let mut child = tokio::process::Command::new(req.language.interpreter())
            .arg(&script)
            .current_dir(&req.workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                SandboxError::Unavailable(format!(
                    "could not start {}: {e}",
                    req.language.interpreter()
                ))
            })?;
        let mut stdout = child.stdout.take().expect("piped");
        let mut stderr = child.stderr.take().expect("piped");
        let mut out = Vec::new();
        let mut err = Vec::new();
        let finished = tokio::select! {
            r = async {
                let _ = tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err));
                child.wait().await
            } => Some(r.map_err(failed)?),
            _ = tokio::time::sleep(req.timeout) => None,
            _ = cancel.cancelled() => {
                let _ = tokio::fs::remove_file(&script).await;
                return Err(SandboxError::Cancelled);
            }
        };
        let _ = tokio::fs::remove_file(&script).await;
        let timed_out = finished.is_none();
        if timed_out {
            let _ = child.kill().await;
        }

        let root = req.workspace.clone();
        let after = tokio::task::spawn_blocking(move || Snapshot::take(&root, u64::MAX))
            .await
            .map_err(failed)?
            .map_err(failed)?;
        let (changed, deleted) = before.diff(&after);
        Ok(ExecResult {
            exit_code: finished.and_then(|s| s.code()).map(i64::from),
            stdout: lossy(out),
            stderr: lossy(err),
            timed_out,
            changed,
            deleted,
            skipped: Vec::new(),
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::Language;

    fn req(dir: &std::path::Path, code: &str, timeout: u64) -> ExecRequest {
        ExecRequest {
            language: Language::Bash,
            code: code.into(),
            workspace: dir.to_path_buf(),
            timeout: Duration::from_secs(timeout),
            run_id: "run_test".into(),
        }
    }

    #[tokio::test]
    async fn runs_bash_and_reports_written_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("in.txt"), "hello").unwrap();
        let sb = LocalSandbox::new(SandboxSettings::default());
        let r = sb
            .exec(
                req(
                    dir.path(),
                    "cat in.txt; echo warn >&2; mkdir -p out; echo hi > out/x.txt; exit 4",
                    10,
                ),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(r.exit_code, Some(4));
        assert_eq!(r.stdout, "hello");
        assert_eq!(r.stderr.trim(), "warn");
        assert_eq!(r.changed, ["out/x.txt"]);
        assert!(!r.timed_out);
    }

    #[tokio::test]
    async fn times_out() {
        let dir = tempfile::tempdir().unwrap();
        let sb = LocalSandbox::new(SandboxSettings::default());
        let r = sb
            .exec(
                req(dir.path(), "echo started; sleep 30", 1),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(r.timed_out);
        assert_eq!(r.exit_code, None);
    }
}
