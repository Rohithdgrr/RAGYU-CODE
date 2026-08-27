//! Chat pane widget: renders the conversation transcript.
//!
//! Each `ChatEntry` becomes a header line plus wrapped body lines. Code
//! fences in assistant messages get a tinted background block with the
//! language label; tool calls render as compact status lines. The pane
//! auto-follows the bottom until the user scrolls up.
//!
//! Markdown (TUI):
//! - headings `#`-`###`, hr `---`, tables `| a | b |`, bullet/ordered lists,
//!   blockquotes `>`, `**bold**`, `*italic*`, `` `code` ``, links.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::super::{icons, theme};

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

// ---------------------------------------------------------------------------
// Build-lines cache (perf): `build_lines` re-parses every entry each frame
// (markdown fences, word-wrap, inline spans). For a long session (600 entries
// × ~20 lines each) that is ~12k lines re-parsed at 60 fps. The cache below
// memoizes the last result keyed by a hash of `entries` + frame params
// (`width`, `raw`, `streaming`, `busy`). A hit avoids the full re-parse and
// just clones the cached `Vec<Line>` (cheap). Invalidation is automatic:
// any new/changed entry changes the hash.
//
// This is a simple LRU of size 1 — sufficient because the TUI renders the
// same transcript every frame until a new turn arrives. For future scale,
// consider an LRU keyed by entry range or a dirty-flag per entry.
// ---------------------------------------------------------------------------
thread_local! {
    static BUILD_CACHE: RefCell<Option<CachedLines>> = const { RefCell::new(None) };
}

struct CachedLines {
    hash: u64,
    width: u16,
    raw: bool,
    busy: bool,
    streaming_hash: u64,
    lines: Vec<Line<'static>>,
}

fn hash_entries(entries: &[ChatEntry], streaming: Option<&str>, busy: bool) -> (u64, u64) {
    let mut h = DefaultHasher::new();
    // Hash entry count and each entry's discriminant + content length + bytes.
    // This is O(total chars) but hashing is ~10× cheaper than markdown parsing
    // + word-wrap, so the cache still wins by a large margin.
    entries.len().hash(&mut h);
    for e in entries {
        match e {
            ChatEntry::User(s) => {
                0u8.hash(&mut h);
                s.hash(&mut h);
            }
            ChatEntry::Assistant(s) => {
                1u8.hash(&mut h);
                s.hash(&mut h);
            }
            ChatEntry::Tool { name, args, ok } => {
                2u8.hash(&mut h);
                name.hash(&mut h);
                args.hash(&mut h);
                ok.hash(&mut h);
            }
            ChatEntry::Op(s) => {
                3u8.hash(&mut h);
                s.hash(&mut h);
            }
            ChatEntry::Shell { cmd, output, ok } => {
                4u8.hash(&mut h);
                cmd.hash(&mut h);
                output.hash(&mut h);
                ok.hash(&mut h);
            }
            ChatEntry::Code { lang, code } => {
                5u8.hash(&mut h);
                lang.hash(&mut h);
                code.hash(&mut h);
            }
            ChatEntry::Checklist { title, steps } => {
                6u8.hash(&mut h);
                title.hash(&mut h);
                for (s, d) in steps {
                    s.hash(&mut h);
                    d.hash(&mut h);
                }
            }
            ChatEntry::Notice(s) => {
                7u8.hash(&mut h);
                s.hash(&mut h);
            }
            ChatEntry::Error(e) => {
                8u8.hash(&mut h);
                (e.severity as u8).hash(&mut h);
                e.title.hash(&mut h);
                e.detail.hash(&mut h);
                e.suggestion.hash(&mut h);
            }
        }
    }
    busy.hash(&mut h);
    let hash = h.finish();
    // Separate hash for the streaming buffer (Option<&str>) so a live delta
    // invalidates the cache even when `entries` is unchanged.
    let mut hs = DefaultHasher::new();
    streaming.hash(&mut hs);
    (hash, hs.finish())
}

/// Severity level for structured error display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorSeverity {
    Info,
    Warn,
    Error,
    Critical,
}

/// A structured error entry with context, severity, and actionable suggestions.
#[derive(Debug, Clone)]
pub struct ErrorEntry {
    pub severity: ErrorSeverity,
    pub title: String,
    pub detail: String,
    pub suggestion: Option<String>,
}

pub enum ChatEntry {
    /// A submitted user prompt (slash commands handled locally never land
    /// here).
    User(String),
    /// A completed assistant answer (prose only — tool rounds commit their
    /// prose as part of Tool entries).
    Assistant(String),
    /// One executed (or declined) tool call.
    Tool {
        name: String,
        args: String,
        /// `None` while pending/running, otherwise pass/fail.
        ok: Option<bool>,
    },
    /// Agent operation divider — like "AGENT START — read_file → grep".
    Op(String),
    /// Shell / execution output with dark terminal styling.
    Shell {
        cmd: String,
        output: String,
        ok: bool,
    },
    /// Explicit code block outside markdown (lang + body).
    Code { lang: String, code: String },
    /// Plan checklist: steps with done flags, rendered with a progress bar.
    Checklist {
        title: String,
        steps: Vec<(String, bool)>,
    },
    /// Local system notices (errors, hints, command feedback).
    Notice(String),
    /// Structured error with severity, context, and suggestions.
    Error(ErrorEntry),
}

/// One ``` fence segment inside assistant content.
enum Segment<'a> {
    Text(&'a str),
    Code {
        lang: Option<&'a str>,
        body: &'a str,
    },
}

#[allow(dead_code)]
fn segment_kind(s: &Segment<'_>) -> &'static str {
    match s {
        Segment::Text(_) => "text",
        Segment::Code { .. } => "code",
    }
}

fn split_fences(content: &str) -> Vec<Segment<'_>> {
    let mut segments = Vec::new();
    let mut rest = content;
    while let Some(pos) = rest.find("```") {
        let (before, after) = rest.split_at(pos);
        if !before.trim().is_empty() || !segments.is_empty() {
            segments.push(Segment::Text(before));
        }
        let after = &after[3..];
        match after.find("```") {
            Some(end) => {
                let (lang, body) = after[..end]
                    .split_once('\n')
                    .unwrap_or(("", after[..end].trim_end()));
                segments.push(Segment::Code {
                    lang: if lang.trim().is_empty() {
                        None
                    } else {
                        Some(lang)
                    },
                    body,
                });
                rest = &after[end + 3..];
            }
            None => {
                // Unterminated fence: render what we have as code.
                let (lang, body) = after.split_once('\n').unwrap_or(("", after));
                segments.push(Segment::Code {
                    lang: if lang.trim().is_empty() {
                        None
                    } else {
                        Some(lang)
                    },
                    body,
                });
                rest = "";
            }
        }
    }
    if !rest.is_empty() {
        segments.push(Segment::Text(rest));
    }
    segments
}

/// Greedy word-wrap on whitespace, width measured in terminal columns so
/// wide glyphs don't overflow. Words longer than a full line are hard-broken.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    fn col_width(s: &str) -> usize {
        UnicodeWidthStr::width(s)
    }

    let width = width.max(4);
    let mut out = Vec::new();
    for para in text.lines() {
        if para.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut line = String::new();
        let mut line_w = 0usize;
        for word in para.split_whitespace() {
            let mut chunk = word;
            loop {
                let sep = usize::from(!line.is_empty());
                if line_w + sep + col_width(chunk) <= width {
                    if sep == 1 {
                        line.push(' ');
                    }
                    line.push_str(chunk);
                    line_w += sep + col_width(chunk);
                    break;
                }
                if line.is_empty() && col_width(chunk) > width {
                    // Hard-break: take as many columns as fit.
                    let mut cut = String::new();
                    let mut taken = 0usize;
                    let mut split_at = chunk.len();
                    for (i, c) in chunk.char_indices() {
                        let cw = c.width().unwrap_or(1).max(1);
                        if taken + cw > width {
                            split_at = i;
                            break;
                        }
                        cut.push(c);
                        taken += cw;
                    }
                    out.push(cut);
                    chunk = &chunk[split_at..];
                } else {
                    out.push(std::mem::take(&mut line));
                    line_w = 0;
                }
            }
        }
        out.push(line);
    }
    out
}

// ── inline markdown ──────────────────────────────────────────────────────
//
// Perf note: `inline_spans` is O(n) in the length of `raw`, not O(n²).
// Earlier versions of this function recursively called `inline_spans` for
// nested bold (`**a **b** c**`) which could double-scan. The current code
// still recurses for `**bold**` inner content, but only on the *inner*
// slice (strictly shorter than `raw`), so the total work is O(n) with a
// single outer scan plus one inner scan per bold segment. The fast path
// below and the early-exit for empty `inner` prevent quadratic blowup.

