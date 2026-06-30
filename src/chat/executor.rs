use crate::chat::approver::Approver;
use crate::chat::session::ToolCall;
use crate::chat::tools::{ToolContext, ToolRegistry};
use anyhow::Result;
use serde_json::{json, Value};

const READONLY_TOOLS: &[&str] = &["read_file", "list_dir", "glob_files", "grep_files"];

pub struct Executor<'a> {
    registry: &'a ToolRegistry,
    ctx: &'a ToolContext,
    approver: &'a dyn Approver,
}

impl<'a> Executor<'a> {
    pub fn new(
        registry: &'a ToolRegistry,
        ctx: &'a ToolContext,
        approver: &'a dyn Approver,
    ) -> Self {
        Self {
            registry,
            ctx,
            approver,
        }
    }

    pub async fn execute(&self, calls: Vec<ToolCall>) -> Result<Vec<(String, String)>> {
        let mut results = Vec::with_capacity(calls.len());
        for call in calls {
            let result = self.execute_one(call).await?;
            results.push(result);
        }
        Ok(results)
    }

    async fn execute_one(&self, call: ToolCall) -> Result<(String, String)> {
        let tool = self
            .registry
            .get(&call.function.name)
            .ok_or_else(|| anyhow::anyhow!("unknown tool: {}", call.function.name))?;

        let args: Value = serde_json::from_str(&call.function.arguments).unwrap_or_else(
            |e| json!({"_raw": call.function.arguments, "_parse_error": e.to_string()}),
        );

        let needs_approval = !READONLY_TOOLS.contains(&tool.name()) && !self.ctx.yolo;
        if needs_approval {
            let args_str = serde_json::to_string(&args).unwrap_or_default();
            let approved = self
                .approver
                .approve(&call.id, tool.name(), &args_str)
                .await;
            if !approved {
                return Ok((call.id, "User declined the tool call.".to_string()));
            }
        }

        let output = match tool.execute(args, self.ctx).await {
            Ok(text) => text,
            Err(e) => format!("Error: {}", e),
        };
        Ok((call.id, output))
    }
}
