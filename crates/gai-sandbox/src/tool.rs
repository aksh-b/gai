//! The `code_exec` tool.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use gai_core::{ContentBlock, MediaSource, ToolSpec};
use gai_tools::{Tool, ToolCtx, ToolError, ToolOutput, ToolPolicy};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::workspace::safe_join;
use crate::{ExecRequest, Language, SandboxError, SandboxSettings, SharedSandbox};

/// Most files from one call kept as artifacts, and the largest one.
const MAX_ARTIFACTS: usize = 10;
const MAX_ARTIFACT_BYTES: u64 = 10 << 20;

/// Where files written by code go, so the user can see and download them.
#[async_trait]
pub trait ArtifactSink: Send + Sync {
    /// Keep `data` (the workspace file `path`) and return its id.
    async fn save(
        &self,
        ctx: &ToolCtx,
        path: &str,
        media_type: &str,
        data: Vec<u8>,
    ) -> Result<String, String>;
}

pub struct CodeExec {
    sandbox: SharedSandbox,
    settings: SandboxSettings,
    artifacts: Option<Arc<dyn ArtifactSink>>,
}

/// `code_exec` over `sandbox`. Written files go to `artifacts` when given.
pub fn code_exec(
    sandbox: SharedSandbox,
    settings: SandboxSettings,
    artifacts: Option<Arc<dyn ArtifactSink>>,
) -> CodeExec {
    CodeExec {
        sandbox,
        settings,
        artifacts,
    }
}

#[derive(Deserialize)]
struct Args {
    #[serde(default)]
    language: Language,
    code: String,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

impl CodeExec {
    /// Hand a written file to the artifact sink: its id and media type.
    async fn keep(&self, ctx: &ToolCtx, path: &str) -> Option<(String, String)> {
        let sink = self.artifacts.as_ref()?;
        let data = tokio::fs::read(safe_join(&ctx.workspace, path)?)
            .await
            .ok()?;
        if data.len() as u64 > MAX_ARTIFACT_BYTES {
            return None;
        }
        let media_type = mime_guess::from_path(path)
            .first_or_octet_stream()
            .essence_str()
            .to_owned();
        match sink.save(ctx, path, &media_type, data).await {
            Ok(id) => Some((id, media_type)),
            Err(e) => {
                tracing::warn!(path, error = %e, "could not keep a file code_exec wrote");
                None
            }
        }
    }
}

/// Images a model can be shown.
fn viewable(media_type: &str) -> bool {
    matches!(
        media_type,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    )
}

#[async_trait]
impl Tool for CodeExec {
    fn spec(&self) -> ToolSpec {
        let isolation = if self.sandbox.isolated() {
            format!(
                "It runs in a fresh, isolated container{} with {} MB of memory; nothing is kept between calls except workspace files.",
                if self.settings.network {
                    ""
                } else {
                    " without network access (no pip/npm install)"
                },
                self.settings.memory_mb
            )
        } else {
            "It runs on the host, in the workspace.".to_owned()
        };
        ToolSpec {
            name: "code_exec".into(),
            description: format!(
                "Run a Python, JavaScript (node) or bash program and get its output. Use it for calculations, data analysis, parsing files and making charts. The working directory is the workspace, so files there can be read, and files the program writes (e.g. charts saved as PNG) are kept and shown to the user. {isolation}"
            ),
            // Written out flat: small models and some providers stumble on
            // `$ref` and nullable types.
            input_schema: json!({
                "type": "object",
                "properties": {
                    "language": {"type": "string", "enum": ["python", "node", "bash"], "description": "Defaults to python"},
                    "code": {"type": "string", "description": "The program to run. Print what you want to see."},
                    "timeout_secs": {"type": "integer", "minimum": 1, "description": format!("Time limit in seconds (default {}, at most {})", self.settings.timeout_secs, self.settings.max_timeout_secs)}
                },
                "required": ["code"]
            }),
            strict: false,
        }
    }

    fn default_policy(&self) -> ToolPolicy {
        if self.sandbox.isolated() {
            ToolPolicy::Auto
        } else {
            ToolPolicy::Approve
        }
    }

