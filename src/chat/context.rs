//! Context-window monitoring and compression for chat sessions.
//!
//! Tracks the accumulated token size of `Session.messages` against a model-specific
//! context limit. When the size exceeds a configurable threshold (default 80%),
//! older non-system messages are dropped and a short system notice is inserted so
//! the model knows the context was compressed.

use crate::chat::session::{Message, Role};
use crate::chat::tokens::count_tokens;

/// Default context limit used when the model name is not recognized.
pub const DEFAULT_CONTEXT_LIMIT: usize = 16_384;

/// Default compression threshold (80% of the model's context window).
pub const DEFAULT_CONTEXT_THRESHOLD: f32 = 0.8;

/// Return the context-window size for a given StepFun model name.
/// Unknown models fall back to `DEFAULT_CONTEXT_LIMIT`.
pub fn model_context_limit(model: &str) -> usize {
    // Strip a possible organization or version suffix after a slash.
    let base = model.split('/').next().unwrap_or(model).trim();
    match base {
        "step-1-8k" => 8_192,
        "step-1-32k" => 32_768,
        "step-1-128k" => 128_000,
        "step-2-16k" => 16_384,
        "step-2-32k" => 32_768,
        "step-2-128k" => 128_000,
        "step-3.7-flash" => 32_768,
        "step-3-mini" => 32_768,
        "step-3" => 32_768,
        _ => DEFAULT_CONTEXT_LIMIT,
    }
}

/// Count tokens in a single message (content + tool call names/arguments).
fn count_message_tokens(msg: &Message) -> usize {
    let mut total = 0;
    if let Some(content) = &msg.content {
        total += count_tokens(content);
    }
    if let Some(tool_calls) = &msg.tool_calls {
        for call in tool_calls {
            total += count_tokens(&call.function.name);
            total += count_tokens(&call.function.arguments);
        }
    }
    total
}

/// Count tokens across an entire message list.
pub fn count_session_tokens(messages: &[Message]) -> usize {
    messages.iter().map(count_message_tokens).sum()
}

/// Maximum allowed tokens before compression is triggered.
pub fn token_budget(limit: usize, threshold: f32) -> usize {
    (limit as f32 * threshold) as usize
}

/// Compress `messages` if its token count exceeds the threshold for `model`.
///
/// System messages are always preserved. The oldest non-system messages are
/// dropped until the remaining history fits within the budget. A short system
/// notice is inserted listing how many messages were removed.
///
/// Returns `true` if compression was performed.
pub fn compress_if_needed(messages: &mut Vec<Message>, model: &str, threshold: f32) -> bool {
    let limit = model_context_limit(model);
    let budget = token_budget(limit, threshold);

    if count_session_tokens(messages) <= budget {
        return false;
    }

    let system_messages: Vec<Message> = messages
        .iter()
        .filter(|m| m.role == Role::System)
        .cloned()
        .collect();
    let non_system: Vec<Message> = messages
        .iter()
        .filter(|m| m.role != Role::System)
        .cloned()
        .collect();

    let system_tokens = count_session_tokens(&system_messages);
    if system_tokens > budget {
        // Even the system prompts alone exceed the budget; nothing safe to drop.
        return false;
    }

    // Keep as many recent non-system messages as the budget allows.
    let mut kept_non_system: Vec<Message> = Vec::new();
    let mut kept_tokens = system_tokens;
    for msg in non_system.iter().rev() {
        let msg_tokens = count_message_tokens(msg);
        if kept_tokens + msg_tokens > budget && !kept_non_system.is_empty() {
            break;
        }
        kept_tokens += msg_tokens;
        kept_non_system.push(msg.clone());
    }
    kept_non_system.reverse();

    let removed = non_system.len().saturating_sub(kept_non_system.len());
    if removed == 0 {
        return false;
    }

    // Build a summary notice and make sure it still fits in the budget.
    let summary = Message::system(format!(
        "[Context compressed: {} earlier messages were removed to stay within the {} token budget. Recent context is preserved below.]",
        removed, budget
    ));
    let summary_tokens = count_message_tokens(&summary);

    // If the summary would push us back over budget, drop older kept messages
    // until it fits. In pathological cases this can end up keeping nothing.
    while kept_tokens + summary_tokens > budget && !kept_non_system.is_empty() {
        let dropped = kept_non_system.remove(0);
        kept_tokens -= count_message_tokens(&dropped);
    }

    let mut compressed = system_messages;
    if kept_tokens + summary_tokens <= budget {
        compressed.push(summary);
    }
    compressed.extend(kept_non_system);

    *messages = compressed;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_limits_are_known() {
        assert_eq!(model_context_limit("step-1-8k"), 8_192);
        assert_eq!(model_context_limit("step-1-32k"), 32_768);
        assert_eq!(model_context_limit("step-2-16k"), 16_384);
        assert_eq!(model_context_limit("step-3.7-flash"), 32_768);
    }

    #[test]
    fn unknown_model_uses_default() {
        assert_eq!(
            model_context_limit("some-future-model"),
            DEFAULT_CONTEXT_LIMIT
        );
    }

    #[test]
    fn counts_session_tokens() {
        let messages = vec![
            Message::system("You are a helpful assistant."),
            Message::user("Hello!"),
            Message::assistant("Hi there!"),
        ];
        let tokens = count_session_tokens(&messages);
        assert!(tokens > 0);
    }

    #[test]
    fn no_compression_when_under_budget() {
        let mut messages = vec![
            Message::system("System prompt."),
            Message::user("Short question."),
        ];
        let compressed = compress_if_needed(&mut messages, "step-2-16k", 0.8);
        assert!(!compressed);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn compression_drops_oldest_non_system_messages() {
        // Threshold small enough to trigger compression but large enough for the
        // system prompt + a few recent messages + summary notice.
        let threshold = 0.01;
        let mut messages = vec![Message::system("System prompt.")];
        // Generate enough short messages to exceed the budget.
        for i in 0..50 {
            messages.push(Message::user(format!(
                "User message number {} with enough text.",
                i
            )));
            messages.push(Message::assistant(format!(
                "Assistant response number {} with enough text.",
                i
            )));
        }
        let original_len = messages.len();
        let compressed = compress_if_needed(&mut messages, "step-2-16k", threshold);
        assert!(compressed);
        assert!(messages.len() < original_len);
        // System + summary + at most the most recent non-system messages.
        assert!(messages.iter().any(|m| {
            m.content
                .as_ref()
                .map(|c| c.contains("Context compressed"))
                .unwrap_or(false)
        }));
        assert_eq!(messages[0].role, Role::System);
        assert!(count_session_tokens(&messages) <= token_budget(16_384, threshold));
    }

    #[test]
    fn compression_preserves_system_messages() {
        let threshold = 0.1;
        let mut messages = vec![
            Message::system("First system prompt."),
            Message::system("Second system prompt."),
            Message::user("A long user message that should be dropped.".repeat(1000)),
            Message::user("Keep me."),
        ];
        let compressed = compress_if_needed(&mut messages, "step-2-16k", threshold);
        assert!(compressed);
        assert!(
            messages.iter().filter(|m| m.role == Role::System).count() >= 2,
            "system prompts must be preserved"
        );
        assert!(count_session_tokens(&messages) <= token_budget(16_384, threshold));
    }
}
