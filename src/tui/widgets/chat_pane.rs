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

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::super::{icons, theme};


#[derive(Debug, Clone)]
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
    /// Plan checklist: steps with done flags, rendered with a progress bar.
    Checklist {
        title: String,
        steps: Vec<(String, bool)>,
    },
    /// Local system notices (errors, hints, command feedback).
    Notice(String),
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
                let (lang, body) = after[..end].split_once('\n').unwrap_or(("", after[..end].trim_end()));
                segments.push(Segment::Code {
                    lang: if lang.trim().is_empty() { None } else { Some(lang) },
                    body,
                });
                rest = &after[end + 3..];
            }
            None => {
                // Unterminated fence: render what we have as code.
                let (lang, body) = after.split_once('\n').unwrap_or(("", after));
                segments.push(Segment::Code {
                    lang: if lang.trim().is_empty() { None } else { Some(lang) },
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

fn inline_spans(raw: &str, base: Style) -> Vec<Span<'static>> {
    // fast path: no markup chars
    if !raw.contains('`') && !raw.contains('*') && !raw.contains('_') && !raw.contains('[') && !raw.contains('~') {
        return vec![Span::styled(raw.to_owned(), base)];
    }
    let mut out: Vec<Span<'static>> = Vec::new();
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0usize;
    let len = chars.len();
    let t = theme::active();
    // styles derived from base
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

    while i < len {
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
        if i + 1 < len && ((chars[i] == '*' && chars[i + 1] == '*') || (chars[i] == '_' && chars[i + 1] == '_')) {
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
                        let is_code = s.style.bg == Some(t.bg_secondary) && s.style.fg == Some(t.syntax_string);
                        if is_code {
                            out.push(s);
                        } else {
                            out.push(Span::styled(s.content.into_owned(), s.style.add_modifier(Modifier::BOLD)));
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
        if chars[i] == '[' {
            if let Some(close_bracket) = (i + 1..len).position(|k| chars[k] == ']').map(|p| i + 1 + p) {
                if close_bracket + 1 < len && chars[close_bracket + 1] == '(' {
                    if let Some(close_paren) = (close_bracket + 2..len).position(|k| chars[k] == ')').map(|p| close_bracket + 2 + p) {
                        let label: String = chars[i + 1..close_bracket].iter().collect();
                        let url: String = chars[close_bracket + 2..close_paren].iter().collect();
                        if !label.is_empty() {
                            out.push(Span::styled(label, link_style));
                            // show url muted in parentheses if not too long
                            if !url.is_empty() && url.len() < 60 {
                                out.push(Span::styled(format!(" ({})", url), Style::default().fg(t.text_muted).add_modifier(Modifier::DIM)));
                            }
                            i = close_paren + 1;
                            continue;
                        }
                    }
                }
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
                    let is_word = marker == '_' && (i > 0 && chars[i - 1].is_alphanumeric()) || (end + 1 < len && chars[end + 1].is_alphanumeric());
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
            if c == '`' || c == '*' || c == '_' || c == '[' || c == '~' {
                break;
            }
            i += 1;
        }
        let chunk: String = chars[start..i].iter().collect();
        // preserve prior style for bold context
        out.push(Span::styled(chunk, base));
        // avoid infinite loop if we didn't advance due to single marker without closing
        if out.last().is_some() && start == i - 1 && matches!(chars[start], '*' | '_' | '~' | '`' | '[') {
            // already handled as plain above; keep moving
        }
    }
    // merge consecutive spans with same style to keep rendering cheap
    let mut merged: Vec<Span<'static>> = Vec::new();
    for s in out {
        if let Some(last) = merged.last_mut() {
            if last.style == s.style {
                let mut combined = last.content.clone().into_owned();
                combined.push_str(&s.content);
                last.content = combined.into();
                continue;
            }
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
    if t.len() < 3 { return false; }
    let mut chars = t.chars().filter(|c| !c.is_whitespace());
    let first = match chars.next() { Some(c) => c, None => return false };
    if !matches!(first, '-' | '*' | '_') { return false; }
    let mut count = 1;
    for c in chars {
        if c != first { return false; }
        count += 1;
    }
    count >= 3
}

fn heading_level(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start();
    let mut level = 0usize;
    for c in trimmed.chars() {
        if c == '#' { level += 1; } else { break; }
    }
    if level == 0 || level > 6 { return None; }
    let rest = trimmed[level..].trim_start();
    // require space after #s or treat as not heading if missing (GFM permissive: allow without space)
    if rest.is_empty() { return None; }
    Some((level, rest.to_owned()))
}

fn is_separator_row(line: &str) -> bool {
    let t = line.trim();
    if !t.contains('|') { return false; }
    // remove pipes and spaces, should be only - : |
    let stripped: String = t.chars().filter(|c| *c != '|' && !c.is_whitespace()).collect();
    if stripped.is_empty() { return false; }
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
    Line::styled(
        format!("  {}", "─".repeat(w)),
        Style::default().fg(t.border_default).bg(t.bg_primary),
    )
}

fn heading_lines(level: usize, raw: &str, inner: usize) -> Vec<Line<'static>> {
    let t = theme::active();
    let (fg, prefix, underline) = match level {
        1 => (t.accent_primary, "━━ ", true),
        2 => (t.accent_primary, "── ", false),
        3 => (t.accent_secondary, "▸ ", false),
        _ => (t.text_primary, "· ", false),
    };
    let base = Style::default().fg(fg).bg(t.bg_primary).add_modifier(Modifier::BOLD);
    let mut out = Vec::new();
    // allow inline formatting inside heading
    let content = raw.trim();
    // strip surrounding ** if present (common LLM pattern: "### **Title**")
    let content = if content.starts_with("**") && content.ends_with("**") && content.len() >= 4 {
        &content[2..content.len() - 2]
    } else { content };
    let mut wrapped = wrap_plain_for_spans(content, inner - 4);
    if wrapped.is_empty() { wrapped.push(String::new()); }
    // first line with prefix
    for (idx, w) in wrapped.into_iter().enumerate() {
        let line_spans = if idx == 0 {
            let mut v = vec![Span::styled("  ", base), Span::styled(prefix, Style::default().fg(fg).bg(t.bg_primary).add_modifier(Modifier::BOLD))];
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
    if joined.is_empty() { return vec![]; }
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
    let base = Style::default().fg(t.text_secondary).bg(t.bg_primary).add_modifier(Modifier::ITALIC);
    let content = raw.trim_start_matches('>').trim_start().trim_start_matches('>').trim();
    let mut out = Vec::new();
    let bar = Style::default().fg(t.accent_secondary).bg(t.bg_primary).add_modifier(Modifier::BOLD);
    for chunk in wrap(content, inner - 4) {
        let mut spans = vec![
            Span::styled("  ", base),
            Span::styled("▎ ", bar),
        ];
        spans.extend(inline_spans(&chunk, base));
        out.push(Line::from(spans));
    }
    out
}

fn list_block_lines(lines: &[&str], inner: usize) -> (Vec<Line<'static>>, usize) {
    let t = theme::active();
    let mut out = Vec::new();
    let mut consumed = 0usize;
    let bullet_style = Style::default().fg(t.accent_secondary).bg(t.bg_primary).add_modifier(Modifier::BOLD);
    let text_style = t.text();
    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i];
        let trimmed = raw.trim_start();
        if trimmed.is_empty() { break; }
        let (is_list, marker_len, _ordered) = if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("· ") {
            (true, 2, false)
        } else if trimmed.starts_with("+ ") {
            (true, 2, false)
        } else if trimmed.len() >= 3 && trimmed.chars().next().unwrap().is_ascii_digit() && trimmed.chars().nth(1) == Some('.') && trimmed.chars().nth(2) == Some(' ') {
            (true, 3, true)
        } else if trimmed.starts_with("  - ") || trimmed.starts_with("  * ") {
            (true, 2, false)
        } else {
            (false, 0, false)
        };
        if !is_list { break; }
        // extract content after marker (handle nested indent)
        let content = trimmed[marker_len..].trim_start();
        // handle continued lines that are indented (>=2 spaces) as part of same item
        let mut full = content.to_owned();
        let mut look = i + 1;
        while look < lines.len() {
            let nxt = lines[look];
            if nxt.starts_with("  ") && !nxt.trim().is_empty() && !is_hr(nxt) && heading_level(nxt).is_none() && !is_table_row(nxt) {
                // continuation
                full.push(' ');
                full.push_str(nxt.trim());
                look += 1;
            } else { break; }
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
    if raw_lines.len() < 2 { return (vec![], 0); }
    if !is_table_row(raw_lines[0]) || !is_separator_row(raw_lines[1]) { return (vec![], 0); }
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
            while cells.len() < cols { cells.push(String::new()); }
            cells.truncate(cols);
            rows.push(cells);
            consumed += 1;
        } else if line.trim().is_empty() {
            break;
        } else {
            break;
        }
    }
    if rows.len() < 1 { return (vec![], 0); }
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
                } else { break; }
            } else { break; }
        }
    } else if needed < usable && usable - needed < 20 {
        // distribute extra a bit
        let extra = (usable - needed) / cols;
        for w in &mut col_widths { *w += extra; }
    }
    // helpers to build border lines
    let mut out: Vec<Line<'static>> = Vec::new();
    let border_style = Style::default().fg(t.border_default).bg(t.bg_primary);
    let header_bg = t.bg_secondary;
    let header_style = Style::default().fg(t.text_primary).bg(header_bg).add_modifier(Modifier::BOLD);
    let cell_style = Style::default().fg(t.text_primary).bg(t.bg_primary);
    let alt_style = Style::default().fg(t.text_primary).bg(t.bg_primary);

    let build_border = |left: &str, mid: &str, right: &str| -> String {
        let mut s = String::from("  ");
        s.push_str(left);
        for (ci, w) in col_widths.iter().enumerate() {
            s.push_str(&"─".repeat(*w + 2));
            if ci + 1 < cols { s.push_str(mid); } else { s.push_str(right); }
        }
        s
    };
    // top
    out.push(Line::styled(build_border("┌", "┬", "┐"), border_style));
    for (ri, row) in rows.iter().enumerate() {
        let is_header = ri == 0;
        let row_style = if is_header { header_style } else if ri % 2 == 0 { cell_style } else { alt_style };
        // build cell line with inline spans per cell
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::styled("  ", border_style));
        spans.push(Span::styled("│", border_style));
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
            if cell_spans.len() == 1 && cell_spans[0].content == cell.as_str() || cell.trim().is_empty() {
                // plain fast path
                spans.push(Span::styled(format!("{display}{}", " ".repeat(pad)), row_style));
            } else {
                // styled: push spans then pad
                // approximate: extend spans content length may differ due to stripped markers, so recalc pad based on display
                for s in cell_spans {
                    spans.push(s);
                }
                // if inline caused markers to disappear, width may be off; add extra pad by computing difference
                let rendered_width: usize = row[ci].chars().filter(|c| *c != '*' && *c != '_' && *c != '`' && *c != '~').collect::<String>().len(); // rough
                let _ = rendered_width;
                spans.push(Span::styled(" ".repeat(pad.saturating_sub(0)), row_style));
                // ensure we fill to col width: add at least one pad if needed (fallback)
                if spans.last().map(|s| s.width() < pad).unwrap_or(true) {
                    // already padded
                }
            }
            spans.push(Span::styled(" ", row_style));
            spans.push(Span::styled("│", border_style));
        }
        out.push(Line::from(spans));
        if is_header {
            out.push(Line::styled(build_border("├", "┼", "┤"), border_style));
        }
    }
    out.push(Line::styled(build_border("└", "┴", "┘"), border_style));
    out.push(Line::default());
    // compensate for inline width approximations: ensure each row width <= inner
    (out, consumed)
}

