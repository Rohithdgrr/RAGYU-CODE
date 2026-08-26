use crate::api::Message;
use crate::clock;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

const SESSION_VERSION: u32 = 2;

#[derive(Serialize, Deserialize)]
struct SessionFile {
    version: u32,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    system: String,
    messages: Vec<Message>,
}

/// Conversation state: system prompt + alternating user/assistant turns,
/// plus creation/last-save ISO-8601 timestamps.
pub struct Session {
    system: String,
    messages: Vec<Message>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

impl Session {
    pub fn new(system_prompt: impl Into<String>) -> Self {
        Self {
            system: system_prompt.into(),
            messages: Vec::new(),
            created_at: None,
            updated_at: None,
        }
    }

    pub fn system(&self) -> &str {
        &self.system
    }

    pub fn set_system(&mut self, prompt: impl Into<String>) {
        self.system = prompt.into();
    }

    pub fn push_user(&mut self, content: impl Into<String>) {
        self.messages.push(Message::user(content));
    }

    pub fn push_assistant(&mut self, content: impl Into<String>) {
        self.messages.push(Message::assistant(content));
    }

    /// Commits an assistant turn that requests tool executions. `content`
    /// holds any prose the model streamed before requesting the calls.
    pub fn push_tool_calls(
        &mut self,
        content: impl Into<String>,
        calls: Vec<crate::api::ToolCall>,
    ) {
        self.messages
            .push(Message::assistant_with_tool_calls(content, calls));
    }

    /// Commits one finished tool round atomically: the assistant prose +
    /// tool-call message, then each executed result paired to its call id.
    /// Results are `(tool_call_id, output)` pairs.
    pub fn commit_tool_round(
        &mut self,
        content: &str,
        calls: &[crate::api::ToolCall],
        results: &[(String, String)],
    ) {
        self.push_tool_calls(content, calls.to_vec());
        for (id, output) in results {
            self.push_tool_result(id.clone(), output.clone());
        }
    }

    /// Appends one executed tool result (paired to its call via id).
    pub fn push_tool_result(&mut self, tool_call_id: impl Into<String>, output: impl Into<String>) {
        self.messages.push(Message::tool(tool_call_id, output));
    }

    /// Removes the last user/assistant exchange. Returns false when empty.
    ///
    /// A tool round (assistant tool-call message + its `tool` results) moves
    /// as one atomic group, so undo never leaves a dangling half-round.
    pub fn undo(&mut self) -> bool {
        let Some(mut start) = self.last_group_start() else {
            return false;
        };
        // A plain exchange is user+assistant: drop the user prompt too.
        if start > 0 && self.messages[start - 1].role == "user" {
            start -= 1;
        }
        self.messages.truncate(start);
        true
    }

    /// Start index of the final turn-group.
    ///
    /// A group is an assistant-with-tool-calls message followed by its
    /// `tool` results; anything else is a single-message group.
    fn last_group_start(&self) -> Option<usize> {
        if self.messages.is_empty() {
            return None;
        }
        let end = self.messages.len();
        let mut start = end;
        while start > 0 && self.messages[start - 1].role == "tool" {
            start -= 1;
        }
        if start < end && start > 0 && self.messages[start - 1].has_tool_calls() {
            start -= 1;
        } else {
            // No tool round at the tail: the last message stands alone.
            return Some(end - 1);
        }
        Some(start)
    }

    /// Removes a dangling trailing user message (after a failed turn).
    pub fn pop_user(&mut self) {
        if self.messages.last().is_some_and(|m| m.role == "user") {
            self.messages.pop();
        }
    }

    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// Keeps only the first `len` messages.
    pub fn truncate_messages(&mut self, len: usize) {
        self.messages.truncate(len);
    }

    /// Replaces the entire message history. The system prompt is
    /// untouched; only the message array is replaced. Used by
    /// auto-compact's hard reset to keep the system prompt + a few
    /// recent turns.
    pub fn replace_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Returns a copy of the messages with old tool results truncated
    /// to a short excerpt + a pointer to the original. Pure: leaves
    /// `self.messages` untouched. Used by the agent loop to keep the
    /// context window lean across long tool-heavy sessions.
    pub fn messages_compressed(&self) -> Vec<Message> {
        compress_old_tool_results(&self.messages)
    }

    /// When this conversation was first saved (ISO-8601), if ever.
    pub fn created_at(&self) -> Option<&str> {
        self.created_at.as_deref()
    }

    /// When this conversation was last saved (ISO-8601), if ever.
    pub fn updated_at(&self) -> Option<&str> {
        self.updated_at.as_deref()
    }

