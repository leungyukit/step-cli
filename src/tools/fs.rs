use crate::chat::tools::{Tool, ToolContext, ToolError};
use anyhow::Result;
use async_trait::async_trait;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

fn resolve_path(ctx: &ToolContext, raw: &str) -> Result<PathBuf, ToolError> {
    let raw = raw.trim();
    let raw_path = PathBuf::from(raw);
    let base = ctx
        .workspace
        .canonicalize()
        .map_err(|e| ToolError::new(format!("invalid workspace: {}", e)))?;
    let candidate = if raw_path.is_absolute() {
        raw_path
    } else {
        base.join(raw_path)
    };
    let candidate = candidate
        .canonicalize()
        .unwrap_or_else(|_| candidate.clone());
    if !ctx.trust && !candidate.starts_with(&base) {
        return Err(ToolError::new(format!(
            "path {:?} is outside workspace {:?}. Use --trust or /trust to allow.",
            candidate, base
        )));
    }
    Ok(candidate)
}

fn get_string(args: &Value, key: &str) -> Result<String, ToolError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| ToolError::new(format!("missing or invalid parameter: {}", key)))
}

fn get_usize(args: &Value, key: &str) -> Option<usize> {
    args.get(key).and_then(|v| v.as_u64()).map(|n| n as usize)
}

fn truncate(text: &str, max_lines: usize, max_bytes: usize) -> String {
    let mut out = text.to_string();
    if out.len() > max_bytes {
        out.truncate(max_bytes);
        out.push_str("\n...[truncated by bytes]...");
    }
    let lines: Vec<&str> = out.lines().collect();
    if lines.len() > max_lines {
        let half = max_lines / 2;
        let head = lines
            .iter()
            .take(half)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        let tail = lines
            .iter()
            .skip(lines.len() - half)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        out = format!(
            "{}\n...[{} lines truncated]...\n{}",
            head,
            lines.len() - max_lines,
            tail
        );
    }
    out
}

pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a text file. Optional offset (1-based) and limit."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Relative or absolute file path" },
                "offset": { "type": "integer", "description": "1-based start line" },
                "limit": { "type": "integer", "description": "Max lines to read" }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String, ToolError> {
        let path = resolve_path(ctx, &get_string(&args, "path")?)?;
        let offset = get_usize(&args, "offset").unwrap_or(1).max(1);
        let limit = get_usize(&args, "limit").unwrap_or(2000);
        if !path.exists() {
            return Err(ToolError::new(format!("file not found: {:?}", path)));
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| ToolError::new(format!("failed to read {:?}: {}", path, e)))?;
        let lines: Vec<&str> = text.lines().collect();
        let start = offset - 1;
        let end = (start + limit).min(lines.len());
        let selected = if start >= lines.len() {
            String::new()
        } else {
            lines[start..end].join("\n")
        };
        Ok(truncate(&selected, 2000, 1024 * 1024))
    }
}

pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Create or overwrite a file. Parent directories are created automatically."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Relative or absolute file path" },
                "content": { "type": "string", "description": "Full file content" }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String, ToolError> {
        let path = resolve_path(ctx, &get_string(&args, "path")?)?;
        let content = get_string(&args, "content")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ToolError::new(format!("failed to create directories: {}", e)))?;
        }
        std::fs::write(&path, content)
            .map_err(|e| ToolError::new(format!("failed to write {:?}: {}", path, e)))?;
        Ok(format!("Wrote {:?}", path))
    }
}

pub struct EditFileTool;

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Replace the first occurrence of old_string with new_string in a file."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "old_string": { "type": "string" },
                "new_string": { "type": "string" }
            },
            "required": ["path", "old_string", "new_string"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String, ToolError> {
        let path = resolve_path(ctx, &get_string(&args, "path")?)?;
        let old = get_string(&args, "old_string")?;
        let new = get_string(&args, "new_string")?;
        if old.is_empty() {
            return Err(ToolError::new("old_string must not be empty"));
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| ToolError::new(format!("failed to read {:?}: {}", path, e)))?;
        let pos = text
            .find(&old)
            .ok_or_else(|| ToolError::new(format!("old_string not found in {:?}", path)))?;
        let mut result = text[..pos].to_string();
        result.push_str(&new);
        result.push_str(&text[pos + old.len()..]);
        std::fs::write(&path, result)
            .map_err(|e| ToolError::new(format!("failed to write {:?}: {}", path, e)))?;
        Ok(format!("Edited {:?}", path))
    }
}

pub struct ListDirTool;

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "List files and directories. Defaults to the workspace root."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Relative or absolute directory path" }
            }
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String, ToolError> {
        let path = if let Ok(p) = get_string(&args, "path") {
            resolve_path(ctx, &p)?
        } else {
            ctx.workspace.clone()
        };
        let entries = std::fs::read_dir(&path)
            .map_err(|e| ToolError::new(format!("failed to read dir {:?}: {}", path, e)))?;
        let mut lines: Vec<String> = Vec::new();
        for entry in entries.flatten() {
            let meta = entry.metadata();
            let kind = meta
                .map(|m| {
                    if m.is_dir() {
                        "dir"
                    } else if m.is_symlink() {
                        "link"
                    } else {
                        "file"
                    }
                })
                .unwrap_or("unknown");
            lines.push(format!("{} {}", kind, entry.file_name().to_string_lossy()));
        }
        lines.sort();
        Ok(lines.join("\n"))
    }
}

