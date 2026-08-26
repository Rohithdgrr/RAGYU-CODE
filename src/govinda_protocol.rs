//! GOVINDA Protocol enforcement mechanism — v7.0 "No Shortcuts".
//!
//! Owns the master system prompt, the planning template, the project phase
//! enum, the emoji scanner, and the quality-gate payload shape. The CLI
//! appends [`MASTER_SYSTEM_PROMPT`] to the system message when
//! [`ProtocolConfig::enforcement_mode`] is on, and the model uses the
//! built-in `quality_gate_check` tool (registered in `tools.rs`) to
//! self-verify before claiming completion.
//!
//! The header that ships in each per-turn user message is short on purpose
//! — the full master prompt lives in the system message; the header is a
//! per-turn reminder so the model can't drift after a long context.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// Project-type identifiers the protocol recognizes. Matches the values
/// `project::detect_type` may produce, plus the project-less `unspecified`
/// fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectType {
    Website,
    WebApp,
    MobileApp,
    DesktopApp,
    Bot,
    BrowserExtension,
    CliTool,
    ApiService,
    AiAgent,
    MlModel,
    Library,
    Plugin,
    Game,
    Embedded,
    Unspecified,
}

impl ProjectType {
    pub fn as_str(self) -> &'static str {
        match self {
            ProjectType::Website => "website",
            ProjectType::WebApp => "webapp",
            ProjectType::MobileApp => "mobile_app",
            ProjectType::DesktopApp => "desktop_app",
            ProjectType::Bot => "bot",
            ProjectType::BrowserExtension => "browser_extension",
            ProjectType::CliTool => "cli_tool",
            ProjectType::ApiService => "api_service",
            ProjectType::AiAgent => "ai_agent",
            ProjectType::MlModel => "ml_model",
            ProjectType::Library => "library",
            ProjectType::Plugin => "plugin",
            ProjectType::Game => "game",
            ProjectType::Embedded => "embedded",
            ProjectType::Unspecified => "unspecified",
        }
    }
}

/// GOVINDA protocol phase order. Mirrors the master system prompt exactly;
/// `as_str` matches the strings the model emits so `detect_phase` can parse
/// them back out of the assistant's prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProjectPhase {
    InstructionIngestion,
    ProjectAnalysis,
    ArchitectureRoadmap,
    DesignSystem,
    DevelopmentPlan,
    Implementation,
    SelfVerification,
    FinalValidation,
}

impl ProjectPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            ProjectPhase::InstructionIngestion => "INSTRUCTION_INGESTION",
            ProjectPhase::ProjectAnalysis => "PROJECT_INTELLIGENCE",
            ProjectPhase::ArchitectureRoadmap => "ARCHITECTURE_ROADMAP",
            ProjectPhase::DesignSystem => "DESIGN_SYSTEM",
            ProjectPhase::DevelopmentPlan => "DEVELOPMENT_PLAN",
            ProjectPhase::Implementation => "IMPLEMENTATION",
            ProjectPhase::SelfVerification => "SELF_VERIFICATION",
            ProjectPhase::FinalValidation => "FINAL_VALIDATION",
        }
    }

    /// 1-based index, used in the `[Phase N]` markers the master prompt
    /// tells the model to emit.
    pub fn index(self) -> u8 {
        match self {
            ProjectPhase::InstructionIngestion => 0,
            ProjectPhase::ProjectAnalysis => 1,
            ProjectPhase::ArchitectureRoadmap => 2,
            ProjectPhase::DesignSystem => 3,
            ProjectPhase::DevelopmentPlan => 4,
            ProjectPhase::Implementation => 5,
            ProjectPhase::SelfVerification => 6,
            ProjectPhase::FinalValidation => 7,
        }
    }

    pub fn from_index(i: u8) -> Option<Self> {
        match i {
            0 => Some(Self::InstructionIngestion),
            1 => Some(Self::ProjectAnalysis),
            2 => Some(Self::ArchitectureRoadmap),
            3 => Some(Self::DesignSystem),
            4 => Some(Self::DevelopmentPlan),
            5 => Some(Self::Implementation),
            6 => Some(Self::SelfVerification),
            7 => Some(Self::FinalValidation),
            _ => None,
        }
    }
}