fn strip_inline_markers(s: &str) -> String {
    // remove **, *, `, ~~, etc for width measurement
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' | '_' | '`' | '~' => {
                // skip if doubled
                if let Some(&next) = chars.peek() {
                    if next == c { chars.next(); }
                }
                // skip single too — we just strip marker chars for width
            }
            '[' => {
                // copy label until ]
                let mut label = String::new();
                while let Some(ch) = chars.next() {
                    if ch == ']' { break; }
                    label.push(ch);
                }
                out.push_str(&label);
                // skip (url)
                if chars.peek() == Some(&'(') {
                    chars.next();
                    while let Some(ch) = chars.next() {
                        if ch == ')' { break; }
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out
}

fn truncate_to_width(s: &str, max: usize) -> String {
    if UnicodeWidthStr::width(s) <= max { return s.to_owned(); }
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
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!(" {} YOU ", icons::USER),
            Style::default()
                .fg(t.text_inverse)
                .bg(t.accent_secondary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " ",
            t.text(),
        ),
    ])];
    for l in wrap(text, inner) {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(l, t.text()),
        ]));
    }
    lines.push(Line::default());
    lines
}

fn assistant_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    let t = theme::active();
    let inner = width.saturating_sub(4) as usize;
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!(" {} GOVINDA ", icons::ASSISTANT),
            Style::default()
                .fg(t.text_inverse)
                .bg(t.accent_primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " ",
            t.text(),
        ),
    ])];
    for seg in split_fences(text) {
        match seg {
            Segment::Text(s) => {
                // block-level markdown parser
                let raw_lines: Vec<&str> = s.lines().collect();
                let mut idx = 0usize;
                let mut para_buf: Vec<String> = Vec::new();
                let flush_para = |buf: &mut Vec<String>, out: &mut Vec<Line<'static>>| {
                    if buf.is_empty() { return; }
                    let joined = buf.join(" ");
                    out.extend(paragraph_lines(&joined, inner));
                    out.push(Line::default());
                    buf.clear();
                };
                while idx < raw_lines.len() {
                    let line = raw_lines[idx];
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        flush_para(&mut para_buf, &mut lines);
                        // collapse consecutive blanks to single blank
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
                    // table detection: need header + separator
                    if is_table_row(trimmed) && idx + 1 < raw_lines.len() && is_separator_row(raw_lines[idx + 1]) {
                        flush_para(&mut para_buf, &mut lines);
                        let slice: Vec<&str> = raw_lines[idx..].to_vec();
                        let (tbl, consumed) = table_lines(&slice, inner);
                        if consumed > 0 {
                            lines.extend(tbl);
                            idx += consumed;
                            continue;
                        }
                    }
                    // list
                    let is_list_start = {
                        let tt = trimmed;
                        tt.starts_with("- ") || tt.starts_with("* ") || tt.starts_with("+ ") || tt.starts_with("· ") || (tt.len() >= 2 && tt.chars().next().unwrap().is_ascii_digit() && tt.chars().nth(1) == Some('.'))
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
                    // default: paragraph accumulation
                    para_buf.push(trimmed.to_owned());
                    idx += 1;
                }
                flush_para(&mut para_buf, &mut lines);
                // trim trailing blank duplication
                if lines.last().map(|l| l.width() == 0).unwrap_or(false) && lines.len() > 1 {
                    // keep one trailing blank as spacer (already)
                }
            }
            Segment::Code { lang, body } => {
                let lang_label = lang.unwrap_or("code");
                let is_shell = matches!(
                    lang.map(str::to_ascii_lowercase).as_deref(),
                    Some("sh") | Some("bash") | Some("zsh") | Some("shell") | Some("console")
                        | Some("terminal") | Some("cmd") | Some("powershell") | Some("ps1") | Some("pwsh"),
                );
                // Icon: terminal glyph for shell blocks, code glyph otherwise.
                let block_icon = if is_shell {
                    icons::COMMANDS_TITLE
                } else {
                    icons::FILE_CODE
                };
                let label = format!(" {} {} ", block_icon, lang_label);
                let border = Style::default().fg(t.border_default).bg(t.bg_secondary);
                let label_style = Style::default()
                    .fg(if is_shell { t.accent_success } else { t.accent_primary })
                    .bg(t.bg_secondary)
                    .add_modifier(Modifier::BOLD);
                let body_bg = Style::default().bg(t.bg_secondary);

                // Precise box: margin "  " + full-width panel of W = inner cols.
                // top:    ┌─ LABEL ───…──┐
                // rows:   │ NN content…  │   (padded so the glass surface is solid)
                // bottom: └──────────────┘
                let w = inner.max(16);
                let label_w = UnicodeWidthStr::width(label.as_str());
                let fill_top = w.saturating_sub(4 + label_w); // "┌─" + "─┐"
                lines.push(Line::from(vec![
                    Span::styled("  ", t.text()),
                    Span::styled("┌─", border),
                    Span::styled(label, label_style),
                    Span::styled(format!("{:─<width$}", "", width = fill_top), border),
                    Span::styled("┐", border),
                ]));

                let code_area = w.saturating_sub(8); // │ + space + NNN + space … content … space │
                for (i, raw) in body.trim_end_matches('\n').lines().enumerate() {
                    for (j, l) in wrap(raw, code_area).into_iter().enumerate() {
                        let num = if j == 0 { format!("{i:>3} ") } else { "    ".to_owned() };
                        let text_w = UnicodeWidthStr::width(l.as_str());
                        let pad = code_area.saturating_sub(4 + text_w);

                        // Terminal command detection: "$ cmd", "> cmd", "PS>" prompts.
                        let trimmed = l.trim_start();
                        let is_cmd = trimmed.starts_with("$ ")
                            || trimmed.starts_with("> ")
                            || trimmed.starts_with("PS>");
                        let mut spans = vec![
                            Span::styled("  ", t.text()),
                            Span::styled("│ ", border),
                            Span::styled(num, Style::default().fg(t.text_muted).bg(t.bg_secondary)),
                        ];
                        if is_cmd {
                            // split prompt symbol from the command text
                            let sym_end = trimmed
                                .find(' ')
                                .map_or(trimmed.len(), |p| p + 1);
                            let (sym, rest_all) = trimmed.split_at(sym_end.min(trimmed.len()));
                            let indent_ws = l.len() - trimmed.len();
                            if indent_ws > 0 {
                                spans.push(Span::styled(" ".repeat(indent_ws), body_bg));
                            }
                            spans.push(Span::styled(
                                sym.to_owned(),
                                Style::default().fg(t.accent_success).bg(t.bg_secondary).add_modifier(Modifier::BOLD),
                            ));
                            spans.push(Span::styled(
                                format!("{rest_all}{:width$}", "", width = pad),
                                Style::default().fg(t.text_primary).bg(t.bg_secondary),
                            ));
                        } else {
                            let fg = if l.trim_start().starts_with("//") || l.trim_start().starts_with('#') {
                                t.syntax_comment
                            } else {
                                t.text_primary
                            };
                            spans.push(Span::styled(
                                format!("{l}{:width$}", "", width = pad),
                                Style::default().fg(fg).bg(t.bg_secondary),
                            ));
                        }
                        spans.push(Span::styled(" │", border));
                        lines.push(Line::from(spans));
                    }
                }

                lines.push(Line::from(vec![
                    Span::styled("  ", t.text()),
                    Span::styled(format!("└{:─<width$}┘", "", width = w.saturating_sub(2)), border),
                ]));
                lines.push(Line::default());
            }
        }
    }
    // collapse duplicate trailing blanks to single
    while lines.len() >= 2 && lines[lines.len() - 1].width() == 0 && lines[lines.len() - 2].width() == 0 {
        lines.pop();
    }
    if lines.last().map(|l| l.width() != 0).unwrap_or(false) {
        lines.push(Line::default());
    }
    lines
}

/// `/raw` mode: plain wrapped text, no markdown parsing.
fn assistant_raw_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    let t = theme::active();
    let inner = width.saturating_sub(4) as usize;
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!(" {} GOVINDA ", icons::ASSISTANT),
            Style::default()
                .fg(t.text_inverse)
                .bg(t.accent_primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", t.text()),
    ])];
    for l in text.lines() {
        for w in wrap(l, inner) {
            lines.push(Line::from(Span::styled(w, t.text())));
        }
    }
    lines.push(Line::default());
    lines
}

fn tool_lines(name: &str, args: &str, ok: Option<bool>) -> Vec<Line<'static>> {
    let t = theme::active();
    let args_short: String = {
        let compact = args.replace('\n', " ");
        let mut s: String = compact.chars().take(48).collect();
        if compact.chars().count() > 48 {
            s.push('…');
        }
        s
    };
    let (icon, status_style) = match ok {
        None => (icons::PENDING, t.warning()),
        Some(true) => (icons::CHECK, t.success()),
        Some(false) => (icons::CROSS, t.error()),
    };
    vec![
        Line::from(vec![
            Span::styled(
                format!(" {icon} "),
                status_style,
            ),
            Span::styled(
                name.to_owned(),
                Style::default()
                    .fg(t.text_secondary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("({args_short})"),
                Style::default().fg(t.text_muted),
            ),
        ]),
        Line::default(),
    ]
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
                Span::styled(format!("  {} ", icons::INFO), Style::default().fg(t.accent_primary)),
                Span::styled(l, Style::default().fg(t.text_muted)),
            ])
        })
        .chain(std::iter::once(Line::default()))
        .collect()
}

/// Flattens the transcript (+ optional live streaming buffer) into styled
/// lines ready for a `Paragraph`. `raw` (from `/raw`) renders assistant
/// output as plain wrapped text instead of parsed markdown.
pub fn build_lines(
    entries: &[ChatEntry],
    streaming: Option<&str>,
    busy: bool,
    width: u16,
    raw: bool,
) -> Vec<Line<'static>> {
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
            ChatEntry::Checklist { title, steps } => {
                lines.extend(checklist_lines(title, steps, width))
            }
            ChatEntry::Notice(t) => lines.extend(notice_lines(t, width)),
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
                    .map(|l| Line::from(vec![
                        Span::raw("  "),
                        Span::styled(l, t.text()),
                    ])),
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
                Span::styled(
                    " thinking…",
                    Style::default().fg(t.text_muted),
                ),
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
        let flat_md: String = md.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect();
        let flat_raw: String = raw.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect();
        assert!(!flat_md.contains("###"), "markdown mode should parse headings");
        assert!(flat_raw.contains("###"), "raw mode should show the text verbatim");
    }

    #[test]
    fn heading_stripped_and_styled() {
        let lines = assistant_lines("### **Pros & Cons of Java**", 80);
        let flat: String = lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect::<Vec<_>>().join("\n");
        assert!(!flat.contains("###"), "heading markers should be stripped");
        assert!(flat.contains("Pros & Cons of Java"));
    }

    #[test]
    fn hr_renders_as_line() {
        let lines = assistant_lines("---", 40);
        let flat: String = lines.iter().flat_map(|l| l.spans.iter().map(|s| s.content.to_string())).collect();
        assert!(flat.contains("─"), "hr should be box drawing");
        assert!(!flat.contains("---"));
    }

    #[test]
    fn table_renders_with_borders() {
        let md = "| **Pros** | **Cons** |\n|---|---|\n| Platform-independent | Verbose syntax |\n| Strong OOP | Slower than C |";
        let lines = assistant_lines(md, 80);
        let flat: String = lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect::<Vec<_>>().join("\n");
        assert!(flat.contains("┌"), "table should have top border");
        assert!(flat.contains("│"), "table should have column separators");
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
