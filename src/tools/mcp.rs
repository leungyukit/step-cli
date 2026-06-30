use crate::chat::tools::{Tool, ToolContext, ToolError};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, Mutex};

const PROTOCOL_VERSION: &str = "2024-11-05";

#[derive(Debug, Clone, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpConfig {
    #[serde(alias = "mcpServers")]
    pub mcp_servers: HashMap<String, McpServerConfig>,
}

#[derive(Debug, Clone, Serialize)]
struct JsonRpcRequest<T> {
    jsonrpc: String,
    id: u64,
    method: String,
    params: T,
}

#[derive(Debug, Clone, Deserialize)]
struct JsonRpcResponse<T> {
    id: u64,
    #[serde(default)]
    result: Option<T>,
    #[serde(default)]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ToolsListResult {
    tools: Vec<McpToolInfo>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpToolInfo {
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

#[derive(Debug, Clone, Deserialize)]
struct ToolCallResult {
    #[serde(default)]
    content: Vec<Value>,
    #[serde(default)]
    is_error: bool,
}

struct PendingRequest {
    respond: oneshot::Sender<Result<Value>>,
}

pub struct McpClient {
    next_id: Mutex<u64>,
    stdin: Mutex<tokio::process::ChildStdin>,
    pending: Mutex<HashMap<u64, PendingRequest>>,
    _child: Child,
}

impl McpClient {
    pub async fn start(config: &McpServerConfig) -> Result<Arc<Self>> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .envs(&config.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().context("failed to spawn MCP server")?;
        let stdin = child.stdin.take().context("stdin not available")?;
        let stdout = child.stdout.take().context("stdout not available")?;

        let client = Arc::new(Self {
            next_id: Mutex::new(1),
            stdin: Mutex::new(stdin),
            pending: Mutex::new(HashMap::new()),
            _child: child,
        });

        Self::start_reader(client.clone(), stdout).await?;
        Self::initialize(client.clone()).await?;
        Ok(client)
    }

    async fn start_reader(client: Arc<Self>, stdout: tokio::process::ChildStdout) -> Result<()> {
        let mut reader = BufReader::new(stdout).lines();
        tokio::spawn(async move {
            while let Ok(Some(line)) = reader.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                let response: Result<JsonRpcResponse<Value>, _> = serde_json::from_str(&line);
                if let Ok(resp) = response {
                    let mut pending = client.pending.lock().await;
                    if let Some(req) = pending.remove(&resp.id) {
                        if let Some(err) = resp.error {
                            let _ = req.respond.send(Err(anyhow::anyhow!(
                                "MCP error {}: {}",
                                err.code,
                                err.message
                            )));
                        } else {
                            let _ = req.respond.send(Ok(resp.result.unwrap_or(Value::Null)));
                        }
                    }
                }
            }
        });
        Ok(())
    }

    async fn initialize(client: Arc<Self>) -> Result<()> {
        let params = serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "step-cli", "version": env!("CARGO_PKG_VERSION")}
        });
        let _init: Value = client.request("initialize", params).await?;
        client
            .notify(
                "notifications/initialized",
                Value::Object(Default::default()),
            )
            .await?;
        Ok(())
    }

    async fn next_id(&self) -> u64 {
        let mut id = self.next_id.lock().await;
        *id += 1;
        *id
    }

    pub async fn list_tools(&self) -> Result<Vec<McpToolInfo>> {
        let result: ToolsListResult = self.request("tools/list", Value::Null).await?;
        Ok(result.tools)
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<String> {
        let params = serde_json::json!({
            "name": name,
            "arguments": arguments,
        });
        let result: ToolCallResult = self.request("tools/call", params).await?;
        let mut text = String::new();
        for item in result.content {
            if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
                text.push_str(t);
            } else {
                text.push_str(&serde_json::to_string(&item).unwrap_or_default());
            }
            text.push('\n');
        }
        if result.is_error {
            return Err(anyhow::anyhow!("tool returned error: {}", text));
        }
        Ok(text)
    }

    async fn request<R: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> Result<R> {
        let id = self.next_id().await;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.to_string(),
            params,
        };
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id, PendingRequest { respond: tx });
        }
        let line = serde_json::to_string(&req)? + "\n";
        {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await?;
        }
        let result = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .context("MCP request timed out")??;
        let value: R = serde_json::from_value(result?).context("failed to parse MCP response")?;
        Ok(value)
    }

    async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let line = serde_json::to_string(&req)? + "\n";
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }
}

pub struct McpTool {
    info: McpToolInfo,
    client: Arc<McpClient>,
}

impl McpTool {
    pub fn new(info: McpToolInfo, client: Arc<McpClient>) -> Self {
        Self { info, client }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.info.name
    }

    fn description(&self) -> &str {
        self.info.description.as_deref().unwrap_or("MCP tool")
    }

    fn parameters(&self) -> Value {
        self.info.input_schema.clone()
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<String, ToolError> {
        self.client
            .call_tool(&self.info.name, args)
            .await
            .map_err(|e| ToolError::new(e.to_string()))
    }
}

pub async fn load_mcp_tools(config_path: &std::path::Path) -> Result<Vec<Arc<dyn Tool>>> {
    if !config_path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(config_path)?;
    let config: McpConfig = serde_json::from_str(&text)?;
    let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
    for (name, server_config) in config.mcp_servers {
        match McpClient::start(&server_config).await {
            Ok(client) => match client.list_tools().await {
                Ok(list) => {
                    for info in list {
                        tools.push(Arc::new(McpTool::new(info, client.clone())));
                    }
                }
                Err(e) => eprintln!("MCP server {} list_tools failed: {}", name, e),
            },
            Err(e) => eprintln!("MCP server {} failed to start: {}", name, e),
        }
    }
    Ok(tools)
}
