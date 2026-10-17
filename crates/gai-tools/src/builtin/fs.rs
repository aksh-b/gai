//! File tools, confined to the run's workspace directory.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::{FnTool, ToolError, ToolOutput, typed_tool};

fn io(e: std::io::Error) -> ToolError {
    ToolError::Failed(e.to_string())
}

#[derive(Deserialize, JsonSchema)]
pub struct ReadArgs {
    /// Path relative to the workspace.
    path: String,
    /// First line to return, 1-based.
    #[serde(default)]
    offset: Option<usize>,
    /// Number of lines to return.
    #[serde(default)]
    limit: Option<usize>,
}

pub fn fs_read() -> FnTool<ReadArgs> {
    typed_tool(
        "fs_read",
        "Read a UTF-8 text file from the workspace. Lines are prefixed with their numbers. Use offset/limit for large files.",
        |ctx, a: ReadArgs| async move {
            let path = ctx.resolve_path(&a.path)?;
            let text = tokio::fs::read_to_string(&path).await.map_err(|e| ToolError::Failed(format!("{}: {e}", a.path)))?;
            let start = a.offset.unwrap_or(1).max(1);
            let lines: Vec<String> = text
                .lines()
                .enumerate()
                .skip(start - 1)
                .take(a.limit.unwrap_or(usize::MAX))
                .map(|(i, l)| format!("{:>6}\t{l}", i + 1))
                .collect();
            if lines.is_empty() {
                return Ok(ToolOutput::text(if text.is_empty() { "(empty file)" } else { "(no lines in that range)" }));
            }
            Ok(ToolOutput::text(lines.join("\n")))
        },
    )
    .idempotent()
}

#[derive(Deserialize, JsonSchema)]
pub struct WriteArgs {
    /// Path relative to the workspace. Parent directories are created.
    path: String,
    /// The complete new file content.
    content: String,
}

pub fn fs_write() -> FnTool<WriteArgs> {
    typed_tool(
        "fs_write",
        "Create or overwrite a file in the workspace with the given content.",
        |ctx, a: WriteArgs| async move {
            let path = ctx.resolve_path(&a.path)?;
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(io)?;
            }
            tokio::fs::write(&path, a.content.as_bytes())
                .await
                .map_err(io)?;
            Ok(ToolOutput::text(format!(
                "Wrote {} bytes to {}",
                a.content.len(),
                a.path
            )))
        },
    )
    .idempotent()
}

#[derive(Deserialize, JsonSchema)]
pub struct EditArgs {
    /// Path relative to the workspace.
    path: String,
    /// Exact text to replace. Must occur exactly once unless `replace_all`.
    old_text: String,
    new_text: String,
    #[serde(default)]
    replace_all: bool,
}

pub fn fs_edit() -> FnTool<EditArgs> {
    typed_tool(
        "fs_edit",
        "Replace exact text in a workspace file. old_text must match exactly once (whitespace included) unless replace_all is true.",
        |ctx, a: EditArgs| async move {
            let path = ctx.resolve_path(&a.path)?;
            let text = tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| ToolError::Failed(format!("{}: {e}", a.path)))?;
            let count = text.matches(&a.old_text).count();
            if a.old_text.is_empty() || count == 0 {
                return Err(ToolError::InvalidInput(format!(
                    "old_text not found in {}",
                    a.path
                )));
            }
            if count > 1 && !a.replace_all {
                return Err(ToolError::InvalidInput(format!(
                    "old_text occurs {count} times in {}; add surrounding context or set replace_all",
                    a.path
                )));
            }
            let updated = if a.replace_all {
                text.replace(&a.old_text, &a.new_text)
            } else {
                text.replacen(&a.old_text, &a.new_text, 1)
            };
            tokio::fs::write(&path, updated).await.map_err(io)?;
            Ok(ToolOutput::text(format!(
                "Replaced {} occurrence(s) in {}",
                if a.replace_all { count } else { 1 },
                a.path
            )))
        },
    )
}

#[derive(Deserialize, JsonSchema)]
pub struct ListArgs {
    /// Directory relative to the workspace; defaults to the workspace root.
    #[serde(default)]
    path: Option<String>,
    /// Descend into subdirectories.
    #[serde(default)]
    recursive: bool,
}

pub fn fs_list() -> FnTool<ListArgs> {
    typed_tool(
        "fs_list",
        "List files and directories in the workspace. Directories end with '/'.",
        |ctx, a: ListArgs| async move {
            let rel = a.path.unwrap_or_default();
            let root = ctx.resolve_path(if rel.is_empty() { "." } else { &rel })?;
            let mut out = Vec::new();
            let mut stack = vec![root.clone()];
            while let Some(dir) = stack.pop() {
                let mut rd = tokio::fs::read_dir(&dir)
                    .await
                    .map_err(|e| ToolError::Failed(format!("{rel}: {e}")))?;
                while let Some(entry) = rd.next_entry().await.map_err(io)? {
                    let p = entry.path();
                    let shown = p
                        .strip_prefix(&root)
                        .unwrap_or(&p)
                        .to_string_lossy()
                        .replace('\\', "/");
                    let ty = entry.file_type().await.map_err(io)?;
                    if ty.is_dir() {
                        out.push(format!("{shown}/"));
                        if a.recursive && out.len() < 5_000 {
                            stack.push(p);
                        }
                    } else {
                        out.push(shown);
                    }
                }
            }
            out.sort();
            Ok(ToolOutput::text(if out.is_empty() {
                "(empty)".to_owned()
            } else {
                out.join("\n")
            }))
        },
    )
    .idempotent()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Tool, ToolCtx};
    use serde_json::json;

    #[tokio::test]
    async fn write_read_edit_list() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolCtx::for_workspace(dir.path());
        fs_write()
            .call(
                &ctx,
                json!({"path": "notes/a.txt", "content": "one\ntwo\nthree"}),
            )
            .await
            .unwrap();
        let r = fs_read()
            .call(
                &ctx,
                json!({"path": "notes/a.txt", "offset": 2, "limit": 1}),
            )
            .await
            .unwrap();
        assert_eq!(r.as_text(), "     2\ttwo");
        fs_edit()
            .call(
                &ctx,
                json!({"path": "notes/a.txt", "old_text": "two", "new_text": "2"}),
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("notes/a.txt")).unwrap(),
            "one\n2\nthree"
        );
        let l = fs_list()
            .call(&ctx, json!({"recursive": true}))
            .await
            .unwrap();
        assert_eq!(l.as_text(), "notes/\nnotes/a.txt");
        assert!(
            fs_read()
                .call(&ctx, json!({"path": "../etc/passwd"}))
                .await
                .is_err()
        );
    }
}
