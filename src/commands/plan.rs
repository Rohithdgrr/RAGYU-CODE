//! `/plan <task>` — decompose a task into a numbered plan, store it as the
//! session todo list, and (on confirmation) hand it back for autonomous
//! step-by-step execution.
//!
//! Planning is one non-interactive model call: the system prompt demands a
//! bare numbered list so parsing stays trivial, and the workspace overview
//! from `scan_project` is attached so steps reference real files. Destructive
//! steps are still gated per-call by the usual confirmation layer during
//! execution; `/plan` itself only gates "run this whole plan?".

use super::{App, Outcome, dim, err, ok};
use crate::api;
use crate::render::paint;
use crossterm::style::Color;

/// Most steps a plan may contain.
const MAX_STEPS: usize = 10;

const PLAN_SYSTEM: &str = "You are a planning assistant for a coding agent that can scan, read, \
edit, and verify code in the user's workspace. Decompose the given task into short, concrete, \
self-contained steps (at most 10), ordered so each builds on the last. Prefer steps that name \
specific files or commands. Reply ONLY with a markdown numbered list — one step per line, no \
prose before or after.";

/// Entry point for `/plan [task]`.
pub(super) async fn handle(arg: &str, app: &mut App) -> Outcome {
    let task = arg.trim();
    if task.is_empty() {
        println!("usage: /plan <task>   — decompose a task and execute it step by step");
        return Outcome::Handled;
    }
    if !app.tools_enabled {
        err("planning needs function calling — enable it with /tools on");
        return Outcome::Handled;
    }

    dim("planning…");
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let overview = crate::scan::scan(&cwd).await;
    let ctx = vec![
        api::Message::system(PLAN_SYSTEM),
        api::Message::user(format!(
            "Task:\n{task}\n\nWorkspace overview:\n{overview}\n\nProduce the plan now."
        )),
    ];
    let auth = app.config.provider.auth();
    let opts = api::ChatOptions {
        max_response_bytes: app.max_response_bytes,
        read_timeout: app.read_timeout,
        // Planning must not recurse into tool calls.
        ..api::ChatOptions::new(auth.token(), &app.config.model, app.config.temperature)
    };

    let mut out = String::new();
    let mut no_calls = Vec::new();
    let mut sink = api::StreamSink::new(&mut out, &mut no_calls);
    match api::stream_chat(
        &app.http,
        app.config.provider.as_ref(),
        &opts,
        &ctx,
        &mut sink,
        |_| {},
    )
    .await
    {
        Ok(()) => {}
        Err(e) => {
            err(&format!("plan generation failed ({e:#})."));
            return Outcome::Handled;
        }
    }

    let steps = parse_steps(&out);
    if steps.is_empty() {
        err("the model returned no parseable steps — try rephrasing the task.");
        return Outcome::Handled;
    }

    // The todo list becomes the plan's progress tracker.
    app.todos = steps
        .iter()
        .map(|s| super::todo::Todo {
            text: s.clone(),
            done: false,
        })
        .collect();
    super::todo::save(app);

    println!();
    println!("{}", paint("proposed plan:", accent()));
    for (i, s) in steps.iter().enumerate() {
        println!(
            "  {} {}",
            paint(format!("{:>2}.", i + 1), Color::DarkGrey),
            s
        );
    }
    ok("plan stored in /todo. Confirm to execute all steps autonomously.");
    Outcome::Plan(steps)
}

/// Extracts numbered/bulleted steps from a model reply. Tolerates stray
/// prose by ignoring lines without a list marker; caps at [`MAX_STEPS`].
/// Shared with the TUI planner (`tui::app`).
pub fn parse_steps(text: &str) -> Vec<String> {
    static MARKER: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = MARKER.get_or_init(|| {
        #[allow(clippy::expect_used)] // static, hand-checked pattern
        regex::Regex::new(r"^\s*(?:\d+[.)]|\*|-)\s+(.+)$").expect("valid regex")
    });
    text.lines()
        .filter_map(|l| re.captures(l).map(|c| c[1].trim().to_owned()))
        .filter(|s| !s.is_empty())
        .take(MAX_STEPS)
        .collect()
}

fn accent() -> Color {
    crate::render::accent()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numbered_and_bulleted_lists_only() {
        let raw = "\
Here is my plan:

1. Read src/api.rs to locate the auth middleware
2) Add JWT validation before route handlers
* Run cargo check to verify compilation
- Update tests/auth.rs with token cases

Some closing prose that must be ignored.";
        let steps = parse_steps(raw);
        assert_eq!(steps.len(), 4, "{steps:?}");
        assert!(steps[0].starts_with("Read src/api.rs"));
        assert!(steps[3].starts_with("Update tests"));
    }

    #[test]
    fn empty_and_markerless_replies_yield_no_steps() {
        assert!(parse_steps("").is_empty());
        assert!(parse_steps("just prose\nno list here").is_empty());
    }

    #[test]
    fn plans_are_capped_at_max_steps() {
        let raw: String = (1..=20).map(|i| format!("{i}. step {i}\n")).collect();
        assert_eq!(parse_steps(&raw).len(), MAX_STEPS);
    }
}
