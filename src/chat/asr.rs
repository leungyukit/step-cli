//! StepFun ASR audio transcription support.
//!
//! Uses the StepFun `stepaudio-2.5-asr` SSE endpoint:
//!   POST {base_url}/audio/asr/sse
//!   Body: JSON with base64-encoded audio.
//!   Response: SSE stream with transcript.text.delta / transcript.text.done / error events.

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose, Engine as _};
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Maximum size for a single audio file (200 MiB).
const MAX_AUDIO_SIZE: u64 = 200 * 1024 * 1024;

/// Default ASR model.
pub const DEFAULT_ASR_MODEL: &str = "stepaudio-2.5-asr";

#[derive(Clone)]
pub struct AsrClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl AsrClient {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: DEFAULT_ASR_MODEL.to_string(),
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    fn url(&self) -> String {
        format!("{}/audio/asr/sse", self.base_url.trim_end_matches('/'))
    }

    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if !self.api_key.is_empty() {
            let auth = format!("Bearer {}", self.api_key);
            if let Ok(value) = HeaderValue::from_str(&auth) {
                headers.insert(AUTHORIZATION, value);
            }
        }
        headers
    }

    /// Transcribe a local audio file and return the full text.
    pub async fn transcribe(&self, path: &Path) -> Result<String> {
        let data =
            std::fs::read(path).with_context(|| format!("failed to read audio file {:?}", path))?;
        if data.len() as u64 > MAX_AUDIO_SIZE {
            bail!(
                "audio file {:?} is too large ({} bytes > {} bytes)",
                path,
                data.len(),
                MAX_AUDIO_SIZE
            );
        }
        let format = audio_format_from_path(path);
        if format == "pcm" {
            bail!("PCM audio requires rate/channel/bits configuration and is not supported yet");
        }
        let b64 = general_purpose::STANDARD.encode(&data);
        let audio_url = format!("data:audio/{};base64,{}", format, b64);
        let body = serde_json::json!({
            "model": self.model,
            "audio": audio_url,
        });

        let response = self
            .http
            .post(self.url())
            .headers(self.headers())
            .json(&body)
            .send()
            .await
            .context("ASR request failed")?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            bail!("StepFun ASR error {}: {}", status, text);
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut transcript = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("ASR stream error")?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(pos) = buffer.find('\n') {
                let line = buffer.drain(..=pos).collect::<String>();
                let line = line.trim_end();
                if line.is_empty() {
                    continue;
                }
                if line == "data: [DONE]" {
                    return Ok(transcript);
                }
                if let Some(data) = line.strip_prefix("data: ") {
                    let event: AsrEvent = serde_json::from_str(data)
                        .with_context(|| format!("failed to parse ASR SSE event: {}", data))?;
                    match event.event_type.as_str() {
                        "transcript.text.delta" => {
                            if let Some(delta) = event.delta {
                                transcript.push_str(&delta);
                            }
                        }
                        "transcript.text.done" => {
                            if let Some(text) = event.text {
                                // If the server sends the full text at done, use it.
                                if !text.is_empty() {
                                    transcript = text;
                                }
                            }
                            return Ok(transcript);
                        }
                        "error" => {
                            let msg = event
                                .message
                                .unwrap_or_else(|| "unknown ASR error".to_string());
                            bail!("ASR error: {}", msg);
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(transcript)
    }

    /// Convenience helper: create a client from config fields.
    pub fn from_config(base_url: &str, api_key: &str, model: Option<&str>) -> Self {
        let mut client = Self::new(base_url, api_key);
        if let Some(m) = model {
            client = client.with_model(m);
        }
        client
    }
}

#[derive(Debug, Clone, Deserialize)]
struct AsrEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    delta: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

/// Guess audio format from file extension.
pub fn audio_format_from_path(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("mp3") => "mp3",
        Some("wav") => "wav",
        Some("ogg") => "ogg",
        Some("opus") => "ogg",
        Some("pcm") => "pcm",
        Some("m4a") => "mp3",
        Some("webm") => "webm",
        _ => "mp3",
    }
}

/// Encode an audio file as base64 (without the data URL prefix).
pub fn encode_audio(path: &Path) -> Result<String> {
    let data =
        std::fs::read(path).with_context(|| format!("failed to read audio file {:?}", path))?;
    Ok(general_purpose::STANDARD.encode(&data))
}

/// Resolve and validate an audio file path.
pub fn resolve_audio_path(raw: &str, workspace: &Path, trust: bool) -> Result<PathBuf> {
    let raw = raw.trim();
    let raw_path = PathBuf::from(raw);
    let base = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let candidate = if raw_path.is_absolute() {
        raw_path
    } else {
        base.join(raw_path)
    };
    let candidate = candidate
        .canonicalize()
        .unwrap_or_else(|_| candidate.clone());
    if !trust && !candidate.starts_with(&base) {
        bail!(
            "audio path {:?} is outside workspace {:?}. Use --trust or /trust to allow.",
            candidate,
            base
        );
    }
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_format_detection() {
        assert_eq!(audio_format_from_path(Path::new("foo.mp3")), "mp3");
        assert_eq!(audio_format_from_path(Path::new("foo.wav")), "wav");
        assert_eq!(audio_format_from_path(Path::new("foo.ogg")), "ogg");
        assert_eq!(audio_format_from_path(Path::new("foo.opus")), "ogg");
        assert_eq!(audio_format_from_path(Path::new("foo.pcm")), "pcm");
        assert_eq!(audio_format_from_path(Path::new("foo.bin")), "mp3");
    }

    #[test]
    fn parses_asr_sse_events() {
        let delta = r#"{"type":"transcript.text.delta","delta":"hello "}"#;
        let event: AsrEvent = serde_json::from_str(delta).unwrap();
        assert_eq!(event.event_type, "transcript.text.delta");
        assert_eq!(event.delta.as_deref(), Some("hello "));

        let done = r#"{"type":"transcript.text.done","text":"hello world"}"#;
        let event: AsrEvent = serde_json::from_str(done).unwrap();
        assert_eq!(event.event_type, "transcript.text.done");
        assert_eq!(event.text.as_deref(), Some("hello world"));

        let err = r#"{"type":"error","message":"content blocked"}"#;
        let event: AsrEvent = serde_json::from_str(err).unwrap();
        assert_eq!(event.event_type, "error");
        assert_eq!(event.message.as_deref(), Some("content blocked"));
    }
}
