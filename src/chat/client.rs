use crate::chat::session::{Message, ToolCall};
use crate::config::Config;
use anyhow::{bail, Context, Result};
use futures_util::{Stream, StreamExt};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::pin::Pin;

#[derive(Debug, Clone, Serialize)]
struct ToolFunctionSchema {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Debug, Clone, Serialize)]
struct ToolDefinition {
    r#type: String,
    function: ToolFunctionSchema,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ToolDefinition>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

impl ChatRequest {
    pub fn new(
        model: impl Into<String>,
        messages: Vec<Message>,
        tool_schemas: Vec<(String, String, Value)>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
    ) -> Self {
        let tools = if tool_schemas.is_empty() {
            None
        } else {
            Some(
                tool_schemas
                    .into_iter()
                    .map(|(name, description, parameters)| ToolDefinition {
                        r#type: "function".to_string(),
                        function: ToolFunctionSchema {
                            name,
                            description,
                            parameters,
                        },
                    })
                    .collect(),
            )
        };
        Self {
            model: model.into(),
            messages,
            tools,
            stream: true,
            temperature,
            max_tokens,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ChatChoice {
    message: Option<Message>,
}

#[derive(Debug, Clone, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct Delta {
    content: Option<String>,
    tool_calls: Option<Vec<ToolCallDelta>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ToolCallDelta {
    index: Option<usize>,
    id: Option<String>,
    #[serde(default)]
    function: FunctionDelta,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct FunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct StreamChoice {
    delta: Delta,
    finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct StreamChunk {
    choices: Vec<StreamChoice>,
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    Start,
    ContentDelta(String),
    ToolCalls(Vec<ToolCall>),
    Done,
}

#[derive(Clone)]
pub struct ChatClient {
    http: reqwest::Client,
    base_url: String,
    model: String,
}

impl ChatClient {
    pub fn new(config: &Config) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if !config.api_key.is_empty() {
            let auth = format!("Bearer {}", config.api_key);
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&auth).context("invalid API key")?,
            );
        }
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(300))
            .build()?;
        Ok(Self {
            http,
            base_url: config.base_url.clone(),
            model: config.model.clone(),
        })
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    fn url(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    pub async fn complete(&self, request: ChatRequest) -> Result<Message> {
        let mut request = request;
        request.stream = false;
        let response = self.http.post(self.url()).json(&request).send().await?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            bail!("StepFun API error {}: {}", status, text);
        }
        let body: ChatCompletionResponse = response.json().await?;
        body.choices
            .into_iter()
            .next()
            .and_then(|c| c.message)
            .context("empty choices")
    }

    pub fn stream(
        &self,
        request: ChatRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>> {
        let client = self.http.clone();
        let url = self.url();
        let body = match serde_json::to_value(request) {
            Ok(v) => v,
            Err(e) => {
                return Box::pin(futures_util::stream::once(async move { Err(e.into()) }));
            }
        };
        Box::pin(async_stream::try_stream! {
            let response = client
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("request failed: {e}"))?;
            let status = response.status();
            let mut stream = response.bytes_stream();
            if !status.is_success() {
                let mut body = Vec::new();
                while let Some(chunk) = stream.next().await {
                    body.extend(chunk.map_err(|e| anyhow::anyhow!("stream error: {e}"))?);
                }
                let text = String::from_utf8_lossy(&body);
                Err(anyhow::anyhow!("StepFun API error {}: {}", status, text))?;
            }
            yield StreamEvent::Start;
            let mut content = String::new();
            let mut partials: Vec<ToolCallPartial> = Vec::new();
            let mut buffer = String::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| anyhow::anyhow!("stream error: {e}"))?;
                let text = String::from_utf8_lossy(&chunk);
                buffer.push_str(&text);
                while let Some(pos) = buffer.find('\n') {
                    let line = buffer.drain(..=pos).collect::<String>();
                    let line = line.trim_end();
                    if line.is_empty() {
                        continue;
                    }
                    if line == "data: [DONE]" {
                        break;
                    }
                    if let Some(data) = line.strip_prefix("data: ") {
                        let chunk: StreamChunk = serde_json::from_str(data)
                            .map_err(|e| anyhow::anyhow!("failed to parse SSE data: {e}\n{data}"))?;
                        for choice in chunk.choices {
                            if let Some(delta) = choice.delta.content {
                                content.push_str(&delta);
                                yield StreamEvent::ContentDelta(delta);
                            }
                            if let Some(deltas) = choice.delta.tool_calls {
                                merge_tool_deltas(&mut partials, deltas);
                            }
                            if let Some(reason) = choice.finish_reason {
                                if reason == "tool_calls" || reason == "function_call" {
                                    let tool_calls = build_tool_calls(&partials)?;
                                    yield StreamEvent::ToolCalls(tool_calls);
                                } else {
                                    yield StreamEvent::Done;
                                }
                                return;
                            }
                        }
                    }
                }
            }
            if partials.iter().any(|p| !p.name.is_empty()) {
                let tool_calls = build_tool_calls(&partials)?;
                yield StreamEvent::ToolCalls(tool_calls);
            } else {
                yield StreamEvent::Done;
            }
        })
    }
}

#[derive(Debug, Default, Clone)]
struct ToolCallPartial {
    id: String,
    name: String,
    arguments: String,
}

fn merge_tool_deltas(partials: &mut Vec<ToolCallPartial>, deltas: Vec<ToolCallDelta>) {
    for d in deltas {
        let index = d.index.unwrap_or(0);
        if partials.len() <= index {
            partials.resize_with(index + 1, ToolCallPartial::default);
        }
        let p = &mut partials[index];
        if let Some(id) = d.id {
            p.id = id;
        }
        if let Some(name) = d.function.name {
            p.name = name;
        }
        if let Some(args) = d.function.arguments {
            p.arguments.push_str(&args);
        }
    }
}

fn build_tool_calls(partials: &[ToolCallPartial]) -> Result<Vec<ToolCall>> {
    partials
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if p.name.is_empty() {
                bail!("tool call {} missing name", i);
            }
            Ok(ToolCall::new(
                if p.id.is_empty() {
                    format!("call_{}", i)
                } else {
                    p.id.clone()
                },
                p.name.clone(),
                p.arguments.clone(),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::TryStreamExt;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_config(uri: String) -> Config {
        Config {
            api_key: "test-key".to_string(),
            base_url: uri,
            model: "step-2-16k".to_string(),
            workspace: Some(std::env::current_dir().unwrap()),
            allow_shell: false,
            yolo: false,
            trust: false,
            max_tokens: None,
            temperature: None,
            max_rounds: 10,
        }
    }

    #[tokio::test]
    async fn stream_parses_content_and_tool_calls() {
        let server = MockServer::start().await;
        let body = r#"
data: {"choices":[{"delta":{"role":"assistant","content":"Hello"},"finish_reason":null}]}

data: {"choices":[{"delta":{"content":"!"},"finish_reason":null}]}

data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"list_dir","arguments":""}}]},"finish_reason":null}]}

