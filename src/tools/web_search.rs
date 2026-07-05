//! Web search tool for the agent.
//!
//! The tool delegates to a configurable search provider. Currently supported:
//! - `serper` -> https://serper.dev (Google search API)
//! - `tavily` -> https://tavily.com (AI search API)
//! - `duckduckgo` -> https://duckduckgo.com (HTML scraping, no API key needed)
//!
//! The provider and API key are read from `ToolContext` (populated from config,
//! environment variables, or CLI flags).

use crate::chat::tools::{Tool, ToolContext, ToolError};
use anyhow::Result;
use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};

const DEFAULT_NUM_RESULTS: u32 = 5;
const MAX_NUM_RESULTS: u32 = 10;
const MAX_SNIPPET_LEN: usize = 1_000;

/// A single search result.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Provider-specific search backend.
#[async_trait]
trait SearchProvider: Send + Sync {
    async fn search(&self, query: &str, num_results: u32) -> Result<Vec<SearchResult>, ToolError>;
}

fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .unwrap_or_default()
}

/// Serper (https://serper.dev) search provider.
struct SerperProvider {
    api_key: String,
    client: reqwest::Client,
}

impl SerperProvider {
    fn new(api_key: String) -> Self {
        Self {
            api_key,
            client: build_client(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct SerperOrganicResult {
    title: String,
    link: String,
    snippet: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SerperResponse {
    organic: Option<Vec<SerperOrganicResult>>,
}

#[async_trait]
impl SearchProvider for SerperProvider {
    async fn search(&self, query: &str, num_results: u32) -> Result<Vec<SearchResult>, ToolError> {
        let body = json!({
            "q": query,
            "num": num_results,
        });

        let response = self
            .client
            .post("https://google.serper.dev/search")
            .header("X-API-KEY", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ToolError::new(format!("failed to call Serper API: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response
                .text()
                .await
                .unwrap_or_else(|_| "<could not read error body>".to_string());
            return Err(ToolError::new(format!(
                "Serper API returned {}: {}",
                status, text
            )));
        }

        let parsed: SerperResponse = response
            .json()
            .await
            .map_err(|e| ToolError::new(format!("failed to parse Serper API response: {}", e)))?;

        let results: Vec<SearchResult> = parsed
            .organic
            .unwrap_or_default()
            .into_iter()
            .map(|r| SearchResult {
                title: r.title,
                url: r.link,
                snippet: r.snippet.unwrap_or_default(),
            })
            .collect();

        Ok(results)
    }
}

/// Tavily (https://tavily.com) search provider.
struct TavilyProvider {
    api_key: String,
    client: reqwest::Client,
}

impl TavilyProvider {
    fn new(api_key: String) -> Self {
        Self {
            api_key,
            client: build_client(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct TavilyResult {
    title: String,
    url: String,
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TavilyResponse {
    results: Option<Vec<TavilyResult>>,
}

#[async_trait]
impl SearchProvider for TavilyProvider {
    async fn search(&self, query: &str, num_results: u32) -> Result<Vec<SearchResult>, ToolError> {
        let body = json!({
            "api_key": self.api_key,
            "query": query,
            "search_depth": "basic",
            "max_results": num_results,
        });

        let response = self
            .client
            .post("https://api.tavily.com/search")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ToolError::new(format!("failed to call Tavily API: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response
                .text()
                .await
                .unwrap_or_else(|_| "<could not read error body>".to_string());
            return Err(ToolError::new(format!(
                "Tavily API returned {}: {}",
                status, text
            )));
        }

        let parsed: TavilyResponse = response
            .json()
            .await
            .map_err(|e| ToolError::new(format!("failed to parse Tavily API response: {}", e)))?;

        let results: Vec<SearchResult> = parsed
            .results
            .unwrap_or_default()
            .into_iter()
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.content.unwrap_or_default(),
            })
            .collect();

        Ok(results)
    }
}

/// DuckDuckGo HTML scraping provider.
///
/// This is a free fallback but can break if DuckDuckGo changes their HTML,
/// and may be rate-limited or blocked. No API key is required.
struct DuckDuckGoProvider {
    client: reqwest::Client,
}

impl DuckDuckGoProvider {
    fn new() -> Self {
        Self {
            client: build_client(),
        }
    }
}

#[async_trait]
impl SearchProvider for DuckDuckGoProvider {
    async fn search(&self, query: &str, num_results: u32) -> Result<Vec<SearchResult>, ToolError> {
        // html.duckduckgo.com returns classic HTML results.
        let url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding::encode(query)
        );

        let response = self
            .client
            .get(&url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            )
            .send()
            .await
            .map_err(|e| ToolError::new(format!("failed to call DuckDuckGo: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(ToolError::new(format!(
                "DuckDuckGo returned {}. Try a different search provider.",
                status
            )));
        }

        let html = response
            .text()
            .await
            .map_err(|e| ToolError::new(format!("failed to read DuckDuckGo response: {}", e)))?;

        let results = parse_duckduckgo_html(&html, num_results as usize)?;
        Ok(results)
    }
}

fn parse_duckduckgo_html(html: &str, limit: usize) -> Result<Vec<SearchResult>, ToolError> {
    // DuckDuckGo classic HTML uses result blocks with class "result".
    // We split on "<div class=\"result results_links" and parse each block.
    let title_re = Regex::new(r#"<a[^>]*class="result__a"[^>]*href="([^"]*)"[^>]*>(.*?)</a>"#)
        .map_err(|e| ToolError::new(format!("regex error: {}", e)))?;
    let snippet_re = Regex::new(r#"<a[^>]*class="result__snippet"[^>]*>(.*?)</a>"#)
        .map_err(|e| ToolError::new(format!("regex error: {}", e)))?;

    let mut results = Vec::new();
    // Split the HTML into candidate result blocks.
    for block in html.split("<div class=\"result results_links") {
        if results.len() >= limit {
            break;
        }

        let title_caps = title_re.captures(block);
        let snippet_caps = snippet_re.captures(block);

        if let Some(title_caps) = title_caps {
            let url = html_decode(&title_caps[1]);
            let title = strip_html_tags(&title_caps[2]);
            let snippet = snippet_caps
                .map(|c| strip_html_tags(&c[1]))
                .unwrap_or_default();

            if !title.is_empty() && !url.is_empty() {
                results.push(SearchResult {
                    title,
                    url,
                    snippet,
                });
            }
        }
    }

    Ok(results)
}

/// Decode a handful of common HTML entities.
fn html_decode(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// Remove simple HTML tags from a string.
fn strip_html_tags(html: &str) -> String {
    let re = Regex::new(r"<[^>]+>").unwrap();
    let cleaned = re.replace_all(html, "");
    html_decode(&cleaned).trim().to_string()
}

pub struct WebSearchTool;

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web for current information. Supports serper, tavily (API key required) and duckduckgo (free, may be less stable)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "num_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default 5, max 10)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String, ToolError> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("missing query"))?
            .to_string();

        let num_results = args
            .get("num_results")
            .and_then(|v| v.as_u64())
            .map(|n| (n as u32).clamp(1, MAX_NUM_RESULTS))
            .unwrap_or(DEFAULT_NUM_RESULTS);

        let provider_name =
            ctx.search_provider
                .as_deref()
                .ok_or_else(|| {
                    ToolError::new(
                    "No search provider configured. Set search_provider in ~/.step/config.toml, \
                     STEP_SEARCH_PROVIDER env var, or --search-provider CLI flag. \
                     Supported: serper, tavily, duckduckgo.".to_string(),
                )
                })?
                .to_lowercase();

        let provider: Box<dyn SearchProvider> = match provider_name.as_str() {
            "serper" => {
                let api_key = ctx.search_api_key.as_deref().ok_or_else(|| {
                    ToolError::new(
                        "serper requires search_api_key. Set it in config, \
                         STEP_SEARCH_API_KEY env var, or --search-api-key CLI flag."
                            .to_string(),
                    )
                })?;
                Box::new(SerperProvider::new(api_key.to_string()))
            }
            "tavily" => {
                let api_key = ctx.search_api_key.as_deref().ok_or_else(|| {
                    ToolError::new(
                        "tavily requires search_api_key. Set it in config, \
                         STEP_SEARCH_API_KEY env var, or --search-api-key CLI flag."
                            .to_string(),
                    )
                })?;
                Box::new(TavilyProvider::new(api_key.to_string()))
            }
            "duckduckgo" => Box::new(DuckDuckGoProvider::new()),
            other => {
                return Err(ToolError::new(format!(
                    "unsupported search provider: {}. Supported: serper, tavily, duckduckgo.",
                    other
                )))
            }
        };

        let results = provider.search(&query, num_results).await?;

        if results.is_empty() {
            return Ok("No web search results found.".to_string());
        }

        let mut text = String::new();
        for (i, result) in results.iter().enumerate() {
            let snippet = if result.snippet.len() > MAX_SNIPPET_LEN {
                format!("{}...", &result.snippet[..MAX_SNIPPET_LEN])
            } else {
                result.snippet.clone()
            };
            text.push_str(&format!(
                "{}. {}\n   URL: {}\n   Snippet: {}\n\n",
                i + 1,
                result.title,
                result.url,
                snippet.trim()
            ));
        }
        Ok(text.trim_end().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_search_results() {
        let results = vec![
            SearchResult {
                title: "Rust Book".to_string(),
                url: "https://doc.rust-lang.org/book/".to_string(),
                snippet: "The Rust Programming Language book.".to_string(),
            },
            SearchResult {
                title: "Rust by Example".to_string(),
                url: "https://doc.rust-lang.org/rust-by-example/".to_string(),
                snippet: "Rust by Example has runnable examples.".to_string(),
            },
        ];

        let mut text = String::new();
        for (i, result) in results.iter().enumerate() {
            text.push_str(&format!(
                "{}. {}\n   URL: {}\n   Snippet: {}\n\n",
                i + 1,
                result.title,
                result.url,
                result.snippet.trim()
            ));
        }

        assert!(text.contains("Rust Book"));
        assert!(text.contains("https://doc.rust-lang.org/book/"));
        assert!(text.contains("Rust by Example"));
    }

    #[test]
    fn parses_duckduckgo_html() {
        let html = r#"
        <div class="result results_links results_links_deep web-result">
            <a class="result__a" href="https://example.com/1">Title <b>One</b></a>
            <a class="result__snippet">Snippet number one.</a>
        </div>
        <div class="result results_links results_links_deep web-result">
            <a class="result__a" href="https://example.com/2">Title Two</a>
            <a class="result__snippet">Snippet number two.</a>
        </div>
        "#;

        let results = parse_duckduckgo_html(html, 10).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Title One");
        assert_eq!(results[0].url, "https://example.com/1");
        assert_eq!(results[0].snippet, "Snippet number one.");
        assert_eq!(results[1].title, "Title Two");
    }

    #[test]
    fn html_decode_and_strip_tags() {
        assert_eq!(strip_html_tags("<b>Hello</b> &amp; world"), "Hello & world");
    }
}
