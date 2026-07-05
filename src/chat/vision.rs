//! Vision / multimodal image support.
//!
//! Encodes local image files as base64 data URLs so they can be attached to
//! user messages for vision-capable StepFun models.

use crate::chat::session::{Content, ContentPart};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose, Engine as _};
use std::path::{Path, PathBuf};

/// Maximum size for a single attached image (20 MiB).
const MAX_IMAGE_SIZE: u64 = 20 * 1024 * 1024;

/// Returns true if the model name is known to support image input.
pub fn is_vision_model(model: &str) -> bool {
    // For "org/model" names the model id is the last segment.
    let base = model
        .split('/')
        .next_back()
        .unwrap_or(model)
        .trim()
        .to_lowercase();
    matches!(
        base.as_str(),
        "step-1o-turbo-vision"
            | "step-1o-vision"
            | "step-1v"
            | "step-1v-8k"
            | "step-1v-32k"
            | "gpt-4o"
            | "gpt-4o-mini"
            | "gpt-4-turbo"
            | "gpt-4-vision-preview"
    ) || base.contains("vision")
        || base.contains("-1o-")
}

/// Resolve a raw image path against the workspace, enforcing workspace
/// boundaries unless `trust` is enabled.
pub fn resolve_image_path(raw: &str, workspace: &Path, trust: bool) -> Result<PathBuf> {
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
            "image path {:?} is outside workspace {:?}. Use --trust or /trust to allow.",
            candidate,
            base
        );
    }
    Ok(candidate)
}

/// Encode a local image file as a base64 data URL.
pub fn encode_image(path: &Path) -> Result<String> {
    let data = std::fs::read(path).with_context(|| format!("failed to read image {:?}", path))?;
    if data.len() as u64 > MAX_IMAGE_SIZE {
        bail!(
            "image {:?} is too large ({} bytes > {} bytes)",
            path,
            data.len(),
            MAX_IMAGE_SIZE
        );
    }
    let mime = mime_type_from_extension(path);
    let b64 = general_purpose::STANDARD.encode(&data);
    Ok(format!("data:{};base64,{}", mime, b64))
}

fn mime_type_from_extension(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        Some("svg") => "image/svg+xml",
        _ => "image/png",
    }
}

/// Build user message content from text and a list of local image paths.
pub fn build_user_content(text: &str, image_paths: &[PathBuf]) -> Result<Content> {
    if image_paths.is_empty() {
        return Ok(Content::text(text));
    }
    let mut parts = vec![ContentPart::text(text)];
    for path in image_paths {
        let url = encode_image(path)?;
        parts.push(ContentPart::image_url(url));
    }
    Ok(Content::parts(parts))
}

/// Extract Markdown image references `![alt](path)` from the input text.
///
/// Returns the text with image references removed, plus the list of paths.
pub fn extract_image_paths(text: &str) -> (String, Vec<String>) {
    let mut paths = Vec::new();
    let mut result = String::with_capacity(text.len());
    let mut remaining = text;

    // Simple char-by-char scanner for `![...](...)`.
    while let Some(start) = remaining.find("![") {
        result.push_str(&remaining[..start]);
        remaining = &remaining[start..];

        // Find closing `]` of alt text.
        if let Some(close_bracket) = remaining.find(']') {
            let after_bracket = &remaining[close_bracket..];
            if let Some(open_paren) = after_bracket.find('(') {
                let after_paren = &after_bracket[open_paren + 1..];
                if let Some(close_paren) = after_paren.find(')') {
                    let path = &after_paren[..close_paren];
                    if !path.is_empty() {
                        paths.push(path.to_string());
                    }
                    remaining = &after_paren[close_paren + 1..];
                    continue;
                }
            }
        }

        // Not a well-formed image reference; keep the literal.
        result.push_str(&remaining[..2]);
        remaining = &remaining[2..];
    }
    result.push_str(remaining);
    (result, paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_vision_models() {
        assert!(is_vision_model("step-1o-turbo-vision"));
        assert!(is_vision_model("some-org/step-1o-turbo-vision"));
        assert!(is_vision_model("gpt-4o"));
        assert!(is_vision_model("custom-vision-model"));
        assert!(!is_vision_model("step-2-16k"));
        assert!(!is_vision_model("step-3.7-flash"));
    }

    #[test]
    fn extracts_markdown_image_paths() {
        let text = "Check this ![screenshot](img/foo.png) and ![diagram](img/bar.jpg).";
        let (cleaned, paths) = extract_image_paths(text);
        assert_eq!(cleaned, "Check this  and .");
        assert_eq!(paths, vec!["img/foo.png", "img/bar.jpg"]);
    }

    #[test]
    fn leaves_plain_text_unchanged() {
        let text = "No images here.";
        let (cleaned, paths) = extract_image_paths(text);
        assert_eq!(cleaned, text);
        assert!(paths.is_empty());
    }

    #[test]
    fn encode_image_rejects_missing_file() {
        let result = encode_image(Path::new("/nonexistent/image.png"));
        assert!(result.is_err());
    }
}