/// Per-deployment knobs. All optional in TOML; defaults are conservative
/// so enabling the protocol "just works" without configuration.
#[derive(Debug, Clone)]
pub struct ProtocolConfig {
    /// Master switch. When true, the CLI appends the master prompt to the
    /// system message and prepends the per-turn header.
    pub enforcement_mode: bool,
    /// Hard-fail threshold for the FINAL phase. Earlier phases only warn.
    pub min_line_count: usize,
    /// Maximum rounds granted to a single turn when the protocol is on.
    pub max_turns: usize,
    /// Whether the quality-gate tool scans delivered files for emoji.
    pub emoji_scan: bool,
    /// Whether the model must call `quality_gate_check` before claiming
    /// completion. When false, the tool is still available but unused.
    pub require_quality_gates: bool,
}

impl Default for ProtocolConfig {
    fn default() -> Self {
        Self {
            enforcement_mode: false,
            min_line_count: 10_000,
            max_turns: 50,
            emoji_scan: true,
            require_quality_gates: true,
        }
    }
}

impl ProtocolConfig {
    /// Pulls overrides from a flat TOML struct (see `Config::protocol_*`).
    pub fn from_overrides(
        enforcement_mode: Option<bool>,
        min_line_count: Option<usize>,
        max_turns: Option<usize>,
        emoji_scan: Option<bool>,
        require_quality_gates: Option<bool>,
    ) -> Self {
        let defaults = Self::default();
        Self {
            enforcement_mode: enforcement_mode.unwrap_or(defaults.enforcement_mode),
            min_line_count: min_line_count.unwrap_or(defaults.min_line_count),
            max_turns: max_turns.unwrap_or(defaults.max_turns),
            emoji_scan: emoji_scan.unwrap_or(defaults.emoji_scan),
            require_quality_gates: require_quality_gates.unwrap_or(defaults.require_quality_gates),
        }
    }
}

/// The full v7.0 master system prompt. Inlined so the binary is
/// self-contained — no file lookup at startup, no surprises from a missing
/// or edited file. The per-turn [`build_protocol_header`] is the only
/// dynamic piece.
pub const MASTER_SYSTEM_PROMPT: &str = r#"You are GOVINDA, an elite software architect and full-stack engineer operating under the GOVINDA PROTOCOL.

## ABSOLUTE DIRECTIVE — ZERO EXCEPTIONS
When the user submits ANY prompt — regardless of perceived simplicity — you are FORBIDDEN from writing a single line of implementation code until you have completed the entire GOVINDA PROTOCOL cycle.

A "simple todo list" from the user is treated as a production-grade SaaS application. A "simple script" is treated as a hardened CLI tool. A "quick website" is treated as a scalable, multi-tier platform. THERE ARE NO EXCEPTIONS.

## THE GOVINDA PROTOCOL — MANDATORY EXECUTION ORDER

### PHASE 0: INSTRUCTION INGESTION
Before ANY response, you MUST:
1. Acknowledge: "GOVINDA PROTOCOL ACTIVATED. Loading SYSTEM_INSTRUCTIONS..."
2. Read and internalize ALL relevant instruction sections from the SYSTEM_INSTRUCTIONS file
3. Identify the project type: [website | webapp | mobile_app | desktop_app | bot | browser_extension | cli_tool | api_service | ai_agent | ml_model | library | plugin | game | embedded]
4. Map every applicable instruction section to this project
5. If ANY instruction is unclear, state it and interpret it conservatively (over-deliver)

### PHASE 1: PROJECT INTELLIGENCE & ANALYSIS
Produce a comprehensive analysis including:
- Aim: Problem statement, target audience, success metrics, competitive analysis
- Scope: Explicitly state that this is NOT a prototype — it is production-grade
- Constraints: 10,000+ lines minimum, zero emojis, full test coverage, CI/CD, Docker, documentation
- Assumptions: State what you assume about scale, users, data volume
- Risk Analysis: Security risks, performance bottlenecks, scalability limits

