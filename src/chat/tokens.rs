//! Simple token counting utilities.
//!
//! Uses the `cl100k_base` tokenizer (GPT-4 / Claude / many modern models)
//! as a reasonable approximation for StepFun models.

use tiktoken_rs::cl100k_base;

/// Count tokens in a string using the cl100k_base tokenizer.
pub fn count_tokens(text: &str) -> usize {
    let bpe = cl100k_base().unwrap();
    bpe.encode_with_special_tokens(text).len()
}

/// Count tokens across multiple strings.
pub fn count_tokens_many(texts: &[&str]) -> usize {
    let mut total = 0;
    let bpe = cl100k_base().unwrap();
    for text in texts {
        total += bpe.encode_with_special_tokens(text).len();
    }
    total
}