    fn touch(&mut self) {
        let now = clock::now_iso8601();
        if self.created_at.is_none() {
            self.created_at = Some(now.clone());
        }
        self.updated_at = Some(now);
    }

    /// Context sent to the API: system prompt + the most recent turns that
    /// fit in `budget_tokens` (real tokenizer counts, framing included),
    /// always aligned so the window begins on a user turn.
    pub fn window(&self, budget_tokens: usize) -> Vec<Message> {
        self.window_with(budget_tokens, None)
    }

    /// [`Session::window`] with an extra workspace-context block (relevant
    /// file contents) folded into the system message. The injected text
    /// counts against the budget like any other content; when it alone
    /// would exceed the budget the window still opens (the newest user
    /// turn is never dropped), matching the existing over-budget policy.
    pub fn window_with(&self, budget_tokens: usize, injected: Option<&str>) -> Vec<Message> {
        self.window_with_messages(&self.messages, budget_tokens, injected)
    }

    /// Same budgeting logic as [`Self::window_with`], but operates on
    /// a caller-supplied message slice. Used by the agent loop to
    /// apply old-tool-result compression without mutating the
    /// session.
    pub fn window_with_messages(
        &self,
        messages: &[Message],
        budget_tokens: usize,
        injected: Option<&str>,
    ) -> Vec<Message> {
        let system_text = match injected.filter(|s| !s.trim().is_empty()) {
            Some(extra) => format!("{}\n\n{extra}", self.system),
            None => self.system.clone(),
        };
        let mut ctx = vec![Message::system(system_text)];
        if messages.is_empty() {
            return ctx;
        }
        let mut used = crate::tokens::count_message(&ctx[0]);
        let mut start = messages.len();
        while start > 0 {
            let cost = crate::tokens::count_message(&messages[start - 1]);
            // Always keep at least the newest message, even if it alone
            // exceeds the budget — an empty user context is worse.
            if start < messages.len() && used + cost > budget_tokens {
                break;
            }
            start -= 1;
            used += cost;
        }
        if let Some(first_user) =
            (start..messages.len()).find(|&i| messages[i].role == "user")
        {
            if first_user > start {
                start = first_user;
            }
        } else {
            return ctx;
        }
        ctx.extend_from_slice(&messages[start..]);
        ctx
    }

    /// Real token count of everything the session holds, system included.
    pub fn approx_tokens(&self) -> usize {
        crate::tokens::count_message(&Message::system(self.system.clone()))
            + self
                .messages
                .iter()
                .map(crate::tokens::count_message)
                .sum::<usize>()
    }

    /// Replaces the whole history with a single assistant summary turn,
    /// keeping the system prompt untouched. Returns the number of messages
    /// that were folded away.
    pub fn compact_with_summary(&mut self, summary: &str) -> usize {
        let removed = self.messages.len();
        self.messages = vec![Message::assistant(summary.to_owned())];
        removed
    }

    /// Finds all messages whose content contains `needle` (case-insensitive),
    /// as `(index, role, content)` triples.
    pub fn search(&self, needle: &str) -> Vec<(usize, &str, &str)> {
        let needle = needle.to_lowercase();
        self.messages
            .iter()
            .enumerate()
            .filter(|(_, m)| m.content.to_lowercase().contains(&needle))
            .map(|(i, m)| (i, m.role.as_str(), m.content.as_str()))
            .collect()
    }

    pub fn save_to(&mut self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create directory {}", parent.display()))?;
        }
        self.touch();
        let file = SessionFile {
            version: SESSION_VERSION,
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
            system: self.system.clone(),
            messages: self.messages.clone(),
        };
        let json = serde_json::to_string_pretty(&file).context("failed to serialize session")?;
        // Atomic write: write to a temp file first, then rename to avoid
        // corruption if the process is interrupted mid-write.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &json)
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("cannot rename {} to {}", tmp.display(), path.display()))
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let file: SessionFile = serde_json::from_str(&raw).context("not a valid session file")?;
        let messages = file
            .messages
            .into_iter()
            .filter(|m| matches!(m.role.as_str(), "user" | "assistant" | "tool"))
            .collect();
        Ok(Self {
            system: file.system,
            messages,
            created_at: file.created_at,
            updated_at: file.updated_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Session {
        let mut s = Session::new("sys");
        s.push_user("q1");
        s.push_assistant("a1");
        s.push_user("q2");
        s.push_assistant("a2");
        s.push_user("q3");
        s.push_assistant("a3");
        s
    }

    #[test]
    fn window_keeps_everything_when_budget_allows() {
        let s = sample();
        let w = s.window(usize::MAX);
        assert_eq!(w.len(), 7, "system + all 6 messages");
        assert_eq!(w[0].role, "system");
        assert_eq!(w.last().unwrap().content, "a3");
    }

    #[test]
    fn window_trims_oldest_first_and_stays_aligned() {
        let mut s = Session::new("s");
        s.push_user("hi");
        s.push_assistant("hello there friend");
        s.push_user("again");
        // Budget fits only system + the last user turn.
        let cost = crate::tokens::count_message(&Message::user("again"))
            + crate::tokens::count_message(&Message::system("s"));
        let w = s.window(cost);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].role, "system");
        assert_eq!(w[1], Message::user("again"));
    }

