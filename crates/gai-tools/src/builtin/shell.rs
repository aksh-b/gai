use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use gai_core::ToolSpec;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use crate::{Tool, ToolCtx, ToolError, ToolOutput, ToolPolicy};

/// Run a shell command in the workspace. Needs approval by default: it can
/// do anything the gai process can. Use the code sandbox for untrusted work.
pub struct Shell;

#[derive(Deserialize)]
struct Args {
    command: String,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

#[async_trait]
impl Tool for Shell {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "shell".into(),
            description: "Run a shell command (sh -c) in the workspace directory and return its exit code, stdout and stderr.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "timeout_secs": {"type": "integer", "minimum": 1, "description": "Defaults to the configured shell timeout"}
                },
                "required": ["command"]
            }),
            strict: false,
        }
    }

    fn default_policy(&self) -> ToolPolicy {
        ToolPolicy::Approve
    }

    async fn call(&self, ctx: &ToolCtx, input: Value) -> Result<ToolOutput, ToolError> {
        let a: Args =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        tokio::fs::create_dir_all(&ctx.workspace)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;
        let (shell, flag) = if cfg!(windows) {
            ("cmd", "/C")
        } else {
            ("sh", "-c")
        };
        let mut child = tokio::process::Command::new(shell)
            .arg(flag)
            .arg(&a.command)
            .current_dir(&ctx.workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| ToolError::Failed(format!("could not start shell: {e}")))?;
        let mut stdout = child.stdout.take().expect("piped");
        let mut stderr = child.stderr.take().expect("piped");
        let read_out = async {
            let mut o = Vec::new();
            let mut e = Vec::new();
            let _ = tokio::join!(stdout.read_to_end(&mut o), stderr.read_to_end(&mut e));
            (o, e)
        };
        let timeout = a.timeout_secs.unwrap_or(ctx.settings.shell_timeout_secs);
        let ((out, err), status) = tokio::select! {
            r = async { let io = read_out.await; (io, child.wait().await) } => r,
            _ = tokio::time::sleep(Duration::from_secs(timeout)) => return Err(ToolError::Timeout(timeout)),
            _ = ctx.cancel.cancelled() => return Err(ToolError::Cancelled),
        };
        let code = status.map_err(|e| ToolError::Failed(e.to_string()))?.code();
        let mut text = format!(
            "exit code: {}\n",
            code.map_or("killed".to_owned(), |c| c.to_string())
        );
        let out = String::from_utf8_lossy(&out);
        let err = String::from_utf8_lossy(&err);
        if !out.is_empty() {
            text.push_str(&format!("stdout:\n{out}\n"));
        }
        if !err.is_empty() {
            text.push_str(&format!("stderr:\n{err}\n"));
        }
        Ok(ToolOutput {
            is_error: code != Some(0),
            ..ToolOutput::text(text.trim_end())
        }
        .truncated(ctx.settings.max_output_chars))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_in_the_workspace_and_times_out() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "x").unwrap();
        let ctx = ToolCtx::for_workspace(dir.path());
        let out = Shell
            .call(&ctx, json!({"command": "ls; echo oops >&2; exit 3"}))
            .await
            .unwrap();
        assert!(out.is_error);
        assert!(out.as_text().contains("exit code: 3"));
        assert!(out.as_text().contains("f.txt"));
        assert!(out.as_text().contains("oops"));
        let t = Shell
            .call(&ctx, json!({"command": "sleep 5", "timeout_secs": 1}))
            .await;
        assert!(matches!(t, Err(ToolError::Timeout(1))));
    }
}