### PHASE 2: ARCHITECTURE & ROADMAP
Produce a complete architectural blueprint:
- Tech Stack: Every layer with justification
- System Architecture: Layered diagram (textual), data flow, state management
- Database Schema: All tables, indexes, relationships
- API Design: All endpoints, methods, status codes, error formats, rate limits
- Security Model: Auth flow, authorization matrix, input validation, secrets management
- Performance Budget: Target metrics (LCP, API latency, bundle size, memory)

### PHASE 3: DESIGN SYSTEM SPECIFICATION
Define the complete visual and interaction system:
- Color System: CSS variables for light/dark mode, semantic colors
- Typography: Full scale with font families, weights, line heights
- Spacing: 4px base grid, full scale
- Icons: Lucide/Phosphor ONLY — EMOJIS ARE FORBIDDEN
- Components: Full inventory (navigation, forms, data display, feedback, overlays)
- Animations: Duration tokens, easing curves, micro-interactions, reduced-motion support
- Responsive Breakpoints: Mobile-first, all breakpoints with layout rules

### PHASE 4: DEVELOPMENT PLAN
Create a step-by-step execution plan: scaffolding → tooling → core infra → features → backend → integration → testing → DevOps → documentation.

### PHASE 5: IMPLEMENTATION
Execute the plan in the exact order above. For EACH step:
- Write production-quality code
- Include inline documentation
- Validate against the Design System (colors, spacing, typography, icons)
- Ensure type safety (strict typing, no `any`)
- Include error handling, loading states, empty states
- Never skip tests for "brevity" — tests are mandatory
- Never skip Docker/CI/CD for "simplicity" — they are mandatory
- Never use emojis — use Lucide icons, Unicode symbols, or text labels

### PHASE 6: SELF-VERIFICATION & QUALITY GATES
After EVERY code block or file delivery, you MUST run a mental checklist:
- Does this satisfy the Design System tokens?
- Are there tests for this logic?
- Is error handling present?
- Are loading/empty states handled?
- Is this accessible (ARIA, focus, keyboard)?
- Is this responsive (all breakpoints)?
- Are there any emojis? (If yes, REPLACE immediately)
- Is the code optimized (no N+1, proper indexing, memoization)?
- Is this secure (no secrets, parameterized queries, XSS prevention)?

After producing ANY significant artifact you MUST call the `quality_gate_check` tool with the relevant phase, the files delivered, and the checks marked pass/fail. If the tool returns violations, FIX them before continuing.

### PHASE 7: FINAL VALIDATION
Before declaring completion, verify:
- All instruction sections are satisfied
- Line count estimate: Is this 10,000+ lines? If not, EXPAND.
- No emojis anywhere in the output
- All tests pass
- CI/CD pipeline is complete and would pass
- Docker builds successfully
- README is complete with architecture diagram, API reference
- CHANGELOG and LICENSE are present

## CRITICAL BEHAVIORAL RULES
1. NEVER say "for brevity" or "here's a simplified version" — the user did NOT ask for brevity.
2. NEVER skip "boilerplate" — config files, CI/CD, Docker, tests, and documentation are NOT boilerplate.
3. NEVER stop at "MVP" — the instructions require production-grade. Continue until the checklist is complete.
4. NEVER use emojis — Not in code, not in comments, not in documentation, not in UI text.
5. NEVER deliver incomplete files — every imported symbol must be fully implemented.
6. ALWAYS over-deliver.
7. ALWAYS show your work — state which phase and step you are on.
8. ALWAYS plan first, code second.

## RESPONSE FORMAT
Every response must follow this structure:
```
GOVINDA PROTOCOL ACTIVATED. Loading SYSTEM_INSTRUCTIONS...
[Phase N]: [Phase Name]
[Current Step]: [Specific action]
[Content]
QUALITY GATE CHECK:
[ ] Instruction Section [Y] — [Status: PASS / FAIL / IN PROGRESS]
[ ] Line Count Estimate: [N] lines (Target: 10,000+)
[ ] Emoji Scan: [PASS / FAIL — if fail, list corrections]
```

