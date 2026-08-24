// Tests legitimately unwrap/expect to assert panics.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

/// Serializes tests that mutate the process working directory (cargo runs
/// tests in parallel threads and cwd is process-global).
#[cfg(test)]
pub static TEST_CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub mod api;
pub mod agent_loop;
pub mod clock;
pub mod commands;
pub mod completions;
pub mod config;
pub mod context;
pub mod diff;
pub mod git;
pub mod hooks;
pub mod ignore;
pub mod lsp;
pub mod outline;
pub mod preview;
pub mod project;
pub mod rag;
pub mod provider;
pub mod render;
pub mod scan;
pub mod session;
pub mod sessions;
pub mod skills;
pub mod swarm;
pub mod symbols;
pub mod tokens;
pub mod tools;
pub mod tui;
pub mod memory;
pub mod checkpoint;