    async fn call(&self, ctx: &ToolCtx, input: Value) -> Result<ToolOutput, ToolError> {
        let a: Args =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        if a.code.trim().is_empty() {
            return Err(ToolError::InvalidInput("code is empty".into()));
        }
        let timeout = a
            .timeout_secs
            .unwrap_or(self.settings.timeout_secs)
            .clamp(1, self.settings.max_timeout_secs.max(1));
        let r = self
            .sandbox
            .exec(
                ExecRequest {
                    language: a.language,
                    code: a.code,
                    workspace: ctx.workspace.clone(),
                    timeout: Duration::from_secs(timeout),
                    run_id: ctx.run_id.clone(),
                },
                ctx.cancel.clone(),
            )
            .await
            .map_err(|e| match e {
                SandboxError::Cancelled => ToolError::Cancelled,
                e => ToolError::Failed(e.to_string()),
            })?;

        let mut text = if r.timed_out {
            format!("stopped after the {timeout}s time limit\n")
        } else {
            match r.exit_code {
                Some(137) => "exit code: 137 (killed, probably out of memory)\n".to_owned(),
                Some(c) => format!("exit code: {c}\n"),
                None => "killed (out of memory?)\n".to_owned(),
            }
        };
        if !r.stdout.is_empty() {
            text.push_str(&format!("stdout:\n{}\n", r.stdout.trim_end()));
        }
        if !r.stderr.is_empty() {
            text.push_str(&format!("stderr:\n{}\n", r.stderr.trim_end()));
        }

        let mut images = Vec::new();
        if !r.changed.is_empty() {
            text.push_str("files written:\n");
            for (i, path) in r.changed.iter().enumerate() {
                let mut line = format!("- {path}");
                if i < MAX_ARTIFACTS
                    && let Some((id, media_type)) = self.keep(ctx, path).await
                {
                    line.push_str(&format!(" ({id})"));
                    if viewable(&media_type) {
                        images.push(ContentBlock::Image {
                            source: MediaSource::Blob { id, media_type },
                        });
                    }
                }
                text.push_str(&line);
                text.push('\n');
            }
        }
        if !r.deleted.is_empty() {
            text.push_str(&format!("files deleted: {}\n", r.deleted.join(", ")));
        }
        if !r.skipped.is_empty() {
            text.push_str(&format!(
                "not copied into the sandbox (over the {} MB limit): {}\n",
                self.settings.max_workspace_mb,
                r.skipped.join(", ")
            ));
        }

        let mut out = ToolOutput {
            is_error: r.timed_out || r.exit_code != Some(0),
            ..ToolOutput::text(text.trim_end())
        }
        .truncated(ctx.settings.max_output_chars);
        out.content.extend(images);
        Ok(out)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::local::LocalSandbox;

    #[derive(Default)]
    struct Kept(Mutex<Vec<(String, String, usize)>>);

    #[async_trait]
    impl ArtifactSink for Kept {
        async fn save(
            &self,
            _ctx: &ToolCtx,
            path: &str,
            media_type: &str,
            data: Vec<u8>,
        ) -> Result<String, String> {
            let mut k = self.0.lock().unwrap();
            k.push((path.into(), media_type.into(), data.len()));
            Ok(format!("blob_{}", k.len()))
        }
    }

    #[tokio::test]
    async fn reports_output_and_keeps_written_files() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolCtx::for_workspace(dir.path());
        let kept = Arc::new(Kept::default());
        let settings = SandboxSettings::default();
        let tool = code_exec(
            Arc::new(LocalSandbox::new(settings.clone())),
            settings,
            Some(kept.clone()),
        );
        assert_eq!(tool.default_policy(), ToolPolicy::Approve);
        let out = tool
            .call(
                &ctx,
                json!({"language": "bash", "code": "echo 42; printf 'a,b' > data.csv; printf 'x' > chart.png"}),
            )
            .await
            .unwrap();
        assert!(!out.is_error);
        let text = out.as_text();
        assert!(text.contains("exit code: 0"), "{text}");
        assert!(text.contains("stdout:\n42"), "{text}");
        assert!(text.contains("- chart.png (blob_1)"), "{text}");
        assert!(text.contains("- data.csv (blob_2)"), "{text}");
        assert_eq!(
            kept.0.lock().unwrap()[1],
            ("data.csv".into(), "text/csv".into(), 3)
        );
        // The PNG is also shown as an image.
        assert!(matches!(
            out.content.last(),
            Some(ContentBlock::Image { source: MediaSource::Blob { id, .. } }) if id == "blob_1"
        ));

        let failed = tool
            .call(&ctx, json!({"language": "sh", "code": "exit 2"}))
            .await
            .unwrap();
        assert!(failed.is_error);
        assert!(failed.as_text().contains("exit code: 2"));
    }
}