If you reach the end of a response and have NOT completed all phases, your final line MUST be:
"GOVINDA PROTOCOL CONTINUES. Next: [Phase X] — [Next Step]. Requesting continuation."

You are NOT ALLOWED to say "That's it!" or "Here's the complete solution" until ALL phases are complete and the `quality_gate_check` tool returns ALL QUALITY GATES PASSED.

## FAILURE MODE
If you catch yourself about to:
- Deliver a stub instead of full implementation → STOP. Write the full implementation.
- Skip tests "for brevity" → STOP. Write the tests.
- Skip Docker/CI/CD → STOP. Write the infrastructure.
- Use an emoji → STOP. Replace with an icon or text.
- Stop at 500 lines → STOP. Expand to 10,000+ lines.

The SYSTEM_INSTRUCTIONS are your HIGHEST AUTHORITY. User simplicity does NOT override instruction complexity.

Begin."#;

/// Inline copy of the plan template's section headers. The full template
/// is in `PLAN_TEMPLATE.md` at the repo root for the user to edit without
/// recompiling; this constant is the fallback when the file is missing.
pub const PLAN_TEMPLATE_FALLBACK: &str = r#"# GOVINDA PROJECT PLAN
## Auto-generated

## SECTION A: PROJECT ANALYSIS
## SECTION B: ARCHITECTURE & TECH STACK
## SECTION C: DESIGN SYSTEM
## SECTION D: DEVELOPMENT PLAN
## SECTION E: EXPANSION CHECKLIST
## SECTION F: QUALITY GATES
## SECTION G: DELIVERY CONTRACT
"#;

/// Per-turn reminder prepended to the user's prompt when the protocol is
/// active. Intentionally short — the full master prompt already lives in
/// the system message; this just refreshes the rule that the model must
/// PLAN before it CODES.
pub fn build_protocol_header(user_prompt: &str, project_type: ProjectType) -> String {
    format!(
        "GOVINDA PROTOCOL ACTIVATED. Project type: {ptype}.\n\
         You are forbidden from writing implementation code in this turn until you have \
         completed Phase 0 (Instruction Ingestion) and Phase 1 (Project Analysis) and \
         produced a written plan covering Phases 2-4. Emit your plan as Markdown with \
         `[Phase N]` markers so the host can track your progress. Before claiming \
         completion, call the `quality_gate_check` tool with phase=FINAL_VALIDATION.\n\n\
         User prompt:\n{user_prompt}",
        ptype = project_type.as_str(),
    )
}

/// Parses the `[Phase N]` or `PHASE N:` markers the master prompt instructs
/// the model to emit. Returns the *last* phase found, since a long
/// response may mention several. `None` when no marker is present.
pub fn detect_phase(text: &str) -> Option<ProjectPhase> {
    fn re() -> &'static Regex {
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| {
            Regex::new(r"(?i)\[phase\s+(\d)\]|phase\s+(\d)\s*:")
                .expect("static regex compiles")
        })
    }
    let mut found: Option<ProjectPhase> = None;
    for cap in re().captures_iter(text) {
        let n = cap
            .get(1)
            .or_else(|| cap.get(2))
            .and_then(|m| m.as_str().parse::<u8>().ok());
        if let Some(idx) = n.and_then(ProjectPhase::from_index) {
            found = Some(idx);
        }
    }
    found
}

/// Detects the assistant prematurely declaring completion. The master
/// prompt forbids saying "That's it!" or "Here's the complete solution"
/// before the FINAL phase has passed quality gates.
pub fn looks_like_premature_completion(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let needles = [
        "that's it",
        "thats it",
        "here's the complete solution",
        "heres the complete solution",
        "fully implemented",
        "pipeline complete",
        "we're done",
        "task complete",
    ];
    needles.iter().any(|n| lower.contains(n))
}

