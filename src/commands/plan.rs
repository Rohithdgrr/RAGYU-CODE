//! `/plan <task>` — decompose a task into a numbered plan, store it as the
//! session todo list, and (on confirmation) hand it back for autonomous
//! step-by-step execution.
//!
//! Planning is one non-interactive model call: the system prompt demands a
//! bare numbered list so parsing stays trivial, and the workspace overview
//! from `scan_project` is attached so steps reference real files. Destructive
//! steps are still gated per-call by the usual confirmation layer during
//! execution; `/plan` itself only gates "run this whole plan?".

use super::{App, Outcome, dim, err, info, ok};
use crate::api;

/// Most steps a plan may contain.
const MAX_STEPS: usize = 10;

const PLAN_SYSTEM: &str = "You are a planning assistant for a coding agent that can scan, read, \
edit, and verify code in the user's workspace. Decompose the given task into short, concrete, \
self-contained steps (at most 10), ordered so each builds on the last. Prefer steps that name \
specific files or commands. Reply ONLY with a markdown numbered list — one step per line, no \
prose before or after.";

/// System prompt for the `--build` pipeline: forces every step to carry a
/// phase tag so execution can attach per-phase tool guidance, and forces a
/// trailing `[VERIFY]` step so the run ends in an objective check.
const PIPELINE_SYSTEM: &str = "You are a build-pipeline planner for a coding agent that can read, \
write, and edit files, run shell commands, run tests, and open previews in the user's workspace. \
Decompose the given task into short, concrete steps (at most 10). Tag every step with exactly \
one phase prefix chosen from: [DOCS], [CODE], [DEPS], [RUN], [PREVIEW], [VERIFY].\n\
Rules:\n\
- Use [DOCS] only when written documentation helps (README, design notes); omit otherwise.\n\
- Use [CODE] for creating or changing code and files, [DEPS] for installing dependencies, \
[RUN] for building or starting the program, [PREVIEW] for opening the result.\n\
- Order phases DOCS -> CODE -> DEPS -> RUN -> PREVIEW -> VERIFY, omitting phases that do not apply.\n\
- Always end with exactly one [VERIFY] step that runs tests or a project check.\n\
Reply ONLY with a markdown numbered list — one tagged step per line, no prose before or after.";

/// Phases of the `--build` pipeline, in canonical order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Docs,
    Code,
    Deps,
    Run,
    Preview,
    Verify,
}

impl Phase {
    /// Parses a `[TAG]` inner label (case-insensitive).
    pub fn from_tag(tag: &str) -> Option<Phase> {
        match tag.trim().to_ascii_uppercase().as_str() {
            "DOCS" => Some(Phase::Docs),
            "CODE" => Some(Phase::Code),
            "DEPS" => Some(Phase::Deps),
            "RUN" => Some(Phase::Run),
            "PREVIEW" => Some(Phase::Preview),
            "VERIFY" => Some(Phase::Verify),
            _ => None,
        }
    }

    /// Canonical bracket tag shown in headers and reports.
    pub fn tag(self) -> &'static str {
        match self {
            Phase::Docs => "DOCS",
            Phase::Code => "CODE",
            Phase::Deps => "DEPS",
            Phase::Run => "RUN",
            Phase::Preview => "PREVIEW",
            Phase::Verify => "VERIFY",
        }
    }

    /// Per-phase tool guidance injected into the step prompt during
    /// autonomous execution, steering the model toward the right tools.
    pub fn hint(self) -> &'static str {
        match self {
            Phase::Docs => {
                "Write the documentation with write_file (e.g. README.md or docs/). Do not modify source code in this step."
            }
            Phase::Code => {
                "Implement this with edit_file / insert_after / insert_before / write_file. Read files before editing."
            }
            Phase::Deps => {
                "Install the required dependencies with run_shell (e.g. cargo add / npm install / pip install)."
            }
            Phase::Run => {
                "Build and start the program with run_shell. Report the command and how to stop it."
            }
            Phase::Preview => {
                "Open the result for the user with open_preview or run_shell (browser/file viewer)."
            }
            Phase::Verify => {
                "Verify with run_test and/or check_project. A non-zero exit or failing test counts as failure — report it honestly."
            }
        }
    }

    /// Keyword inference for untagged steps (models sometimes drop tags).
    fn infer(step: &str) -> Phase {
        let lower = step.to_ascii_lowercase();
        let has = |needle: &str| lower.contains(needle);
        // Order matters: "run tests" must classify as Verify, not Run.
        if has("doc") || has("readme") || has("spec") || has("design") {
            Phase::Docs
        } else if has("install")
            || has("cargo add")
            || has("npm i")
            || has("pnpm add")
            || has("yarn add")
            || has("pip install")
            || has("dependency")
            || has("dependencies")
        {
            Phase::Deps
        } else if has("preview") || has("browser") || has("open ") || has("screenshot") {
            Phase::Preview
        } else if has("test") || has("check") || has("verify") || has("lint") || has("assert") {
            Phase::Verify
        } else if has("run")
            || has("build")
            || has("start")
            || has("serve")
            || has("launch")
            || has("compile")
        {
            Phase::Run
        } else {
            Phase::Code
        }
    }
}

/// Text used when the model omits a `[VERIFY]` step — the pipeline always
/// ends in an objective check.
const FALLBACK_VERIFY_STEP: &str =
    "run the project's tests / checks and confirm everything passes";

/// One planned pipeline step: its phase plus the tag-stripped description.
pub type PipelineStep = (Phase, String);