data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\": \".\"}"}}]},"finish_reason":"tool_calls"}]}

data: [DONE]

"#;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("Authorization", "Bearer test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;

        let client = ChatClient::new(&test_config(server.uri())).unwrap();
        let request = ChatRequest::new(
            "step-2-16k",
            vec![Message::user("hi")],
            vec![(
                "list_dir".to_string(),
                "list files".to_string(),
                serde_json::json!({"type":"object","properties":{}}),
            )],
            None,
            None,
        );

        let events: Vec<StreamEvent> = client
            .stream(request)
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        let mut content = String::new();
        let mut saw_tools = false;
        for ev in events {
            match ev {
                StreamEvent::ContentDelta(d) => content.push_str(&d),
                StreamEvent::ToolCalls(calls) => {
                    saw_tools = true;
                    assert_eq!(calls.len(), 1);
                    assert_eq!(calls[0].function.name, "list_dir");
                }
                _ => {}
            }
        }
        assert_eq!(content, "Hello!");
        assert!(saw_tools);
    }

    #[tokio::test]
    async fn complete_returns_assistant_message() {
        let server = MockServer::start().await;
        let body = r#"{"choices":[{"index":0,"message":{"role":"assistant","content":"Hi there"},"finish_reason":"stop"}]}"#;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("Authorization", "Bearer test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;

        let client = ChatClient::new(&test_config(server.uri())).unwrap();
        let request = ChatRequest::new(
            "step-2-16k",
            vec![Message::user("hello")],
            vec![],
            None,
            None,
        );
        let msg = client.complete(request).await.unwrap();
        assert_eq!(msg.content.as_deref(), Some("Hi there"));
    }
}