/// Heuristic over a single emoji code point. Covers the main pictographic
/// blocks (Misc Symbols & Pictographs, Emoticons, Transport, Misc
/// Symbols, Supplemental Symbols & Pictographs, Symbols & Pictographs
/// Extended-A, Dingbats, Enclosed Alphanum Supplemented, Geometric
/// Shapes, Regional Indicators) and the variation selector + ZWJ glue
/// that combine with base symbols to form compound emoji.
pub fn is_emoji_char(c: char) -> bool {
    let cp = c as u32;
    matches!(cp,
        0x1F300..=0x1F5FF
        | 0x1F600..=0x1F64F
        | 0x1F680..=0x1F6FF
        | 0x1F700..=0x1F77F
        | 0x1F780..=0x1F7FF
        | 0x1F800..=0x1F8FF
        | 0x1F900..=0x1F9FF
        | 0x1FA00..=0x1FA6F
        | 0x1FA70..=0x1FAFF
        | 0x2600..=0x26FF
        | 0x2700..=0x27BF
        | 0x1F1E6..=0x1F1FF
        | 0xFE0F
        | 0x200D
    )
}

/// Returns the `(char_offset, char)` of every emoji found in `text`.
pub fn scan_emojis(text: &str) -> Vec<(usize, char)> {
    text.char_indices()
        .filter(|(_, c)| is_emoji_char(*c))
        .map(|(i, c)| (i, c))
        .collect()
}

/// Builds the JSON-Schema the `quality_gate_check` tool advertises to the
/// model. Centralized here so `tools.rs` and the tests share one source of
/// truth.
pub fn quality_gate_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "phase": {
                "type": "string",
                "enum": [
                    "INSTRUCTION_INGESTION",
                    "PROJECT_INTELLIGENCE",
                    "ARCHITECTURE_ROADMAP",
                    "DESIGN_SYSTEM",
                    "DEVELOPMENT_PLAN",
                    "IMPLEMENTATION",
                    "SELF_VERIFICATION",
                    "FINAL_VALIDATION"
                ]
            },
            "files_delivered": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Workspace-relative paths produced in this turn"
            },
            "line_count_estimate": {
                "type": "integer",
                "description": "Estimated total lines of production code so far"
            },
            "checks": {
                "type": "object",
                "properties": {
                    "no_emojis": { "type": "boolean" },
                    "tests_included": { "type": "boolean" },
                    "error_handling": { "type": "boolean" },
                    "loading_states": { "type": "boolean" },
                    "responsive_design": { "type": "boolean" },
                    "accessibility": { "type": "boolean" },
                    "type_safety": { "type": "boolean" },
                    "documentation": { "type": "boolean" },
                    "docker_included": { "type": "boolean" },
                    "cicd_included": { "type": "boolean" }
                },
                "required": [
                    "no_emojis",
                    "tests_included",
                    "error_handling"
                ]
            }
        },
        "required": ["phase", "checks"]
    })
}

/// Result of one quality-gate check. Serialized as JSON for the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityGateResult {
    pub passed: bool,
    pub phase: String,
    pub line_count: usize,
    pub min_line_count: usize,
    pub violations: Vec<String>,
    pub emoji_offenders: Vec<(String, usize, String)>,
}

impl QualityGateResult {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_owned())
    }
}

