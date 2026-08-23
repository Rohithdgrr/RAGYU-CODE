//! Chat pane widget: renders the conversation transcript.
//!
//! Each `ChatEntry` becomes a header line plus wrapped body lines. Code
//! fences in assistant messages get a tinted background block with the
//! language label; tool calls render as compact status lines. The pane
//! auto-follows the bottom until the user scrolls up.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::super::theme;

/// Width used for code-block gutters.
const CODE_GUTTER: usize = 2;

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

fn user_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    let t = theme::active();
    let inner = width.saturating_sub(4) as usize;
    let mut lines = vec![Line::styled(
        "you ❯".to_owned(),
        Style::default().fg(t.accent_secondary).add_modifier(Modifier::BOLD),
    )];
    for l in wrap(text, inner) {
        lines.push(Line::styled(l, t.text()));
    }
    lines.push(Line::default());
    lines
}

fn assistant_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    let t = theme::active();
    let inner = width.saturating_sub(4) as usize;
    let mut lines = vec![Line::styled(
        "govinda".to_owned(),
        Style::default().fg(t.accent_primary).add_modifier(Modifier::BOLD),
    )];
    for seg in split_fences(text) {
        match seg {
            Segment::Text(s) => {
                for l in wrap(s, inner) {
                    if l.starts_with('#') {
                        lines.push(Line::styled(
                            l,
                            Style::default()
                                .fg(t.accent_primary)
                                .add_modifier(Modifier::BOLD),
                        ));
                    } else {
                        lines.push(Line::styled(l, t.text()));
                    }
                }
            }
            Segment::Code { lang, body } => {
                lines.push(Line::styled(
                    format!("┌─{}{}", lang.unwrap_or("code"), "─".repeat(inner.saturating_sub(lang.map_or(4, str::len)).min(40))),
                    Style::default().fg(t.text_muted).bg(t.bg_secondary),
                ));
                for (i, raw) in body.trim_end_matches('\n').lines().enumerate() {
                    for (j, l) in wrap(raw, inner - CODE_GUTTER - 1).into_iter().enumerate() {
                        let gutter = if j == 0 {
                            format!("{:>3} ", i + 1)
                        } else {
                            "    ".to_owned()
                        };
                        lines.push(Line::styled(
                            format!("{gutter} {l}"),
                            Style::default().fg(t.syntax_type).bg(t.bg_secondary),
                        ));
                    }
                }
                lines.push(Line::styled(
                    format!("└{}", "─".repeat(inner.min(60))),
                    Style::default().fg(t.text_muted).bg(t.bg_secondary),
                ));
            }
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
        None => ("…", t.warning()),
        Some(true) => ("✓", t.success()),
        Some(false) => ("✗", t.error()),
    };
    vec![
        Line::from(vec![
            Span::styled(format!("tool {icon}"), status_style),
            Span::styled(
                format!(" {name}({args_short})"),
                Style::default().fg(t.text_secondary),
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
        Line::styled(
            format!("plan {title}"),
            Style::default()
                .fg(t.accent_primary)
                .add_modifier(Modifier::BOLD),
        ),
        Line::styled(
            format!("{done}/{total} [{bar}]"),
            if done == total {
                t.success()
            } else {
                Style::default().fg(t.text_secondary)
            },
        ),
    ];
    let inner = width.saturating_sub(6) as usize;
    for (i, (step, is_done)) in steps.iter().enumerate() {
        let (box_char, style) = if *is_done {
            ("✓", t.success())
        } else {
            ("·", t.text_dim())
        };
        for (j, l) in wrap(step, inner).into_iter().enumerate() {
            let prefix = if j == 0 {
                format!("{:>2}. {} ", i + 1, box_char)
            } else {
                "     ".to_owned()
            };
            lines.push(Line::styled(format!("{prefix}{l}"), style));
        }
    }
    lines.push(Line::default());
    lines
}

fn notice_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    let t = theme::active();
    wrap(text, width.saturating_sub(2) as usize)
        .into_iter()
        .map(|l| Line::styled(format!("· {l}"), Style::default().fg(t.text_muted)))
        .chain(std::iter::once(Line::default()))
        .collect()
}

/// Flattens the transcript (+ optional live streaming buffer) into styled
/// lines ready for a `Paragraph`.
pub fn build_lines(
    entries: &[ChatEntry],
    streaming: Option<&str>,
    busy: bool,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for e in entries {
        match e {
            ChatEntry::User(t) => lines.extend(user_lines(t, width)),
            ChatEntry::Assistant(t) => lines.extend(assistant_lines(t, width)),
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
            lines.push(Line::styled(
                format!("govinda {spinner}"),
                Style::default()
                    .fg(t.accent_primary)
                    .add_modifier(Modifier::BOLD),
            ));
            lines.extend(
                wrap(partial, width.saturating_sub(4) as usize)
                    .into_iter()
                    .map(|l| Line::styled(l, t.text())),
            );
        } else if busy {
            lines.push(Line::styled("thinking…", Style::default().fg(theme::active().text_muted)));
        }
    }
    if lines.is_empty() {
        let t = theme::active();
        lines.push(Line::styled(
            "Ask me to code, debug, or explain…",
            Style::default().fg(t.text_muted),
        ));
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
        let lines = build_lines(&entries, None, false, 80);
        assert!(lines.len() > 6);
    }
}
