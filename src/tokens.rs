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