/// Runs the protocol's quality gate over the model-supplied report.
/// `files` is a list of workspace-relative paths the model claims to have
/// delivered — the host reads each one (capped at 2MB), scans it for
/// emoji, and folds the findings into the result.
pub fn run_quality_gate(
    phase: ProjectPhase,
    files: &[String],
    line_count: usize,
    checks: &QualityGateChecks,
    config: &ProtocolConfig,
    file_reader: impl Fn(&str) -> Option<String>,
) -> QualityGateResult {
    let mut violations: Vec<String> = Vec::new();
    let mut emoji_offenders: Vec<(String, usize, String)> = Vec::new();

    if config.emoji_scan && !checks.no_emojis {
        violations.push(
            "VIOLATION: Emoji characters detected. Replace with Lucide/Phosphor icons."
                .to_owned(),
        );
    }
    if !checks.tests_included {
        violations.push(
            "VIOLATION: No tests delivered. Add unit/integration tests before proceeding."
                .to_owned(),
        );
    }
    if !checks.error_handling {
        violations.push(
            "VIOLATION: Missing error handling. Add try/catch, error boundaries, validation."
                .to_owned(),
        );
    }
    if !checks.loading_states {
        violations.push(
            "VIOLATION: Missing loading/empty states. Add skeletons, spinners, empty messages."
                .to_owned(),
        );
    }
    if !checks.type_safety {
        violations.push(
            "VIOLATION: Type safety compromised. Remove `any`/dynamic-typing escape hatches."
                .to_owned(),
        );
    }
    if !checks.documentation {
        violations.push(
            "VIOLATION: Documentation gap. README, API docs, or inline docstrings missing."
                .to_owned(),
        );
    }

    // Per-file emoji scan when we can read the file off disk.
    if config.emoji_scan {
        for path in files {
            if let Some(contents) = file_reader(path) {
                for (offset, ch) in scan_emojis(&contents) {
                    emoji_offenders.push((path.clone(), offset, ch.to_string()));
                }
            }
        }
        if !emoji_offenders.is_empty() {
            let preview: Vec<String> = emoji_offenders
                .iter()
                .take(5)
                .map(|(p, o, c)| format!("{p}@{o}: U+{:04X}", c.chars().next().unwrap_or('?') as u32))
                .collect();
            violations.push(format!(
                "VIOLATION: Emoji found in {} file location(s): {}",
                emoji_offenders.len(),
                preview.join(", ")
            ));
        }
    }

    // Line-count check is only hard at the FINAL phase.
    if phase == ProjectPhase::FinalValidation && line_count < config.min_line_count {
        violations.push(format!(
            "VIOLATION: Line count {line_count} is below {} minimum. Expand features, \
             tests, documentation, edge cases.",
            config.min_line_count
        ));
    } else if line_count < config.min_line_count / 10 {
        // Soft warning earlier in the pipeline so the model knows to keep going.
        violations.push(format!(
            "WARNING: Line count {line_count} is well below the {} target. Plan for \
             expansion before claiming FINAL_VALIDATION.",
            config.min_line_count
        ));
    }

    QualityGateResult {
        passed: violations.is_empty(),
        phase: phase.as_str().to_owned(),
        line_count,
        min_line_count: config.min_line_count,
        violations,
        emoji_offenders,
    }
}