    #[test]
    fn window_never_opens_on_an_assistant_turn() {
        let mut s = Session::new("s");
        s.push_user("hi");
        s.push_assistant("a very long assistant reply that alone eats the whole budget");
        // Only room for part of the history; whatever is dropped, the window
        // must not begin with an assistant message.
        let w = s.window(crate::tokens::count_message(&Message::system("s")) + 6);
        assert!(!w.is_empty());
        assert_eq!(w[0].role, "system");
        assert!(
            w.iter().skip(1).all(|m| m.role != "assistant"),
            "window must not open on an assistant turn: {w:?}"
        );
    }

    #[test]
    fn window_always_sends_the_newest_message_even_over_budget() {
        let mut s = Session::new("s");
        s.push_user("x".repeat(500));
        let w = s.window(0);
        assert_eq!(w.len(), 2, "system + the single newest message");
        assert_eq!(w[1], Message::user("x".repeat(500)));
    }

    #[test]
    fn window_with_folds_injection_into_the_system_message() {
        let s = Session::new("sys");
        let w = s.window_with(usize::MAX, Some("--- src/api.rs ---\nfn a() {}"));
        assert_eq!(w.len(), 1);
        assert!(
            w[0].content.starts_with("sys\n\n--- src/api.rs ---"),
            "{:?}",
            w[0].content
        );
        // Empty injections leave the system prompt untouched.
        let w = s.window_with(usize::MAX, Some("   "));
        assert_eq!(w[0].content, "sys");
        let w = s.window_with(usize::MAX, None);
        assert_eq!(w[0].content, "sys");
    }

    #[test]
    fn approx_tokens_uses_the_real_tokenizer() {
        let mut s = Session::new("sys");
        s.push_user("hello world");
        let expected = crate::tokens::count_message(&Message::system("sys"))
            + crate::tokens::count_message(&Message::user("hello world"));
        assert_eq!(s.approx_tokens(), expected);
    }

    #[test]
    fn search_is_case_insensitive_and_indexed() {
        let s = sample();
        let hits = s.search("Q2");
        assert_eq!(hits, vec![(2usize, "user", "q2")]);
        assert!(s.search("nope").is_empty());
    }

    #[test]
    fn undo_drops_last_exchange() {
        let mut s = sample();
        assert!(s.undo());
        assert_eq!(s.messages().last().unwrap().content, "a2");
        s.clear();
        assert!(!s.undo());
    }

    #[test]
    fn undo_removes_whole_tool_round() {
        let mut s = sample();
        let call = crate::api::ToolCall::new("c1", "lookup", "{}");
        s.commit_tool_round("", &[call], &[("c1".to_owned(), "result".to_owned())]);
        // The tool round + the user prompt that triggered it vanish together.
        assert!(s.undo());
        assert_eq!(s.messages().len(), 6);
        assert!(s.messages().iter().all(|m| m.role != "tool"));
        assert!(!s.messages().last().unwrap().has_tool_calls());
        // Undoing again behaves like the plain case.
        assert!(s.undo());
        assert_eq!(s.messages().len(), 4);
    }

    #[test]
    fn pop_user_only_removes_trailing_user_turn() {
        let mut s = sample();
        s.pop_user(); // last is assistant -> untouched
        assert_eq!(s.messages().len(), 6);
        s.push_user("draft");
        s.pop_user();
        assert_eq!(s.messages().len(), 6);
    }

