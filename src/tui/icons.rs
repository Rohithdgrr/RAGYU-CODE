//! Icon system — Nerd Font glyphs (no emoji).
//!
//! Every glyph lives in the Nerd Fonts private-use areas. The UI targets
//! terminals running a patched Nerd Font; the typography stack it is designed
//! against is:
//!
//! - Headings / display ....... Space Grotesk  (500–700)
//! - Body / UI text ........... DM Sans or Manrope (400–600)
//! - Numbers / data / code .... JetBrains Mono
//!
//! Terminals render with whatever font the user configured, so keep labels
//! uppercase + letterspaced for headings and let the terminal's mono handle
//! numerals — the icons below carry the recognition load.

/// Brand mark — sharp diamond, echoes the cut-glass edges.
pub const LOGO: &str = "\u{f219}";

// ── Mode chips ──────────────────────────────────────────────────────────────
pub const MODE_READY: &str = "\u{f058}"; // check-circle
pub const MODE_AGENT: &str = "\u{f135}"; // rocket
pub const MODE_REVIEW: &str = "\u{f071}"; // warning-triangle
pub const MODE_PLAN: &str = "\u{f0cb}"; // ordered-list

// ── Status bar ──────────────────────────────────────────────────────────────
pub const GIT_BRANCH: &str = "\u{f418}"; // git-branch
pub const DIRTY_DOT: &str = "\u{f111}"; // circle (filled)
pub const MODEL_CHIP: &str = "\u{f2db}"; // microchip
pub const TOKENS: &str = "\u{f0e7}"; // bolt
pub const TURNS: &str = "\u{f021}"; // refresh
pub const LATENCY: &str = "\u{f017}"; // clock
pub const TOOLS: &str = "\u{f013}"; // gear
pub const GATED: &str = "\u{f024}"; // flag
pub const PINNED: &str = "\u{f08d}"; // pin
pub const ERRORS: &str = "\u{f057}"; // times-circle
pub const LIVE: &str = "\u{f012}"; // signal

// ── Chat chips ──────────────────────────────────────────────────────────────
pub const USER: &str = "\u{f007}"; // user
pub const ASSISTANT: &str = "\u{f0d0}"; // magic wand (sparkles)
pub const THINKING: &str = "\u{f1ce}"; // circle-o-notch

// ── Tool status ─────────────────────────────────────────────────────────────
pub const CHECK: &str = "\u{f00c}";
pub const CROSS: &str = "\u{f00d}";
pub const PENDING: &str = "\u{f252}"; // hourglass-half
pub const LOCKED: &str = "\u{f023}"; // lock

// ── Panes / chrome ──────────────────────────────────────────────────────────
pub const FOLDER: &str = "\u{f07b}";
pub const FOLDER_OPEN: &str = "\u{f07c}";
pub const FILE: &str = "\u{f016}";
pub const FILE_CODE: &str = "\u{f1c9}";
pub const TREE_TITLE: &str = "\u{f418}"; // git-branch doubles as project tree
pub const FILES_TITLE: &str = "\u{f022}"; // list-alt
pub const COMMANDS_TITLE: &str = "\u{f120}"; // terminal
pub const SEND: &str = "\u{f1d8}"; // paper-plane
pub const INFO: &str = "\u{f05a}"; // info-circle
pub const WARNING: &str = "\u{f071}";
pub const SUCCESS: &str = "\u{f058}"; // check-circle

// ── File-type glyphs (dev-icon range) ───────────────────────────────────────
pub fn file_icon(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("");
    match ext {
        "rs" => "\u{e7a8}",
        "py" => "\u{e606}",
        "js" | "mjs" | "cjs" => "\u{e74e}",
        "ts" | "tsx" => "\u{e628}",
        "html" => "\u{e736}",
        "css" | "scss" => "\u{e749}",
        "md" => "\u{e73e}",
        "json" | "toml" | "yaml" | "yml" => "\u{e60b}",
        "lock" => "\u{f023}",
        "sh" | "ps1" | "cmd" | "bat" => "\u{f120}",
        "exe" | "dll" | "pdb" | "rlib" | "rmeta" | "d" => "\u{f2d0}", // cog-boxed binary
        _ => FILE,
    }
}

/// Maps a slash command to its icon. Kept in sync with
/// `crate::commands::SLASH_COMMANDS` so every palette row gets a meaningful,
/// fast-to-recognize glyph.
pub fn command(cmd: &str) -> &'static str {
    match cmd {
        "/help" => "\u{f059}",     // question-circle
        "/exit" | "/quit" | "/q" => "\u{f08b}", // sign-out
        "/clear" | "/reset" => "\u{f014}", // trash
        "/models" => "\u{f2db}",   // microchip list
        "/model" => "\u{f2db}",    // microchip
        "/temp" => "\u{f2c9}",     // thermometer-half
        "/system" => "\u{f120}",   // terminal
        "/history" => "\u{f1da}",  // history
        "/undo" => "\u{f0e2}",     // undo arrow
        "/retry" => "\u{f021}",    // refresh
        "/variants" => "\u{f141}", // ellipsis-h
        "/pick" => "\u{f245}",     // hand-pointer
        "/compact" => "\u{f066}",  // compress
        "/search" => "\u{f002}",   // search
        "/save" => "\u{f0c7}",     // save
        "/load" => "\u{f019}",     // download
        "/sessions" => "\u{f1c0}", // database
        "/fork" => "\u{f126}",     // code-branch
        "/export" => "\u{f093}",   // upload
        "/stats" => "\u{f080}",    // chart-bar
        "/theme" => "\u{f1fc}",    // paint-brush
        "/tokens" => "\u{f0e7}",   // bolt
        "/raw" => "\u{f070}",      // eye-slash
        "/config" => "\u{f0ad}",   // wrench
        "/timeout" => "\u{f252}",  // hourglass-half
        "/limit" => "\u{f0e4}",    // tachometer
        "/tools" => "\u{f0ad}",    // wrench
        "/todo" => "\u{f046}",     // check-square-o
        "/diff" => "\u{f0db}",     // columns
        "/apply" => "\u{f00c}",    // check
        "/reject" => "\u{f00d}",   // times
        "/review" => "\u{f06e}",   // eye
        "/scan" => "\u{f00e}",     // search-plus
        "/plan" => "\u{f0cb}",     // list-ol
        "/project" => "\u{f187}",  // archive
        "/checkpoint" => "\u{f0c7}", // save
        "/rewind" => "\u{f04a}",    // backward
        "/memory" => "\u{f040}",     // at
        "/skills" => "\u{f0e6}",    // comments
        "/commit" => "\u{f0c1}",    // link
        "/pr" => "\u{f126}",        // code-fork
        "/pty" => "\u{f120}",       // terminal
        "/auto-compact" => "\u{f0e7}", // bolt
        "/pin" => PINNED,
        "/agent" => MODE_AGENT,
        _ => INFO,
    }
}
