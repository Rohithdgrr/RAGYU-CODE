// Tests legitimately unwrap/expect to assert panics.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

/// Serializes tests that mutate the process working directory (cargo runs
/// tests in parallel threads and cwd is process-global).
#[cfg(test)]
pub static TEST_CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub mod agent_loop;
pub mod api;
pub mod audit;
pub mod auto_compact;
pub mod checkpoint;
pub mod clock;
pub mod commands;
pub mod completions;
pub mod config;
pub mod context;
pub mod diff;
pub mod git;
pub mod govinda_protocol;
pub mod hooks;
pub mod ignore;
pub mod lsp;
pub mod memory;
pub mod model_rank;
pub mod omniroute;
pub mod opencode;
pub mod outline;
pub mod preflight;
pub mod preview;
pub mod project;
pub mod prompt_cache;
pub mod provider;
pub mod rag;
pub mod render;
pub mod router;
pub mod router_health;
pub mod scan;
pub mod session;
pub mod sessions;
pub mod skills;
pub mod ssrf;
pub mod swarm;
pub mod symbols;
pub mod tokens;
pub mod toolbox;
pub mod tools;
pub mod tui;
