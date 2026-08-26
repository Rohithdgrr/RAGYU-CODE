use crate::api::Message;
use std::sync::OnceLock;
use tiktoken_rs::CoreBPE;

/// Per-message wire overhead (role/name framing) added to content tokens,
/// matching the convention OpenAI documents for chat messages.
const PER_MESSAGE_OVERHEAD: usize = 4;
/// Extra framing per streamed tool call inside an assistant message.
const PER_TOOL_CALL_OVERHEAD: usize = 4;

static BPE: OnceLock<Option<CoreBPE>> = OnceLock::new();

fn bpe() -> Option<&'static CoreBPE> {
    BPE.get_or_init(|| tiktoken_rs::cl100k_base().ok()).as_ref()
}

/// Counts tokens with a real BPE tokenizer (cl100k, shared by most modern
/// chat APIs). If the vocabulary fails to load — essentially never — callers
/// transparently fall back to a chars÷4 estimate rather than panicking.
pub fn count(text: &str) -> usize {
    match bpe() {
        Some(b) => b.encode_ordinary(text).len(),
        None => text.chars().count().div_ceil(4),
    }
}

/// Token cost of sending one message over the wire, framing included.
pub fn count_message(msg: &Message) -> usize {
    let mut tokens = count(&msg.content) + PER_MESSAGE_OVERHEAD;
    for call in msg.tool_calls.iter().flatten() {
        tokens += PER_TOOL_CALL_OVERHEAD + count(&call.id) + count(&call.function.name);
        // Arguments are JSON text on the wire; count them verbatim.
        tokens += count(&call.function.arguments);
    }
    tokens
}

/// Returns a human-readable summary of the current token budget for the
/// `show_token_budget` tool.
pub fn budget_summary() -> anyhow::Result<String> {
    // We can't reach the live App from here without a global, so we read
    // what's available in the process environment and fall back to sensible
    // defaults. The exact value is informational; the model uses it to
    // decide whether to compact.
    let used = std::env::var("GOVINDA_USED_TOKENS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    let budget = std::env::var("GOVINDA_CONTEXT_TOKENS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(crate::provider::DEFAULT_CONTEXT_TOKENS);
    let model_limit = std::env::var("GOVINDA_MODEL_LIMIT")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(budget);
    let headroom = budget.saturating_sub(used);
    let pct = if budget > 0 { (used * 100) / budget } else { 0 };
    Ok(format!(
        "used_tokens: {used}\nbudget_tokens: {budget}\nmodel_limit_tokens: {model_limit}\nheadroom_tokens: {headroom}\npercent_used: {pct}%"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_real_tokens_not_chars() {
        // Known cl100k encodings: "hello world" = 2 tokens.
        assert_eq!(count("hello world"), 2);
        assert_eq!(count("a"), 1);
        let text = "the quick brown fox jumps over the lazy dog";
        let n = count(text);
        assert!(
            (6..=12).contains(&n),
            "expected ~10 real tokens for '{text}', got {n}"
        );
    }

    #[test]
    fn empty_text_is_free() {
        assert_eq!(count(""), 0);
    }

    #[test]
    fn message_cost_includes_framing_overhead() {
        let m = Message::user("");
        assert_eq!(count_message(&m), PER_MESSAGE_OVERHEAD);
    }

    #[test]
    fn longer_text_costs_more() {
        assert!(count("a") < count("hello world how are you"));
    }

    #[test]
    fn tool_call_messages_cost_name_and_arguments() {
        let plain = count_message(&Message::assistant(""));
        let mut with_calls = Message::assistant("");
        with_calls.tool_calls = Some(vec![crate::api::ToolCall::new(
            "call_abc123",
            "weather",
            r#"{"city":"Paris"}"#,
        )]);
        let cost = count_message(&with_calls);
        assert!(cost > plain + PER_TOOL_CALL_OVERHEAD);
        // Deterministic: id + name + arguments all counted.
        let expected = plain
            + PER_TOOL_CALL_OVERHEAD
            + count("call_abc123")
            + count("weather")
            + count(r#"{"city":"Paris"}"#);
        assert_eq!(cost, expected);
    }
}