/// Parses a pipeline reply into tagged steps. Tolerates missing tags by
/// keyword inference, and guarantees a trailing `[VERIFY]` step. Caps at
/// [`MAX_STEPS`] (reserving room for the guaranteed verify step).
pub fn parse_pipeline_steps(text: &str) -> Vec<PipelineStep> {
    let mut steps: Vec<PipelineStep> = parse_steps(text)
        .into_iter()
        .map(|step| {
            let (phase, rest) = split_phase_tag(&step);
            (
                phase.unwrap_or_else(|| Phase::infer(rest)),
                rest.to_owned(),
            )
        })
        .collect();
    let needs_verify = !steps.iter().any(|(p, _)| *p == Phase::Verify);
    if needs_verify {
        steps.truncate(MAX_STEPS - 1);
        steps.push((Phase::Verify, FALLBACK_VERIFY_STEP.to_owned()));
    } else {
        steps.truncate(MAX_STEPS);
    }
    steps
}

/// Splits a leading `[TAG]` off a step line; `None` when absent/unknown.
fn split_phase_tag(step: &str) -> (Option<Phase>, &str) {
    if let Some(rest) = step.strip_prefix('[')
        && let Some(close) = rest.find(']')
    {
        let body = rest[close + 1..].trim_start();
        return (Phase::from_tag(&rest[..close]), body);
    }
    (None, step.trim())
}

/// One non-interactive planning call: streams silently and returns the raw
/// model text. Shared by `/plan` and the `--build` pipeline.
async fn ask_planner(app: &mut App, system: &str, user: &str) -> anyhow::Result<String> {
    let ctx = vec![
        api::Message::system(system),
        api::Message::user(user.to_owned()),
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
    api::stream_chat(
        &app.http,
        app.config.provider.as_ref(),
        &opts,
        &ctx,
        &mut sink,
        |_| {},
    )
    .await?;
    Ok(out)
}

/// Generates the `--build` pipeline plan for `task`: one planning call with
/// the phase-forcing system prompt, parsed into tagged steps.
pub async fn generate_pipeline(app: &mut App, task: &str) -> anyhow::Result<Vec<PipelineStep>> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let overview = crate::scan::scan(&cwd).await;
    let user = format!(
        "Task:\n{task}\n\nWorkspace overview:\n{overview}\n\nProduce the phased plan now."
    );
    let raw = ask_planner(app, PIPELINE_SYSTEM, &user).await?;
    Ok(parse_pipeline_steps(&raw))
}

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
    let out = match ask_planner(
        app,
        PLAN_SYSTEM,
        &format!(
            "Task:\n{task}\n\nWorkspace overview:\n{overview}\n\nProduce the plan now."
        ),
    )
    .await
    {
        Ok(out) => out,
        Err(e) => {
            err(format!("plan generation failed ({e:#})."));
            return Outcome::Handled;
        }
    };

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

    dim("proposed plan:");
    for (i, s) in steps.iter().enumerate() {
        info(format!("  {:>2}. {}", i + 1, s));
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

    #[test]
    fn pipeline_tags_are_parsed_and_stripped() {
        let raw = "\
1. [DOCS] Write README.md with usage examples
2. [CODE] Create src/server.rs with the HTTP handler
3. [DEPS] Install axum via cargo add
4. [RUN] Start the dev server on port 8080
5. [VERIFY] Run cargo test and report results";
        let steps = parse_pipeline_steps(raw);
        assert_eq!(steps.len(), 5, "{steps:?}");
        assert_eq!(steps[0].0, Phase::Docs);
        assert_eq!(steps[0].1, "Write README.md with usage examples");
        assert_eq!(steps[2].0, Phase::Deps);
        assert_eq!(steps[4].0, Phase::Verify);
    }

    #[test]
    fn untagged_steps_fall_back_to_keyword_inference() {
        let raw = "1. Install the serde crate\n2. Refactor main.rs\n3. Run cargo check";
        let steps = parse_pipeline_steps(raw);
        assert_eq!(steps[0].0, Phase::Deps, "{steps:?}");
        assert_eq!(steps[1].0, Phase::Code);
        assert_eq!(steps[2].0, Phase::Verify);
        // "run tests" must classify as Verify, not Run.
        assert_eq!(parse_pipeline_steps("1. run all tests")[0].0, Phase::Verify);
    }

    #[test]
    fn verify_step_is_guaranteed() {
        // No [VERIFY] in the reply — one is appended.
        let steps = parse_pipeline_steps("1. [CODE] write the parser");
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[1].0, Phase::Verify);

        // Cap still holds when appending: 10 tagged non-verify steps.
        let raw: String = (1..=10)
            .map(|i| format!("{i}. [CODE] task {i}\n"))
            .collect();
        let steps = parse_pipeline_steps(&raw);
        assert_eq!(steps.len(), MAX_STEPS);
        assert_eq!(steps.last().unwrap().0, Phase::Verify);
    }

    #[test]
    fn empty_reply_yields_only_the_verify_step() {
        let steps = parse_pipeline_steps("");
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].0, Phase::Verify);
    }

    #[test]
    fn phase_tags_round_trip_case_insensitively() {
        for tag in ["docs", "Docs", "DOCS", " code ", "verify"] {
            let (phase, _) = split_phase_tag(&format!("[{tag}] do it"));
            assert!(phase.is_some(), "{tag}");
        }
        assert!(split_phase_tag("[NOPE] x").0.is_none());
        assert!(split_phase_tag("no bracket").0.is_none());
    }
}
