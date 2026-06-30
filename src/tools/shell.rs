use crate::chat::tools::{Tool, ToolContext, ToolError};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

const DANGEROUS_PATTERNS: &[&str] = &[
    "rm -rf /",
    "rm -rf /*",
    ":(){ :|:& };:",
    "> /dev/sda",
    ">/dev/sda",
    "mkfs.",
    "dd if=/dev/zero of=/dev/",
];

pub struct ExecShellTool;

#[async_trait]
impl Tool for ExecShellTool {
    fn name(&self) -> &str {
        "exec_shell"
    }

    fn description(&self) -> &str {
        "Run a shell command. Optional working_dir and timeout (seconds). Set background=true to run it as a background job."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Shell command to execute" },
                "working_dir": { "type": "string", "description": "Working directory relative to workspace" },
                "timeout": { "type": "integer", "description": "Timeout in seconds (default 30)" },
                "background": { "type": "boolean", "description": "Run as a background job" }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String, ToolError> {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("missing command"))?
            .to_string();
        let working_dir = args.get("working_dir").and_then(|v| v.as_str()).map(|s| {
            if PathBuf::from(s).is_absolute() {
                PathBuf::from(s)
            } else {
                ctx.workspace.join(s)
            }
        });
        let seconds = args
            .get("timeout")
            .and_then(|v| v.as_u64())
            .map(|s| s.max(1))
            .unwrap_or(30);
        let background = args
            .get("background")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if !ctx.allow_shell && !ctx.yolo {
            return Err(ToolError::new(
                "shell execution is disabled. Set allow_shell=true in config, STEP_ALLOW_SHELL=1, or use --yolo.",
            ));
        }

        let lower = command.to_lowercase();
        for pat in DANGEROUS_PATTERNS {
            if lower.contains(pat) {
                return Err(ToolError::new(format!(
                    "dangerous command blocked: {}",
                    command
                )));
            }
        }

        let cwd = working_dir.unwrap_or_else(|| ctx.workspace.clone());

        if background {
            if let Some(jm) = ctx.job_manager.as_ref() {
                let id = jm
                    .lock()
                    .await
                    .spawn(command, cwd)
                    .await
                    .map_err(|e| ToolError::new(format!("failed to spawn job: {}", e)))?;
                return Ok(format!("Started background job {}", id));
            } else {
                return Err(ToolError::new("background jobs not available in this mode"));
            }
        }

        let (program, arg) = if cfg!(target_os = "windows") {
            ("cmd", "/C")
        } else {
            ("sh", "-c")
        };

        let mut cmd = Command::new(program);
        cmd.arg(arg).arg(&command).current_dir(&cwd);
        cmd.kill_on_drop(true);

        let result = timeout(Duration::from_secs(seconds), cmd.output()).await;
        let output = match result {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => return Err(ToolError::new(format!("failed to spawn command: {}", e))),
            Err(_) => return Err(ToolError::new("command timed out")),
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let mut text = String::new();
        if !stdout.is_empty() {
            text.push_str("STDOUT:\n");
            text.push_str(&stdout);
        }
        if !stderr.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str("STDERR:\n");
            text.push_str(&stderr);
        }
        if !output.status.success() {
            text.push_str(&format!(
                "\nEXIT_CODE: {}",
                output.status.code().unwrap_or(-1)
            ));
        }
        if text.len() > 64 * 1024 {
            text.truncate(64 * 1024);
            text.push_str("\n...[output truncated]...");
        }
        Ok(text)
    }
}