    #[test]
    fn save_load_roundtrip() {
        let mut s = sample();
        let path = std::env::temp_dir().join(format!("govinda-test-{}.json", std::process::id()));
        s.save_to(&path).expect("save");
        assert!(s.created_at().is_some());
        assert_eq!(s.created_at(), s.updated_at());

        let loaded = Session::load_from(&path).expect("load");
        assert_eq!(loaded.system(), "sys");
        assert_eq!(loaded.messages(), s.messages());
        assert_eq!(loaded.created_at(), s.created_at());
        assert_eq!(loaded.updated_at(), s.updated_at());

        // A later save refreshes updated_at but preserves created_at.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        s.save_to(&path).expect("resave");
        assert_ne!(s.created_at(), s.updated_at());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn legacy_files_without_timestamps_load() {
        let path =
            std::env::temp_dir().join(format!("govinda-test-legacy-{}.json", std::process::id()));
        std::fs::write(
            &path,
            r#"{"version":1,"system":"s","messages":[{"role":"user","content":"u"}]}"#,
        )
        .unwrap();
        let loaded = Session::load_from(&path).expect("load");
        assert!(loaded.created_at().is_none());
        assert!(loaded.updated_at().is_none());
        // First re-save stamps created_at.
        let mut loaded = loaded;
        loaded.save_to(&path).unwrap();
        assert!(loaded.created_at().is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_filters_foreign_roles() {
        let path =
            std::env::temp_dir().join(format!("govinda-test-roles-{}.json", std::process::id()));
        std::fs::write(
            &path,
            r#"{"version":2,"system":"s","messages":[
                {"role":"user","content":"u"},
                {"role":"tool","content":"t","tool_call_id":"c1"},
                {"role":"assistant","content":"a"},
                {"role":"function","content":"legacy"}
            ]}"#,
        )
        .unwrap();
        let loaded = Session::load_from(&path).expect("load");
        assert_eq!(loaded.messages().len(), 3, "user/tool/assistant kept");
        assert_eq!(loaded.messages()[1].tool_call_id.as_deref(), Some("c1"));
        let _ = std::fs::remove_file(&path);
    }
}

/// Tool results older than the most recent `KEEP_RECENT_TOOL_ROUNDS`
/// rounds are truncated to this many characters before being sent to
/// the model. Old tool results are usually re-readable on disk; the
/// model needs only enough context to know they happened.
pub const COMPRESSED_TOOL_CHARS: usize = 200;
const KEEP_RECENT_TOOL_ROUNDS: usize = 3;

/// Truncates old tool-result messages to a short excerpt + a pointer
/// to the original. Pure function: easy to unit-test.
pub fn compress_old_tool_results(messages: &[Message]) -> Vec<Message> {
    let tool_indices: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter_map(|(i, m)| if m.role == "tool" { Some(i) } else { None })
        .collect();
    if tool_indices.len() <= KEEP_RECENT_TOOL_ROUNDS {
        return messages.to_vec();
    }
    // The boundary is the start of the most recent
    // KEEP_RECENT_TOOL_ROUNDS tool messages. Everything before it
    // (older tool results) gets truncated; everything from it on is
    // untouched.
    let boundary_idx = tool_indices[tool_indices.len() - KEEP_RECENT_TOOL_ROUNDS];
    let mut out: Vec<Message> = Vec::with_capacity(messages.len());
    for m in &messages[..boundary_idx] {
        if m.role == "tool" && m.content.chars().count() > COMPRESSED_TOOL_CHARS {
            let excerpt: String = m.content.chars().take(COMPRESSED_TOOL_CHARS).collect();
            let mut tm = m.clone();
            tm.content = format!(
                "{excerpt}…\n// truncated, tool_call_id={:?}",
                m.tool_call_id
            );
            out.push(tm);
        } else {
            out.push(m.clone());
        }
    }
    out.extend_from_slice(&messages[boundary_idx..]);
    out
}

#[cfg(test)]
mod compress_tests {
    use super::*;

    fn tool_msg(id: &str, content: &str) -> Message {
        Message::tool(id, content)
    }

    #[test]
    fn keeps_short_history_untouched() {
        let msgs = vec![
            Message::user("u1"),
            tool_msg("c1", "short"),
            Message::assistant("a1"),
        ];
        let out = compress_old_tool_results(&msgs);
        assert_eq!(out.len(), msgs.len());
        assert_eq!(out[1].content, "short");
    }

    #[test]
    fn truncates_old_tool_results() {
        let big = "x".repeat(COMPRESSED_TOOL_CHARS * 3);
        let msgs = vec![
            tool_msg("c1", &big),
            Message::assistant("a1"),
            tool_msg("c2", &big),
            Message::assistant("a2"),
            tool_msg("c3", &big),
            Message::assistant("a3"),
            tool_msg("c4", "recent small"),
        ];
        let out = compress_old_tool_results(&msgs);
        // Oldest tool messages truncated; the most recent 3 tool
        // messages kept verbatim.
        assert!(out[0].content.contains("truncated"), "c1 should be truncated");
        assert_eq!(out[2].content, big, "c2 should be untouched (recent)");
        assert_eq!(out[4].content, big, "c3 should be untouched (recent)");
        assert_eq!(out[6].content, "recent small", "c4 should be untouched");
    }
}