pub struct GlobFilesTool;

#[async_trait]
impl Tool for GlobFilesTool {
    fn name(&self) -> &str {
        "glob_files"
    }

    fn description(&self) -> &str {
        "Find files matching a glob pattern relative to the workspace."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Glob pattern, e.g. src/**/*.rs" }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String, ToolError> {
        let pattern = get_string(&args, "pattern")?;
        let base = ctx.workspace.clone();
        let full = if Path::new(&pattern).is_absolute() {
            pattern.clone()
        } else {
            base.join(&pattern).to_string_lossy().to_string()
        };
        let paths = glob::glob(&full)
            .map_err(|e| ToolError::new(format!("invalid glob pattern: {}", e)))?;
        let mut results: Vec<String> = Vec::new();
        for p in paths.flatten() {
            if !ctx.trust && !p.starts_with(&base) {
                continue;
            }
            results.push(p.to_string_lossy().to_string());
        }
        results.sort();
        results.truncate(200);
        Ok(results.join("\n"))
    }
}

pub struct GrepFilesTool;

#[async_trait]
impl Tool for GrepFilesTool {
    fn name(&self) -> &str {
        "grep_files"
    }

    fn description(&self) -> &str {
        "Search file contents with a regex pattern under a directory."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Regex pattern" },
                "path": { "type": "string", "description": "Directory to search (defaults to workspace)" },
                "glob": { "type": "string", "description": "Optional glob filter, e.g. *.rs" }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String, ToolError> {
        let pattern = get_string(&args, "pattern")?;
        let regex =
            Regex::new(&pattern).map_err(|e| ToolError::new(format!("invalid regex: {}", e)))?;
        let root = if let Ok(p) = get_string(&args, "path") {
            resolve_path(ctx, &p)?
        } else {
            ctx.workspace.clone()
        };
        let glob_filter = get_string(&args, "glob").ok();
        let mut results: Vec<String> = Vec::new();
        let mut visited: HashSet<PathBuf> = HashSet::new();
        search_dir(
            &root,
            &regex,
            &glob_filter,
            ctx,
            &mut results,
            &mut visited,
            0,
        )?;
        results.truncate(200);
        Ok(results.join("\n"))
    }
}

fn search_dir(
    dir: &Path,
    regex: &Regex,
    glob_filter: &Option<String>,
    ctx: &ToolContext,
    results: &mut Vec<String>,
    visited: &mut HashSet<PathBuf>,
    depth: usize,
) -> Result<(), ToolError> {
    if depth > 16 {
        return Ok(());
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !ctx.trust && !path.starts_with(&ctx.workspace) {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            if meta.is_dir() {
                let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
                if visited.insert(canonical) {
                    search_dir(&path, regex, glob_filter, ctx, results, visited, depth + 1)?;
                }
                continue;
            }
        }
        if let Some(ref g) = glob_filter {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default();
            if !glob::Pattern::new(g)
                .map(|p| p.matches(&name))
                .unwrap_or(false)
            {
                continue;
            }
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        for (i, line) in text.lines().enumerate() {
            if regex.is_match(line) {
                results.push(format!("{}:{}:{}", path.display(), i + 1, line));
                if results.len() >= 200 {
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(tmp: &std::path::Path) -> ToolContext {
        ToolContext {
            workspace: tmp.to_path_buf(),
            trust: false,
            yolo: false,
            allow_shell: false,
            job_manager: None,
        }
    }

    #[tokio::test]
    async fn read_write_edit_tools() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx(tmp.path());

        let write = WriteFileTool;
        write
            .execute(json!({"path":"hello.txt","content":"hello world"}), &ctx)
            .await
            .unwrap();

        let read = ReadFileTool;
        let content = read
            .execute(json!({"path":"hello.txt"}), &ctx)
            .await
            .unwrap();
        assert_eq!(content, "hello world");

        let edit = EditFileTool;
        edit.execute(
            json!({"path":"hello.txt","old_string":"world","new_string":"StepFun"}),
            &ctx,
        )
        .await
        .unwrap();
        let content = read
            .execute(json!({"path":"hello.txt"}), &ctx)
            .await
            .unwrap();
        assert_eq!(content, "hello StepFun");
    }

    #[tokio::test]
    async fn glob_and_grep() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx(tmp.path());

        std::fs::write(tmp.path().join("a.rs"), "fn main() {}\n").unwrap();
        std::fs::write(tmp.path().join("b.rs"), "fn helper() {}\n").unwrap();

        let list = ListDirTool;
        let out = list.execute(json!({}), &ctx).await.unwrap();
        assert!(out.contains("a.rs"));
        assert!(out.contains("b.rs"));

        let glob = GlobFilesTool;
        let out = glob.execute(json!({"pattern":"*.rs"}), &ctx).await.unwrap();
        assert!(out.contains("a.rs"));

        let grep = GrepFilesTool;
        let out = grep
            .execute(json!({"pattern":"helper","glob":"*.rs"}), &ctx)
            .await
            .unwrap();
        assert!(out.contains("b.rs"));
    }

    #[tokio::test]
    async fn workspace_boundary_blocks_outside_path() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ctx = ctx(tmp.path());
        ctx.trust = false;

        let read = ReadFileTool;
        let result = read.execute(json!({"path":"/etc/passwd"}), &ctx).await;
        assert!(result.is_err());
    }
}