fn inline_spans(raw: &str, base: Style) -> Vec<Span<'static>> {
    // Fast path: no markup chars or very short input — avoids allocating
    // the `chars` vector and scanning. Common case for tool args / notices.
    if raw.is_empty() {
        return vec![Span::styled(String::new(), base)];
    }
    if !raw.contains('`')
        && !raw.contains('*')
        && !raw.contains('_')
        && !raw.contains('[')
        && !raw.contains('~')
        && !raw.contains('=')
    {
        return vec![Span::styled(raw.to_owned(), base)];
    }
    let mut out: Vec<Span<'static>> = Vec::new();
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0usize;
    let len = chars.len();
    let t = theme::active();
    // Enhanced styles derived from base
    let bold = base.add_modifier(Modifier::BOLD);
    let italic = base.add_modifier(Modifier::ITALIC);
    let bold_italic = base.add_modifier(Modifier::BOLD | Modifier::ITALIC);
    let code_style = Style::default()
        .fg(t.syntax_string)
        .bg(t.bg_secondary)
        .add_modifier(Modifier::BOLD);
    let strike = base.add_modifier(Modifier::CROSSED_OUT);
    let link_style = Style::default()
        .fg(t.accent_primary)
        .add_modifier(Modifier::UNDERLINED | Modifier::BOLD);
    let highlight_style = Style::default()
        .fg(t.text_inverse)
        .bg(t.accent_secondary)
        .add_modifier(Modifier::BOLD);

    while i < len {
        // ==highlight== syntax for important text
        if i + 1 < len && chars[i] == '=' && chars[i + 1] == '=' {
            let mut found: Option<usize> = None;
            let mut k = i + 2;
            while k + 1 < len {
                if chars[k] == '=' && chars[k + 1] == '=' {
                    found = Some(k);
                    break;
                }
                k += 1;
            }
            if let Some(end) = found {
                let inner: String = chars[i + 2..end].iter().collect();
                if !inner.is_empty() {
                    out.push(Span::styled(inner, highlight_style));
                    i = end + 2;
                    continue;
                }
            }
        }
        // ``` inline code ```
        if chars[i] == '`' {
            // count run of backticks (1 or 2)
            let j = i + 1;
            if let Some(end) = (j..len).position(|k| chars[k] == '`').map(|p| j + p) {
                let inner: String = chars[j..end].iter().collect();
                if !inner.is_empty() {
                    out.push(Span::styled(" ".to_owned(), base));
                    out.push(Span::styled(format!(" {} ", inner), code_style));
                    out.push(Span::styled(" ".to_owned(), base));
                    i = end + 1;
                    continue;
                }
            }
        }
        // bold **text** or __text__
        if i + 1 < len
            && ((chars[i] == '*' && chars[i + 1] == '*')
                || (chars[i] == '_' && chars[i + 1] == '_'))
        {
            let marker = chars[i];
            // find closing same pair
            let mut found: Option<usize> = None;
            let mut k = i + 2;
            while k + 1 < len {
                if chars[k] == marker && chars[k + 1] == marker {
                    found = Some(k);
                    break;
                }
                k += 1;
            }
            if let Some(end) = found {
                let inner: String = chars[i + 2..end].iter().collect();
                if !inner.trim().is_empty() {
                    let inner_spans = inline_spans(&inner, bold);
                    // flatten without extra allocations: push inner spans with bold merged
                    for s in inner_spans {
                        // if inner already has code spans, preserve those; otherwise bold
                        let is_code = s.style.bg == Some(t.bg_secondary)
                            && s.style.fg == Some(t.syntax_string);
                        if is_code {
                            out.push(s);
                        } else {
                            out.push(Span::styled(
                                s.content.into_owned(),
                                s.style.add_modifier(Modifier::BOLD),
                            ));
                        }
                    }
                    i = end + 2;
                    continue;
                }
            }
        }
        // strikethrough ~~text~~
        if i + 1 < len && chars[i] == '~' && chars[i + 1] == '~' {
            let mut found: Option<usize> = None;
            let mut k = i + 2;
            while k + 1 < len {
                if chars[k] == '~' && chars[k + 1] == '~' {
                    found = Some(k);
                    break;
                }
                k += 1;
            }
            if let Some(end) = found {
                let inner: String = chars[i + 2..end].iter().collect();
                if !inner.is_empty() {
                    out.push(Span::styled(inner, strike));
                    i = end + 2;
                    continue;
                }
            }
        }
        // link [text](url)
        if chars[i] == '['
            && let Some(close_bracket) = (i + 1..len)
                .position(|k| chars[k] == ']')
                .map(|p| i + 1 + p)
            && close_bracket + 1 < len
            && chars[close_bracket + 1] == '('
            && let Some(close_paren) = (close_bracket + 2..len)
                .position(|k| chars[k] == ')')
                .map(|p| close_bracket + 2 + p)
        {
            let label: String = chars[i + 1..close_bracket].iter().collect();
            let url: String = chars[close_bracket + 2..close_paren].iter().collect();
            if !label.is_empty() {
                out.push(Span::styled(label, link_style));
                // show url muted in parentheses if not too long
                if !url.is_empty() && url.len() < 60 {
                    out.push(Span::styled(
                        format!(" ({})", url),
                        Style::default()
                            .fg(t.text_muted)
                            .add_modifier(Modifier::DIM),
                    ));
                }
                i = close_paren + 1;
                continue;
            }
        }
        // italic *text* or _text_  (single)
        if (chars[i] == '*' || chars[i] == '_') && !(i + 1 < len && chars[i + 1] == chars[i]) {
            let marker = chars[i];
            // avoid matching list marker "* " at line start — caller handles lists separately;
            // here we still allow inline italic mid-sentence
            let mut found: Option<usize> = None;
            let mut k = i + 1;
            while k < len {
                if chars[k] == marker {
                    // ensure not part of ** already handled and not empty
                    if k > i + 1 {
                        found = Some(k);
                        break;
                    }
                }
                k += 1;
            }
            if let Some(end) = found {
                let inner: String = chars[i + 1..end].iter().collect();
                if !inner.trim().is_empty() && !inner.contains(' ') || inner.len() < 80 {
                    // heuristic: avoid catching stray underscores in words
                    let is_word = marker == '_' && (i > 0 && chars[i - 1].is_alphanumeric())
                        || (end + 1 < len && chars[end + 1].is_alphanumeric());
                    if !is_word {
                        out.push(Span::styled(inner, italic));
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        // plain char — coalesce
        let start = i;
        i += 1;
        while i < len {
            let c = chars[i];
            if c == '`' || c == '*' || c == '_' || c == '[' || c == '~' || c == '=' {
                break;
            }
            i += 1;
        }
        let chunk: String = chars[start..i].iter().collect();
        // preserve prior style for bold context
        out.push(Span::styled(chunk, base));
        // plain coalesce always advances i by at least 1, so no infinite loop
        // even for single unclosed markers like `*unclosed`
    }
    // merge consecutive spans with same style to keep rendering cheap
    let mut merged: Vec<Span<'static>> = Vec::new();
    for s in out {
        if let Some(last) = merged.last_mut()
            && last.style == s.style
        {
            let mut combined = last.content.clone().into_owned();
            combined.push_str(&s.content);
            last.content = combined.into();
            continue;
        }
        merged.push(s);
    }
    // handle combined bold+italic preservation for bold caller
    if base.add_modifier(Modifier::BOLD) == bold && base.add_modifier(Modifier::ITALIC) != italic {
        // caller passed bold — ensure inner italic keeps bold too (handled above)
    }
    let _ = bold_italic;
    merged
}

fn is_hr(line: &str) -> bool {
    let t = line.trim();
    if t.len() < 3 {
        return false;
    }
    let mut chars = t.chars().filter(|c| !c.is_whitespace());
    let first = match chars.next() {
        Some(c) => c,
        None => return false,
    };
    if !matches!(first, '-' | '*' | '_') {
        return false;
    }
    let mut count = 1;
    for c in chars {
        if c != first {
            return false;
        }
        count += 1;
    }
    count >= 3
}

fn heading_level(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start();
    let mut level = 0usize;
    for c in trimmed.chars() {
        if c == '#' {
            level += 1;
        } else {
            break;
        }
    }
    if level == 0 || level > 6 {
        return None;
    }
    let rest = trimmed[level..].trim_start();
    // require space after #s or treat as not heading if missing (GFM permissive: allow without space)
    if rest.is_empty() {
        return None;
    }
    Some((level, rest.to_owned()))
}

fn is_separator_row(line: &str) -> bool {
    let t = line.trim();
    if !t.contains('|') {
        return false;
    }
    // remove pipes and spaces, should be only - : |
    let stripped: String = t
        .chars()
        .filter(|c| *c != '|' && !c.is_whitespace())
        .collect();
    if stripped.is_empty() {
        return false;
    }
    stripped.chars().all(|c| matches!(c, '-' | ':'))
        && t.matches('|').count() >= 1
        && stripped.len() >= 3
}

fn is_table_row(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|') && t.ends_with('|') && t.contains('|')
}

fn split_table_row(line: &str) -> Vec<String> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(|c| c.trim().to_owned())
        .collect()
}

// ── block helpers ──────────────────────────────────────────────────────

fn hr_line(inner: usize) -> Line<'static> {
    let t = theme::active();
    let w = inner.min(64).max(10);
    // Enhanced horizontal rule with better visual appeal
    Line::styled(
        format!("  {}{}{}", "─".repeat(w / 3), "┄", "─".repeat(w / 3)),
        Style::default().fg(t.border_default).bg(t.bg_primary),
    )
}

fn heading_lines(level: usize, raw: &str, inner: usize) -> Vec<Line<'static>> {
    let t = theme::active();
    let (fg, prefix, underline) = match level {
        1 => (t.accent_primary, "━ ", true),
        2 => (t.accent_primary, "╺ ", false),
        3 => (t.accent_secondary, "▸ ", false),
        _ => (t.text_primary, "· ", false),
    };
    let base = Style::default()
        .fg(fg)
        .bg(t.bg_primary)
        .add_modifier(Modifier::BOLD);
    let mut out = Vec::new();
    // allow inline formatting inside heading
    let content = raw.trim();
    // strip surrounding ** if present (common LLM pattern: "### **Title**")
    let content = if content.starts_with("**") && content.ends_with("**") && content.len() >= 4 {
        &content[2..content.len() - 2]
    } else {
        content
    };
    let mut wrapped = wrap_plain_for_spans(content, inner - 4);
    if wrapped.is_empty() {
        wrapped.push(String::new());
    }
    // first line with prefix
    for (idx, w) in wrapped.into_iter().enumerate() {
        let line_spans = if idx == 0 {
            let mut v = vec![
                Span::styled("  ", base),
                Span::styled(
                    prefix,
                    Style::default()
                        .fg(fg)
                        .bg(t.bg_primary)
                        .add_modifier(Modifier::BOLD),
                ),
            ];
            // re-parse this chunk's slice with inline (approx)
            let chunk_spans = inline_spans(&w, base);
            v.extend(chunk_spans);
            v
        } else {
            let mut v = vec![Span::styled("    ", base)];
            v.extend(inline_spans(&w, base));
            v
        };
        out.push(Line::from(line_spans));
    }
    if underline {
        out.push(Line::styled(
            format!("  {}", "━".repeat(inner.min(40))),
            Style::default().fg(t.border_default).bg(t.bg_primary),
        ));
    }
    // keep heading isolated with breathing room? caller adds blank
    out
}