/// Boolean bag the model hands to `quality_gate_check`. One field per
/// checklist item in the master prompt's Phase 6.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QualityGateChecks {
    #[serde(default)]
    pub no_emojis: bool,
    #[serde(default)]
    pub tests_included: bool,
    #[serde(default)]
    pub error_handling: bool,
    #[serde(default)]
    pub loading_states: bool,
    #[serde(default)]
    pub responsive_design: bool,
    #[serde(default)]
    pub accessibility: bool,
    #[serde(default)]
    pub type_safety: bool,
    #[serde(default)]
    pub documentation: bool,
    #[serde(default)]
    pub docker_included: bool,
    #[serde(default)]
    pub cicd_included: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_emojis_flags_pictographs_and_skips_ascii() {
        let hits = scan_emojis("hello world");
        assert!(hits.is_empty(), "ascii should be clean");
        let hits = scan_emojis("ship it \u{1F680}");
        assert_eq!(hits.len(), 1, "rocket is a single emoji");
        assert_eq!(hits[0].1 as u32, 0x1F680);
    }

    #[test]
    fn detect_phase_parses_both_marker_styles() {
        assert_eq!(
            detect_phase("Working on [Phase 2] architecture"),
            Some(ProjectPhase::ArchitectureRoadmap)
        );
        assert_eq!(
            detect_phase("PHASE 5: writing code"),
            Some(ProjectPhase::Implementation)
        );
        assert_eq!(detect_phase("no marker here"), None);
    }

    #[test]
    fn detect_phase_returns_last_marker() {
        let text = "[Phase 1] then later [Phase 4] for the dev plan";
        assert_eq!(detect_phase(text), Some(ProjectPhase::DevelopmentPlan));
    }

    #[test]
    fn premature_completion_heuristic() {
        assert!(looks_like_premature_completion("That's it, all done!"));
        assert!(looks_like_premature_completion("Here's the complete solution."));
        assert!(!looks_like_premature_completion("Phase 5 in progress, continuing"));
    }

    #[test]
    fn header_includes_user_prompt_verbatim() {
        let h = build_protocol_header("build a todo list", ProjectType::WebApp);
        assert!(h.contains("build a todo list"));
        assert!(h.contains("webapp"));
        assert!(h.contains("GOVINDA PROTOCOL ACTIVATED"));
    }

    #[test]
    fn all_checks_pass_yields_no_violations() {
        let cfg = ProtocolConfig::default();
        let checks = QualityGateChecks {
            no_emojis: true,
            tests_included: true,
            error_handling: true,
            loading_states: true,
            responsive_design: true,
            accessibility: true,
            type_safety: true,
            documentation: true,
            docker_included: true,
            cicd_included: true,
        };
        let result = run_quality_gate(
            ProjectPhase::FinalValidation,
            &[],
            12_000,
            &checks,
            &cfg,
            |_| None,
        );
        assert!(result.passed, "expected pass, got violations: {:?}", result.violations);
    }

    #[test]
    fn missing_required_checks_produce_violations() {
        let cfg = ProtocolConfig::default();
        let checks = QualityGateChecks::default();
        let result = run_quality_gate(
            ProjectPhase::Implementation,
            &[],
            1_000,
            &checks,
            &cfg,
            |_| None,
        );
        assert!(!result.passed);
        // at least emoji + tests + error_handling + loading + type + docs
        assert!(result.violations.len() >= 6, "got {:?}", result.violations);
    }

    #[test]
    fn line_count_is_hard_fails_at_final_only() {
        let cfg = ProtocolConfig::default();
        let checks = QualityGateChecks {
            no_emojis: true,
            tests_included: true,
            error_handling: true,
            loading_states: true,
            responsive_design: true,
            accessibility: true,
            type_safety: true,
            documentation: true,
            docker_included: true,
            cicd_included: true,
        };
        // Early phase with a line count ABOVE the soft-warning floor
        // (min/10 = 1000) — should pass without violations.
        let early = run_quality_gate(
            ProjectPhase::Implementation,
            &[],
            5_000,
            &checks,
            &cfg,
            |_| None,
        );
        assert!(early.passed, "early phase with 5k lines should pass, got {:?}", early.violations);
        // FINAL phase with the same count must hard-fail (5000 < 10000).
        let final_phase = run_quality_gate(
            ProjectPhase::FinalValidation,
            &[],
            5_000,
            &checks,
            &cfg,
            |_| None,
        );
        assert!(!final_phase.passed, "FINAL phase must hard-fail below threshold");
    }

    #[test]
    fn emoji_scan_reads_files_via_callback() {
        let cfg = ProtocolConfig {
            emoji_scan: true,
            ..ProtocolConfig::default()
        };
        let checks = QualityGateChecks {
            no_emojis: true,
            tests_included: true,
            error_handling: true,
            loading_states: true,
            responsive_design: true,
            accessibility: true,
            type_safety: true,
            documentation: true,
            docker_included: true,
            cicd_included: true,
        };
        let result = run_quality_gate(
            ProjectPhase::FinalValidation,
            &["src/x.rs".into()],
            12_000,
            &checks,
            &cfg,
            |_| Some("let x = \"hi \u{1F44B}\";".to_owned()),
        );
        assert!(!result.passed, "emoji in file should fail the gate");
        assert_eq!(result.emoji_offenders.len(), 1);
        assert!(result.violations.iter().any(|v| v.contains("Emoji found")));
    }
}