fn wrap_plain_for_spans(text: &str, width: usize) -> Vec<String> {
    wrap(text, width)
}

fn paragraph_lines(text: &str, inner: usize) -> Vec<Line<'static>> {
    let t = theme::active();
    let base = t.text();
    // collapse internal newlines/extra spaces into single paragraph stream
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.is_empty() {
        return vec![];
    }
    let mut out = Vec::new();
    for chunk in wrap(&joined, inner) {
        let spans = inline_spans(&chunk, base);
        let mut line_spans = vec![Span::raw("  ")];
        line_spans.extend(spans);
        out.push(Line::from(line_spans));
    }
    out
}

fn blockquote_lines(raw: &str, inner: usize) -> Vec<Line<'static>> {
    let t = theme::active();
    let base = Style::default()
        .fg(t.text_secondary)
        .bg(t.bg_primary)
        .add_modifier(Modifier::ITALIC);
    let content = raw
        .trim_start_matches('>')
        .trim_start()
        .trim_start_matches('>')
        .trim();
    let mut out = Vec::new();
    let bar = Style::default()
        .fg(t.accent_secondary)
        .bg(t.bg_primary)
        .add_modifier(Modifier::BOLD);
    // Add quote icon at the start
    out.push(Line::from(vec![
        Span::styled("  ", base),
        Span::styled(
            format!(" {} ", icons::QUOTE),
            Style::default()
                .fg(t.accent_secondary)
                .bg(t.bg_primary)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    for chunk in wrap(content, inner - 4) {
        let mut spans = vec![Span::styled("  ", base), Span::styled("▎ ", bar)];
        spans.extend(inline_spans(&chunk, base));
        out.push(Line::from(spans));
    }
    out
}

#[allow(clippy::unwrap_used)] // safe: all calls guarded by len() checks
fn list_block_lines(lines: &[&str], inner: usize) -> (Vec<Line<'static>>, usize) {
    let t = theme::active();
    let mut out = Vec::new();
    let mut consumed = 0usize;
    let bullet_style = Style::default()
        .fg(t.accent_secondary)
        .bg(t.bg_primary)
        .add_modifier(Modifier::BOLD);
    let text_style = t.text();
    // Add list icon at the start
    out.push(Line::from(vec![
        Span::styled("  ", text_style),
        Span::styled(
            format!(" {} ", icons::LIST),
            Style::default()
                .fg(t.accent_secondary)
                .bg(t.bg_primary)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i];
        let trimmed = raw.trim_start();
        if trimmed.is_empty() {
            break;
        }
        let (is_list, marker_len, _ordered) =
            if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("· ")
            {
                (true, 2, false)
            } else if trimmed.starts_with("+ ") {
                (true, 2, false)
            } else if trimmed.len() >= 3
                && trimmed.chars().next().unwrap().is_ascii_digit()
                && trimmed.chars().nth(1) == Some('.')
                && trimmed.chars().nth(2) == Some(' ')
            {
                (true, 3, true)
            } else if trimmed.starts_with("  - ") || trimmed.starts_with("  * ") {
                (true, 2, false)
            } else {
                (false, 0, false)
            };
        if !is_list {
            break;
        }
        // extract content after marker (handle nested indent)
        let content = trimmed[marker_len..].trim_start();
        // handle continued lines that are indented (>=2 spaces) as part of same item
        let mut full = content.to_owned();
        let mut look = i + 1;
        while look < lines.len() {
            let nxt = lines[look];
            if nxt.starts_with("  ")
                && !nxt.trim().is_empty()
                && !is_hr(nxt)
                && heading_level(nxt).is_none()
                && !is_table_row(nxt)
            {
                // continuation
                full.push(' ');
                full.push_str(nxt.trim());
                look += 1;
            } else {
                break;
            }
        }
        let bullet = if _ordered {
            // extract number
            let num = trimmed.chars().next().unwrap();
            format!(" {num}. ")
        } else {
            " • ".to_owned()
        };
        for (widx, chunk) in wrap(&full, inner - 4).into_iter().enumerate() {
            let mut spans = Vec::new();
            if widx == 0 {
                spans.push(Span::styled(" ", text_style));
                spans.push(Span::styled(bullet.clone(), bullet_style));
            } else {
                spans.push(Span::styled("    ", text_style));
            }
            spans.extend(inline_spans(&chunk, text_style));
            out.push(Line::from(spans));
        }
        i = if look > i + 1 { look } else { i + 1 };
        consumed = i;
    }
    (out, consumed)
}

fn table_lines(raw_lines: &[&str], inner: usize) -> (Vec<Line<'static>>, usize) {
    let t = theme::active();
    // must have at least header + separator
    if raw_lines.len() < 2 {
        return (vec![], 0);
    }
    if !is_table_row(raw_lines[0]) || !is_separator_row(raw_lines[1]) {
        return (vec![], 0);
    }
    let header = split_table_row(raw_lines[0]);
    let cols = header.len().max(1);
    // collect body rows while they look like table rows
    let mut rows: Vec<Vec<String>> = Vec::new();
    rows.push(header);
    let mut consumed = 2; // header+sep
    for &line in &raw_lines[2..] {
        if is_table_row(line) {
            let mut cells = split_table_row(line);
            // pad to cols
            while cells.len() < cols {
                cells.push(String::new());
            }
            cells.truncate(cols);
            rows.push(cells);
            consumed += 1;
        } else if line.trim().is_empty() {
            break;
        } else {
            break;
        }
    }
    if rows.is_empty() {
        return (vec![], 0);
    }
    // compute column widths: distribute inner- (borders) across cols
    // total width needed = sum(col_width)+ cols+1 + 2*cols padding (space both sides)
    // inner includes "  " prefix (2), so usable is inner-2
    let usable = inner.saturating_sub(2);
    let borders = cols + 1;
    let padding = cols * 2; // one space left/right per cell
    let mut col_widths: Vec<usize> = vec![0; cols];
    for r in &rows {
        for (ci, cell) in r.iter().enumerate().take(cols) {
            let plain = strip_inline_markers(cell);
            let w = UnicodeWidthStr::width(plain.as_str());
            col_widths[ci] = col_widths[ci].max(w);
        }
    }
    // cap total
    let total_content: usize = col_widths.iter().sum();
    let needed = total_content + borders + padding;
    if needed > usable {
        // proportionally shrink
        let mut excess = needed - usable;
        // shrink largest columns first
        while excess > 0 {
            if let Some((idx, _)) = col_widths.iter().enumerate().max_by_key(|(_, w)| *w) {
                if col_widths[idx] > 4 {
                    col_widths[idx] -= 1;
                    excess -= 1;
                } else {
                    break;
                }
            } else {
                break;
            }
        }
    } else if needed < usable && usable - needed < 20 {
        // distribute extra a bit
        let extra = (usable - needed) / cols;
        for w in &mut col_widths {
            *w += extra;
        }
    }
    // helpers to build border lines
    let mut out: Vec<Line<'static>> = Vec::new();
    let border_style = Style::default().fg(t.border_default).bg(t.bg_primary);
    let header_bg = t.bg_secondary;
    let header_style = Style::default()
        .fg(t.text_primary)
        .bg(header_bg)
        .add_modifier(Modifier::BOLD);
    let cell_style = Style::default().fg(t.text_primary).bg(t.bg_primary);
    let alt_style = Style::default().fg(t.text_primary).bg(t.bg_primary);

    let build_border = |left: &str, mid: &str, right: &str| -> String {
        let mut s = String::from("  ");
        s.push_str(left);
        for (ci, w) in col_widths.iter().enumerate() {
            s.push_str(&"═".repeat(*w + 2)); // Use double lines for better visual
            if ci + 1 < cols {
                s.push_str(mid);
            } else {
                s.push_str(right);
            }
        }
        s
    };
    // Enhanced top border with rounded corners
    out.push(Line::styled(build_border("╔", "╦", "╗"), border_style));
    for (ri, row) in rows.iter().enumerate() {
        let is_header = ri == 0;
        let row_style = if is_header {
            header_style
        } else if ri % 2 == 0 {
            cell_style
        } else {
            alt_style
        };
        // build cell line with inline spans per cell
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::styled("  ", border_style));
        spans.push(Span::styled("║", border_style)); // Use double lines
        for (ci, cell) in row.iter().enumerate().take(cols) {
            spans.push(Span::styled(" ", row_style));
            // truncate/pad cell plain then apply inline styling (approx: style whole cell)
            let plain = strip_inline_markers(cell);
            let display = truncate_to_width(&plain, col_widths[ci]);
            let pad = col_widths[ci].saturating_sub(UnicodeWidthStr::width(display.as_str()));
            // use inline spans for this cell's raw content (so **bold** inside cell works)
            let cell_spans = inline_spans(cell, row_style);
            // we need to render cell_spans but ensure width: simplify: if inline produced extra styling, use it and pad
            // To keep width correct, we flatten cell_spans to string and truncate already; now push with styles
            // For richer, push inline spans with padding trailing
            if cell_spans.len() == 1 && cell_spans[0].content == cell.as_str()
                || cell.trim().is_empty()
            {
                // plain fast path
                spans.push(Span::styled(
                    format!("{display}{}", " ".repeat(pad)),
                    row_style,
                ));
            } else {
                // styled: push spans then pad
                // approximate: extend spans content length may differ due to stripped markers, so recalc pad based on display
                for s in cell_spans {
                    spans.push(s);
                }
                // if inline caused markers to disappear, width may be off; add extra pad by computing difference
                let rendered_width: usize = row[ci]
                    .chars()
                    .filter(|c| *c != '*' && *c != '_' && *c != '`' && *c != '~')
                    .collect::<String>()
                    .len(); // rough
                let _ = rendered_width;
                spans.push(Span::styled(" ".repeat(pad.saturating_sub(0)), row_style));
                // ensure we fill to col width: add at least one pad if needed (fallback)
                if spans.last().map(|s| s.width() < pad).unwrap_or(true) {
                    // already padded
                }
            }
            spans.push(Span::styled(" ", row_style));
            spans.push(Span::styled("║", border_style)); // Use double lines
        }
        out.push(Line::from(spans));
        if is_header {
            out.push(Line::styled(build_border("╠", "╬", "╣"), border_style));
        }
    }
    // Enhanced bottom border with rounded corners
    out.push(Line::styled(build_border("╚", "╩", "╝"), border_style));
    out.push(Line::default());
    // compensate for inline width approximations: ensure each row width <= inner
    (out, consumed)
}

/// Per-token syntax highlighting for a code line. Returns styled spans.
/// All spans share `bg_secondary` background. `line` is the raw line text.
fn highlight_code_line(line: &str, lang: Option<&str>, t: &theme::Theme) -> Vec<Span<'static>> {
    let bg = Color::Rgb(12, 16, 28); // terminal-dark background
    let text_s = Style::default().fg(t.text_primary).bg(bg);
    let comment_s = Style::default()
        .fg(t.syntax_comment)
        .bg(bg)
        .add_modifier(Modifier::ITALIC);
    let keyword_s = Style::default()
        .fg(t.syntax_keyword)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
    let string_s = Style::default().fg(t.syntax_string).bg(bg);
    let number_s = Style::default().fg(t.syntax_number).bg(bg);
    let function_s = Style::default().fg(t.syntax_function).bg(bg);
    let type_s = Style::default().fg(t.syntax_keyword).bg(bg);
    let punct_s = Style::default().fg(t.text_muted).bg(bg);

    let lang_lower = lang.map(|l| l.to_lowercase());
    let keywords: &[&str] = match lang_lower.as_deref() {
        Some("rust") | Some("rs") => &[
            "fn", "let", "mut", "const", "static", "pub", "use", "mod", "struct", "enum", "impl",
            "trait", "type", "where", "for", "while", "loop", "match", "if", "else", "return",
            "break", "continue", "unsafe", "async", "await", "move", "ref", "crate", "super",
            "self", "Self", "true", "false", "in", "as", "dyn",
        ],
        Some("javascript") | Some("js") | Some("typescript") | Some("ts") => &[
            "function",
            "const",
            "let",
            "var",
            "if",
            "else",
            "for",
            "while",
            "return",
            "break",
            "continue",
            "switch",
            "case",
            "default",
            "try",
            "catch",
            "finally",
            "throw",
            "async",
            "await",
            "class",
            "extends",
            "new",
            "this",
            "super",
            "import",
            "export",
            "from",
            "typeof",
            "instanceof",
            "in",
            "of",
            "true",
            "false",
            "null",
            "undefined",
        ],
        Some("python") | Some("py") => &[
            "def", "class", "if", "elif", "else", "for", "while", "return", "break", "continue",
            "import", "from", "as", "try", "except", "finally", "raise", "with", "lambda", "yield",
            "global", "nonlocal", "pass", "assert", "True", "False", "None", "and", "or", "not",
            "in", "is",
        ],
        Some("go") | Some("golang") => &[
            "func",
            "var",
            "const",
            "type",
            "struct",
            "interface",
            "package",
            "import",
            "return",
            "if",
            "else",
            "for",
            "range",
            "switch",
            "case",
            "default",
            "break",
            "continue",
            "go",
            "select",
            "defer",
            "chan",
            "map",
            "goto",
        ],
        _ => &[],
    };
    let line_comment_prefixes: &[&str] = match lang_lower.as_deref() {
        Some("python") | Some("py") | Some("bash") | Some("sh") | Some("shell") | Some("yaml")
        | Some("yml") | Some("toml") => &["#"],
        Some("html") | Some("xml") => &["<!--"],
        _ => &["//"],
    };

    // Quick exits
    let trimmed = line.trim_start();
    // Whole-line comment
    if line_comment_prefixes.iter().any(|p| trimmed.starts_with(p)) {
        return vec![Span::styled(line.to_owned(), comment_s)];
    }

    let mut out: Vec<Span<'static>> = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0usize;
    let len = bytes.len();
    let mut buf_start = 0usize;
    let push_plain = |out: &mut Vec<Span<'static>>, buf_start: &mut usize, i: usize, s: Style| {
        if i > *buf_start {
            let chunk: String = line[*buf_start..i].to_owned();
            out.push(Span::styled(chunk, s));
            *buf_start = i;
        }
    };
    while i < len {
        let c = match line[i..].chars().next() {
            Some(ch) => ch,
            None => break,
        };

        // String literal detection
        if c == '"' || c == '\'' || c == '`' {
            push_plain(&mut out, &mut buf_start, i, text_s);
            // find matching end (no escape handling beyond \")
            let quote = c;
            let mut j = i + quote.len_utf8();
            while j < len {
                let cc = line[j..].chars().next().unwrap_or(' ');
                if cc == '\\' && j + 1 < len {
                    j += cc.len_utf8();
                    if j < len {
                        let next = line[j..].chars().next().unwrap_or(' ');
                        j += next.len_utf8();
                    }
                    continue;
                }
                if cc == quote {
                    j += cc.len_utf8();
                    break;
                }
                j += cc.len_utf8();
            }
            out.push(Span::styled(line[i..j].to_owned(), string_s));
            i = j;
            buf_start = i;
            continue;
        }
        // Line comment mid-line
        let is_line_comment_start = line_comment_prefixes.iter().any(|p| {
            line[i..].starts_with(p)
                && (p.len() > 1 || i == 0 || !line.as_bytes()[i - 1].is_ascii_alphanumeric())
        });
        if is_line_comment_start {
            push_plain(&mut out, &mut buf_start, i, text_s);
            out.push(Span::styled(line[i..].to_owned(), comment_s));
            i = len;
            buf_start = i;
            continue;
        }
        // Number
        if c.is_ascii_digit() {
            push_plain(&mut out, &mut buf_start, i, text_s);
            let mut j = i;
            while j < len {
                let cc = line[j..].chars().next().unwrap_or(' ');
                if cc.is_ascii_alphanumeric() || cc == '.' || cc == '_' {
                    j += cc.len_utf8();
                } else {
                    break;
                }
            }
            out.push(Span::styled(line[i..j].to_owned(), number_s));
            i = j;
            buf_start = i;
            continue;
        }
        // Identifier / keyword
        if c.is_alphabetic() || c == '_' {
            let mut j = i;
            while j < len {
                let cc = line[j..].chars().next().unwrap_or(' ');
                if cc.is_alphanumeric() || cc == '_' {
                    j += cc.len_utf8();
                } else {
                    break;
                }
            }
            let word: &str = &line[i..j];
            // check for function call (next non-space is '(')
            let mut k = j;
            while k < len && line.as_bytes()[k] == b' ' {
                k += 1;
            }
            let style = if keywords.contains(&word) {
                keyword_s
            } else if k < len && line.as_bytes()[k] == b'(' {
                function_s
            } else if word
                .chars()
                .next()
                .map(|c| c.is_uppercase())
                .unwrap_or(false)
                && word.len() > 1
            {
                type_s
            } else {
                text_s
            };
            push_plain(&mut out, &mut buf_start, i, text_s);
            out.push(Span::styled(word.to_owned(), style));
            i = j;
            buf_start = i;
            continue;
        }
        // Punctuation
        if matches!(
            c,
            '{' | '}'
                | '('
                | ')'
                | '['
                | ']'
                | ';'
                | ','
                | '.'
                | ':'
                | '<'
                | '>'
                | '='
                | '+'
                | '-'
                | '*'
                | '/'
                | '!'
                | '?'
                | '&'
                | '|'
                | '^'
                | '~'
                | '@'
                | '#'
                | '$'
                | '%'
        ) {
            push_plain(&mut out, &mut buf_start, i, text_s);
            let mut j = i + c.len_utf8();
            // group runs of same-class punctuation
            while j < len {
                let cc = line[j..].chars().next().unwrap_or(' ');
                if matches!(
                    cc,
                    '{' | '}'
                        | '('
                        | ')'
                        | '['
                        | ']'
                        | ';'
                        | ','
                        | '.'
                        | ':'
                        | '<'
                        | '>'
                        | '='
                        | '+'
                        | '-'
                        | '*'
                        | '/'
                        | '!'
                        | '?'
                        | '&'
                        | '|'
                        | '^'
                        | '~'
                        | '@'
                        | '#'
                        | '$'
                        | '%'
                ) {
                    j += cc.len_utf8();
                } else {
                    break;
                }
            }
            out.push(Span::styled(line[i..j].to_owned(), punct_s));
            i = j;
            buf_start = i;
            continue;
        }
        i += c.len_utf8();
    }
    push_plain(&mut out, &mut buf_start, i, text_s);
    if out.is_empty() {
        out.push(Span::styled(line.to_owned(), text_s));
    }
    out
}

fn strip_inline_markers(s: &str) -> String {
    // remove **, *, `, ~~, etc for width measurement
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' | '_' | '`' | '~' => {
                // skip if doubled
                if let Some(&next) = chars.peek()
                    && next == c
                {
                    chars.next();
                }
                // skip single too — we just strip marker chars for width
            }
            '[' => {
                // copy label until ]
                let mut label = String::new();
                for ch in chars.by_ref() {
                    if ch == ']' {
                        break;
                    }
                    label.push(ch);
                }
                out.push_str(&label);
                // skip (url)
                if chars.peek() == Some(&'(') {
                    chars.next();
                    for ch in chars.by_ref() {
                        if ch == ')' {
                            break;
                        }
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out
}

fn truncate_to_width(s: &str, max: usize) -> String {
    if UnicodeWidthStr::width(s) <= max {
        return s.to_owned();
    }
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(1);
        if w + cw > max.saturating_sub(1) {
            out.push('…');
            break;
        }
        out.push(ch);
        w += cw;
    }
    out
}

fn user_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    let t = theme::active();
    let inner = width.saturating_sub(4) as usize;
    let mut lines = Vec::new();
    let text_bg = t.bg_primary;
    let _border = Style::default().fg(t.accent_secondary).bg(text_bg);
    let _text_s = Style::default().fg(t.text_primary).bg(text_bg);
    let head_style = Style::default()
        .fg(t.text_inverse)
        .bg(t.accent_secondary)
        .add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(t.text_muted).bg(text_bg);

    // Header chip: "● YOU" (purple)
    lines.push(Line::from(vec![
        Span::styled(" ● YOU ", head_style),
        Span::styled(" ", muted),
    ]));

    // Bubble: purple left border, white-ish fill
    let bubble_bg = t.bg_secondary;
    let bubble_border = Style::default().fg(t.accent_secondary).bg(bubble_bg);
    let bubble_text = Style::default().fg(t.text_primary).bg(bubble_bg);
    lines.push(Line::from(vec![
        Span::styled(" ", text_bg),
        Span::styled("│", bubble_border),
        Span::styled(" ", bubble_text),
    ]));
    for l in wrap(text, inner.saturating_sub(2)) {
        let pad = inner.saturating_sub(UnicodeWidthStr::width(l.as_str()) + 2);
        lines.push(Line::from(vec![
            Span::styled(" ", text_bg),
            Span::styled("│", bubble_border),
            Span::styled(" ", bubble_text),
            Span::styled(l, bubble_text),
            Span::styled(format!("{:width$}", "", width = pad), bubble_text),
            Span::styled(" ", bubble_text),
            Span::styled("│", bubble_border),
            Span::styled(" ", text_bg),
        ]));
    }
    lines.push(Line::from(vec![
        Span::styled(" ", text_bg),
        Span::styled("│", bubble_border),
        Span::styled(" ", bubble_text),
    ]));
    lines.push(Line::default());
    lines
}

#[allow(clippy::unwrap_used)] // safe: all calls guarded by len() checks
fn assistant_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    let t = theme::active();
    let inner = width.saturating_sub(4) as usize;
    let mut lines = Vec::new();
    let text_bg = t.bg_primary;
    let bubble_bg = t.bg_secondary;
    let head_style = Style::default()
        .fg(t.text_inverse)
        .bg(t.accent_primary)
        .add_modifier(Modifier::BOLD);
    let bubble_border = Style::default().fg(t.accent_primary).bg(bubble_bg);
    let bubble_text = Style::default().fg(t.text_primary).bg(bubble_bg);
    let muted = Style::default().fg(t.text_muted).bg(text_bg);

    // Header chip: "◆ GOVINDA" (blue)
    lines.push(Line::from(vec![
        Span::styled(" ◆ GOVINDA ", head_style),
        Span::styled(" ", muted),
    ]));

    // Bubble: blue left border, white-ish fill
    lines.push(Line::from(vec![
        Span::styled(" ", text_bg),
        Span::styled("│", bubble_border),
        Span::styled(" ", bubble_text),
    ]));

    for seg in split_fences(text) {
        match seg {
            Segment::Text(s) => {
                let raw_lines: Vec<&str> = s.lines().collect();
                let mut idx = 0usize;
                let mut para_buf: Vec<String> = Vec::new();
                let flush_para = |buf: &mut Vec<String>, out: &mut Vec<Line<'static>>| {
                    if buf.is_empty() {
                        return;
                    }
                    let joined = buf.join(" ");
                    for l in paragraph_lines(&joined, inner.saturating_sub(2)) {
                        let pad = inner.saturating_sub(l.width() + 2);
                        out.push(Line::from(vec![
                            Span::styled(" ", text_bg),
                            Span::styled("│", bubble_border),
                            Span::styled(" ", bubble_text),
                            Span::styled(
                                l.spans
                                    .iter()
                                    .map(|s| s.content.as_ref())
                                    .collect::<String>(),
                                bubble_text,
                            ),
                            Span::styled(format!("{:width$}", "", width = pad), bubble_text),
                            Span::styled(" ", bubble_text),
                            Span::styled("│", bubble_border),
                            Span::styled(" ", text_bg),
                        ]));
                    }
                    out.push(Line::default());
                    buf.clear();
                };
                while idx < raw_lines.len() {
                    let line = raw_lines[idx];
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        flush_para(&mut para_buf, &mut lines);
                        if lines.last().map(|l| l.width() != 0).unwrap_or(true) {
                            lines.push(Line::default());
                        }
                        idx += 1;
                        continue;
                    }
                    if is_hr(trimmed) {
                        flush_para(&mut para_buf, &mut lines);
                        lines.push(hr_line(inner));
                        lines.push(Line::default());
                        idx += 1;
                        continue;
                    }
                    if let Some((level, rest)) = heading_level(trimmed) {
                        flush_para(&mut para_buf, &mut lines);
                        lines.extend(heading_lines(level, &rest, inner));
                        lines.push(Line::default());
                        idx += 1;
                        continue;
                    }
                    if trimmed.starts_with('>') {
                        flush_para(&mut para_buf, &mut lines);
                        lines.extend(blockquote_lines(line, inner));
                        lines.push(Line::default());
                        idx += 1;
                        continue;
                    }
                    if is_table_row(trimmed)
                        && idx + 1 < raw_lines.len()
                        && is_separator_row(raw_lines[idx + 1])
                    {
                        flush_para(&mut para_buf, &mut lines);
                        let slice: Vec<&str> = raw_lines[idx..].to_vec();
                        let (tbl, consumed) = table_lines(&slice, inner);
                        if consumed > 0 {
                            lines.extend(tbl);
                            idx += consumed;
                            continue;
                        }
                    }
                    let is_list_start = {
                        let tt = trimmed;
                        tt.starts_with("- ")
                            || tt.starts_with("* ")
                            || tt.starts_with("+ ")
                            || tt.starts_with("· ")
                            || (tt.len() >= 2
                                && tt.chars().next().unwrap().is_ascii_digit()
                                && tt.chars().nth(1) == Some('.'))
                    };
                    if is_list_start {
                        flush_para(&mut para_buf, &mut lines);
                        let slice: Vec<&str> = raw_lines[idx..].to_vec();
                        let (lst, consumed) = list_block_lines(&slice, inner);
                        if consumed > 0 {
                            lines.extend(lst);
                            lines.push(Line::default());
                            idx += consumed;
                            continue;
                        }
                    }
                    para_buf.push(trimmed.to_owned());
                    idx += 1;
                }
                flush_para(&mut para_buf, &mut lines);
            }
            Segment::Code { lang, body } => {
                let lang_label = lang.unwrap_or("code");
                let is_shell = matches!(
                    lang.map(str::to_ascii_lowercase).as_deref(),
                    Some("sh")
                        | Some("bash")
                        | Some("zsh")
                        | Some("shell")
                        | Some("console")
                        | Some("terminal")
                        | Some("cmd")
                        | Some("powershell")
                        | Some("ps1")
                        | Some("pwsh"),
                );
                let block_icon = if is_shell {
                    icons::COMMANDS_TITLE
                } else {
                    icons::CODE_BLOCK
                };
                let label = format!(" {} {} ", block_icon, lang_label);
                let copied_text = "copied";

                // Clean dark terminal background
                let code_bg = Color::Rgb(28, 35, 52);
                let border_c = Color::Rgb(52, 64, 92);
                let w = inner.max(20);
                let label_w = UnicodeWidthStr::width(label.as_str());
                let copied_w = UnicodeWidthStr::width(copied_text);
                let fixed = 2 + 3 + copied_w + 2 + 2;
                let fill_top = w.saturating_sub(fixed + label_w);

                let border_s = Style::default().fg(border_c).bg(code_bg);
                let label_style = Style::default()
                    .fg(if is_shell {
                        t.accent_success
                    } else {
                        t.accent_primary
                    })
                    .bg(code_bg)
                    .add_modifier(Modifier::BOLD);
                let copied_style = Style::default()
                    .fg(t.text_muted)
                    .bg(code_bg)
                    .add_modifier(Modifier::DIM);

                // Top rounded border with lang label + copied
                lines.push(Line::from(vec![
                    Span::styled(" ", text_bg),
                    Span::styled(" ╭─", border_s),
                    Span::styled(label, label_style),
                    Span::styled(format!("{:─<width$}", "", width = fill_top), border_s),
                    Span::styled(format!(" {copied_text} "), copied_style),
                    Span::styled("╮", border_s),
                ]));

                // Code body with line numbers
                let code_area = w.saturating_sub(6);
                let body_bg = Style::default().bg(code_bg);
                for (i, raw) in body.trim_end_matches('\n').lines().enumerate() {
                    for (j, l) in wrap(raw, code_area).into_iter().enumerate() {
                        let num = if j == 0 {
                            format!("{:>3} ", i + 1)
                        } else {
                            "    ".to_owned()
                        };
                        let trimmed = l.trim_start();
                        let is_cmd = trimmed.starts_with("$ ")
                            || trimmed.starts_with("> ")
                            || trimmed.starts_with("PS>");
                        let mut spans = vec![
                            Span::styled(" ", text_bg),
                            Span::styled("│ ", border_s),
                            Span::styled(num, Style::default().fg(t.text_muted).bg(code_bg)),
                        ];
                        if is_cmd {
                            let sym_end = trimmed.find(' ').map_or(trimmed.len(), |p| p + 1);
                            let (sym, rest_all) = trimmed.split_at(sym_end.min(trimmed.len()));
                            let indent_ws = l.len() - trimmed.len();
                            if indent_ws > 0 {
                                spans.push(Span::styled(" ".repeat(indent_ws), body_bg));
                            }
                            spans.push(Span::styled(
                                sym.to_owned(),
                                Style::default()
                                    .fg(t.accent_success)
                                    .bg(code_bg)
                                    .add_modifier(Modifier::BOLD),
                            ));
                            spans.push(Span::styled(
                                rest_all.to_owned(),
                                Style::default().fg(t.text_primary).bg(code_bg),
                            ));
                        } else {
                            let code_spans = highlight_code_line(&l, lang, &t);
                            for s in code_spans {
                                spans.push(s);
                            }
                        }
                        spans.push(Span::styled(" │", border_s));
                        lines.push(Line::from(spans));
                    }
                }

                // Bottom rounded border
                lines.push(Line::from(vec![
                    Span::styled(" ", text_bg),
                    Span::styled(
                        format!(" ╰{:─<width$}╯", "", width = w.saturating_sub(2)),
                        border_s,
                    ),
                ]));
                lines.push(Line::default());
            }
        }
    }
    // collapse duplicate trailing blanks
    while lines.len() >= 2
        && lines[lines.len() - 1].width() == 0
        && lines[lines.len() - 2].width() == 0
    {
        lines.pop();
    }
    // Bottom border
    lines.push(Line::from(vec![
        Span::styled(" ", text_bg),
        Span::styled("│", bubble_border),
        Span::styled(" ", bubble_text),
    ]));
    if lines.last().map(|l| l.width() != 0).unwrap_or(false) {
        lines.push(Line::default());
    }
    lines
}

/// `/raw` mode: plain wrapped text, no markdown parsing.
fn assistant_raw_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    let t = theme::active();
    let inner = width.saturating_sub(4) as usize;
    let mut lines = Vec::new();

    // ── Enhanced Header chip with timestamp ──
    let timestamp = chrono::Local::now().format("%H:%M").to_string();
    lines.push(Line::from(vec![
        Span::styled(
            format!(" {} GOVINDA ", icons::ASSISTANT),
            Style::default()
                .fg(t.text_inverse)
                .bg(t.accent_primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {} ", timestamp),
            Style::default()
                .fg(t.text_muted)
                .bg(t.bg_primary)
                .add_modifier(Modifier::DIM),
        ),
    ]));

    // ── Enhanced Bubble border with rounded corners ──
    let border = Style::default().fg(t.border_default).bg(t.bg_secondary);
    lines.push(Line::from(vec![
        Span::styled(" ╭", border),
        Span::styled("─".repeat(inner.saturating_sub(1)), border),
        Span::styled("╮", border),
    ]));

    for l in text.lines() {
        for w in wrap(l, inner) {
            lines.push(Line::from(vec![
                Span::styled(" │ ", border),
                Span::styled(w, t.text()),
                Span::styled(" │", border),
            ]));
        }
    }

    // ── Enhanced Bubble border bottom with rounded corners ──
    let bubble_border = Style::default().fg(t.border_default).bg(t.bg_secondary);
    lines.push(Line::from(vec![
        Span::styled(" ╰", bubble_border),
        Span::styled("─".repeat(inner.saturating_sub(1)), bubble_border),
        Span::styled("╯", bubble_border),
    ]));
    lines.push(Line::default());
    lines
}

fn tool_lines(name: &str, args: &str, ok: Option<bool>) -> Vec<Line<'static>> {
    let t = theme::active();
    let inner_w = 60usize;
    let budget = inner_w.saturating_sub(name.len() + 12);
    let args_short: String = {
        let compact = args.replace('\n', " ").replace('\r', "");
        let mut s: String = compact.chars().take(budget).collect();
        if compact.chars().count() > budget {
            s.push('…');
        }
        s
    };
    let (status_text, status_color, bar_color) = match ok {
        None => ("running…", t.accent_warning, t.accent_warning),
        Some(true) => ("ok", t.accent_success, t.accent_success),
        Some(false) => ("error", t.accent_error, t.accent_error),
    };
    let bar_s = Style::default().fg(bar_color).bg(t.bg_primary);
    let text_s = Style::default().fg(t.text_primary).bg(t.bg_primary);
    let name_s = Style::default()
        .fg(t.text_primary)
        .bg(t.bg_primary)
        .add_modifier(Modifier::BOLD);
    let args_s = Style::default().fg(t.text_muted).bg(t.bg_primary);
    let status_s = Style::default()
        .fg(status_color)
        .bg(t.bg_primary)
        .add_modifier(Modifier::BOLD);

    let name_w = UnicodeWidthStr::width(name);
    let args_w = UnicodeWidthStr::width(args_short.as_str());
    let status_w = UnicodeWidthStr::width(status_text);
    let used = 3 + name_w + 1 + args_w + status_w + 2;
    let fill = inner_w.saturating_sub(used).max(0);

    vec![Line::from(vec![
        Span::styled("  ", bar_s),
        Span::styled("█", bar_s),
        Span::styled(" ", text_s),
        Span::styled(name.to_owned(), name_s),
        Span::styled(" ", text_s),
        Span::styled(args_short, args_s),
        Span::styled(" ".repeat(fill), text_s),
        Span::styled(status_text, status_s),
    ])]
}

fn checklist_lines(title: &str, steps: &[(String, bool)], width: u16) -> Vec<Line<'static>> {
    let t = theme::active();
    let done = steps.iter().filter(|(_, d)| *d).count();
    let total = steps.len().max(1);
    let filled = (done * 10) / total;
    let bar: String = "█".repeat(filled) + &"░".repeat(10 - filled);

    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                format!(" {} PLAN ", icons::MODE_PLAN),
                Style::default()
                    .fg(t.text_inverse)
                    .bg(t.accent_primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {title}"),
                Style::default()
                    .fg(t.accent_primary)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{done}/{total} [{bar}]"),
                if done == total {
                    t.success()
                } else {
                    Style::default().fg(t.text_secondary)
                },
            ),
        ]),
    ];
    let inner = width.saturating_sub(6) as usize;
    for (i, (step, is_done)) in steps.iter().enumerate() {
        let (box_char, style) = if *is_done {
            (icons::CHECK, t.success())
        } else {
            ("·", t.text_dim())
        };
        for (j, l) in wrap(step, inner).into_iter().enumerate() {
            let prefix = if j == 0 {
                format!("   {:>2}. {} ", i + 1, box_char)
            } else {
                "       ".to_owned()
            };
            lines.push(Line::styled(format!("{prefix}{l}"), style));
        }
    }
    lines.push(Line::default());
    lines
}

fn notice_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    let t = theme::active();
    wrap(text, width.saturating_sub(6) as usize)
        .into_iter()
        .map(|l| {
            Line::from(vec![
                Span::styled(
                    format!("  {} ", icons::INFO),
                    Style::default().fg(t.accent_primary),
                ),
                Span::styled(l, Style::default().fg(t.text_muted)),
            ])
        })
        .chain(std::iter::once(Line::default()))
        .collect()
}

/// Renders a structured error card with severity icon, title, detail, and
/// optional suggestion.
fn error_lines(entry: &ErrorEntry, width: u16) -> Vec<Line<'static>> {
    let t = theme::active();
    let inner = width.saturating_sub(4) as usize;

    // Severity → icon + color
    let (icon, color) = match entry.severity {
        ErrorSeverity::Info => (icons::INFO, t.accent_primary),
        ErrorSeverity::Warn => (icons::WARNING, t.accent_warning),
        ErrorSeverity::Error => (icons::ERRORS, t.accent_error),
        ErrorSeverity::Critical => (icons::ERRORS, t.accent_error),
    };

    // ── Title line ──
    let title_style = Style::default()
        .fg(color)
        .add_modifier(Modifier::BOLD);
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("  {icon} "), Style::default().fg(color)),
        Span::styled(entry.title.clone(), title_style),
    ])];

    // ── Detail lines (indented under the icon) ──
    for l in wrap(&entry.detail, inner.saturating_sub(4)) {
        lines.push(Line::from(vec![
            Span::raw("      "),
            Span::styled(l, Style::default().fg(t.text_primary)),
        ]));
    }

    // ── Suggestion line (if present) ──
    if let Some(ref suggestion) = entry.suggestion {
        for l in wrap(suggestion, inner.saturating_sub(4)) {
            lines.push(Line::from(vec![
                Span::styled("      → ", Style::default().fg(t.accent_secondary)),
                Span::styled(l, Style::default().fg(t.accent_secondary)),
            ]));
        }
    }

    lines.push(Line::default());
    lines
}

fn op_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    let t = theme::active();
    let inner = width.saturating_sub(4) as usize;
    // "AGENT START — read_file → grep → edit_file"
    let label = format!(" ● {text} ");
    let label_w = UnicodeWidthStr::width(label.as_str());
    // Center the label with dashes on both sides (like the screenshot)
    let total_dash = inner.saturating_sub(label_w);
    let left_dash = total_dash / 2;
    let right_dash = total_dash - left_dash;
    let label_style = Style::default()
        .fg(t.accent_secondary)
        .bg(t.bg_primary)
        .add_modifier(Modifier::BOLD);
    let dash_style = Style::default().fg(t.border_default).bg(t.bg_primary);
    vec![
        Line::from(vec![
            Span::styled("  ", dash_style),
            Span::styled("─".repeat(left_dash), dash_style),
            Span::styled(label, label_style),
            Span::styled("─".repeat(right_dash), dash_style),
        ]),
        Line::default(),
    ]
}

fn shell_lines(cmd: &str, output: &str, ok: bool, width: u16) -> Vec<Line<'static>> {
    let t = theme::active();
    let inner = width.saturating_sub(4) as usize;
    let status_color = if ok { t.accent_success } else { t.accent_error };
    let status_text = if ok { "ok" } else { "error" };
    let shell_bg = Color::Rgb(15, 20, 32); // dark terminal background
    let shell_border = Color::Rgb(36, 48, 77);
    let mut lines = Vec::new();
    // ── Header: status dot + tool name + status ──
    lines.push(Line::from(vec![
        Span::styled(
            " ● ",
            Style::default()
                .fg(status_color)
                .bg(t.bg_primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            cmd.to_owned(),
            Style::default()
                .fg(t.text_primary)
                .bg(t.bg_primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" ".repeat(2)),
        Span::styled(
            status_text,
            Style::default()
                .fg(status_color)
                .bg(t.bg_primary)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    // ── Dark terminal box ──
    lines.push(Line::from(vec![
        Span::styled(
            " ┌",
            Style::default().fg(Color::Rgb(52, 64, 92)).bg(shell_bg),
        ),
        Span::styled(
            "─".repeat(inner.saturating_sub(1)),
            Style::default().fg(Color::Rgb(52, 64, 92)).bg(shell_bg),
        ),
    ]));
    for l in output.lines() {
        for w in wrap(l, inner.saturating_sub(2)) {
            let fg = if w.trim_start().starts_with("error") || w.contains("cannot find") {
                t.accent_error
            } else if w.trim_start().starts_with("Finished") {
                t.accent_success
            } else {
                Color::Rgb(199, 210, 254) // light blue-grey for terminal output
            };
            lines.push(Line::from(vec![
                Span::styled(" │ ", Style::default().fg(shell_border).bg(shell_bg)),
                Span::styled(w, Style::default().fg(fg).bg(shell_bg)),
            ]));
        }
    }
    lines.push(Line::from(vec![
        Span::styled(
            " └",
            Style::default().fg(Color::Rgb(52, 64, 92)).bg(shell_bg),
        ),
        Span::styled(
            "─".repeat(inner.saturating_sub(1)),
            Style::default().fg(Color::Rgb(52, 64, 92)).bg(shell_bg),
        ),
    ]));
    lines.push(Line::default());
    lines
}

fn code_explicit_lines(lang: &str, code: &str, width: u16) -> Vec<Line<'static>> {
    let t = theme::active();
    let inner = width.saturating_sub(6) as usize;
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("  {} {} ", icons::CODE_BLOCK, lang),
            Style::default()
                .fg(t.accent_primary)
                .bg(t.bg_secondary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "copied",
            Style::default().fg(t.text_muted).bg(t.bg_secondary),
        ),
    ])];
    for (i, raw) in code.lines().enumerate() {
        for (j, w) in wrap(raw, inner.saturating_sub(6)).into_iter().enumerate() {
            let num = if j == 0 {
                format!("{:>2} ", i + 1)
            } else {
                "   ".to_owned()
            };
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default().bg(t.bg_secondary)),
                Span::styled(num, Style::default().fg(t.text_muted).bg(t.bg_secondary)),
                Span::styled(w, Style::default().fg(t.text_primary).bg(t.bg_secondary)),
            ]));
        }
    }
    lines.push(Line::default());
    lines
}

/// Flattens the transcript (+ optional live streaming buffer) into styled
/// lines ready for a `Paragraph`. `raw` (from `/raw`) renders assistant
/// output as plain wrapped text instead of parsed markdown.
///
/// Perf: this function is called every frame (250 ms). It memoizes the last
/// result keyed by a hash of `entries` + frame params. A hit avoids the
/// full markdown/word-wrap re-parse.
pub fn build_lines(
    entries: &[ChatEntry],
    streaming: Option<&str>,
    busy: bool,
    width: u16,
    raw: bool,
) -> Vec<Line<'static>> {
    // Fast path: check memo cache. Hashing entries is O(total chars) but
    // still ~10× cheaper than the word-wrap + inline_spans + table layout
    // that `assistant_lines` does per entry.
    let (hash, streaming_hash) = hash_entries(entries, streaming, busy);
    let cache_hit = BUILD_CACHE.with(|c| {
        if let Some(cached) = c.borrow().as_ref() {
            if cached.hash == hash
                && cached.streaming_hash == streaming_hash
                && cached.width == width
                && cached.raw == raw
                && cached.busy == busy
            {
                return Some(cached.lines.clone());
            }
        }
        None
    });
    if let Some(lines) = cache_hit {
        return lines;
    }

    let mut lines = Vec::new();
    for e in entries {
        match e {
            ChatEntry::User(t) => lines.extend(user_lines(t, width)),
            ChatEntry::Assistant(t) => {
                if raw {
                    lines.extend(assistant_raw_lines(t, width));
                } else {
                    lines.extend(assistant_lines(t, width));
                }
            }
            ChatEntry::Tool { name, args, ok } => lines.extend(tool_lines(name, args, *ok)),
            ChatEntry::Op(t) => lines.extend(op_lines(t, width)),
            ChatEntry::Shell { cmd, output, ok } => {
                lines.extend(shell_lines(cmd, output, *ok, width))
            }
            ChatEntry::Code { lang, code } => lines.extend(code_explicit_lines(lang, code, width)),
            ChatEntry::Checklist { title, steps } => {
                lines.extend(checklist_lines(title, steps, width))
            }
            ChatEntry::Notice(t) => lines.extend(notice_lines(t, width)),
            ChatEntry::Error(e) => lines.extend(error_lines(e, width)),
        }
    }
    if let Some(partial) = streaming {
        if !partial.is_empty() {
            let t = theme::active();
            let spinner = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏']
                [std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0usize, |d| (d.subsec_millis() / 100 % 10) as usize)];
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {} GOVINDA ", icons::ASSISTANT),
                    Style::default()
                        .fg(t.text_inverse)
                        .bg(t.accent_primary)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" {spinner}"),
                    Style::default()
                        .fg(t.accent_primary)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
            lines.extend(
                wrap(partial, width.saturating_sub(4) as usize)
                    .into_iter()
                    .map(|l| Line::from(vec![Span::raw("  "), Span::styled(l, t.text())])),
            );
        } else if busy {
            let t = theme::active();
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {} GOVINDA ", icons::ASSISTANT),
                    Style::default()
                        .fg(t.text_inverse)
                        .bg(t.accent_primary)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" thinking…", Style::default().fg(t.text_muted)),
            ]));
        }
    }
    if lines.is_empty() {
        let t = theme::active();
        lines.push(Line::default());
        lines.push(Line::styled(
            "  Ask me to code, debug, or explain…",
            Style::default().fg(t.text_muted),
        ));
        lines.push(Line::default());
    }
    // Store in memo cache for next frame.
    BUILD_CACHE.with(|c| {
        *c.borrow_mut() = Some(CachedLines {
            hash,
            width,
            raw,
            busy,
            streaming_hash,
            lines: lines.clone(),
        });
    });
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_on_width() {
        let lines = wrap("aaaa bbbb cccc dddd", 9);
        assert_eq!(lines, vec!["aaaa bbbb", "cccc dddd"]);
    }

    #[test]
    fn hard_breaks_overlong_words() {
        let lines = wrap("abcdefghijklmno", 5);
        assert_eq!(lines, vec!["abcde", "fghij", "klmno"]);
    }

    #[test]
    fn splits_code_fences_with_language() {
        let segs = split_fences("prose\n```rust\nfn a() {}\n```\ntail");
        assert_eq!(segs.len(), 3);
        match &segs[1] {
            Segment::Code { lang, body } => {
                assert_eq!(*lang, Some("rust"));
                assert!(body.contains("fn a"));
            }
            other => panic!("expected code segment, got {}", segment_kind(other)),
        }
    }

    #[test]
    fn unterminated_fence_still_renders() {
        let segs = split_fences("```py\nprint(1)");
        assert!(matches!(segs[0], Segment::Code { .. }));
    }

    #[test]
    fn build_lines_handles_every_entry_kind() {
        let entries = [
            ChatEntry::User("hi".into()),
            ChatEntry::Assistant("answer ```rs\nx()\n``` end".into()),
            ChatEntry::Tool {
                name: "read_file".into(),
                args: r#"{"path":"a.rs"}"#.into(),
                ok: Some(true),
            },
            ChatEntry::Notice("note".into()),
        ];
        let lines = build_lines(&entries, None, false, 80, false);
        assert!(lines.len() > 6);
    }

    #[test]
    fn raw_mode_skips_markdown_parsing() {
        let entries = vec![ChatEntry::Assistant("### Heading\n- item".into())];
        // Markdown mode strips the heading marker; raw mode keeps it.
        let md = build_lines(&entries, None, false, 80, false);
        let raw = build_lines(&entries, None, false, 80, true);
        let flat_md: String = md
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        let flat_raw: String = raw
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        assert!(
            !flat_md.contains("###"),
            "markdown mode should parse headings"
        );
        assert!(
            flat_raw.contains("###"),
            "raw mode should show the text verbatim"
        );
    }

    #[test]
    fn heading_stripped_and_styled() {
        let lines = assistant_lines("### **Pros & Cons of Java**", 80);
        let flat: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!flat.contains("###"), "heading markers should be stripped");
        assert!(flat.contains("Pros & Cons of Java"));
    }

    #[test]
    fn hr_renders_as_line() {
        let lines = assistant_lines("---", 40);
        let flat: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(flat.contains("─"), "hr should be box drawing");
        assert!(!flat.contains("---"));
    }

    #[test]
    fn table_renders_with_borders() {
        let md = "| **Pros** | **Cons** |\n|---|---|\n| Platform-independent | Verbose syntax |\n| Strong OOP | Slower than C |";
        let lines = assistant_lines(md, 80);
        let flat: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(flat.contains("╔"), "table should have top border");
        assert!(flat.contains("║"), "table should have column separators");
        assert!(!flat.contains("|---|"), "separator row should be consumed");
        assert!(flat.contains("Pros"), "bold markers stripped");
    }

    #[test]
    fn inline_bold_and_code() {
        let spans = inline_spans("Use **bold** and `code` here", Style::default());
        let flat: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(!flat.contains("**"), "markers removed");
        assert!(flat.contains("bold"));
        assert!(flat.contains("code"));
    }
}
