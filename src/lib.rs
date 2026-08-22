// Tests legitimately unwrap/expect to assert panics.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod api;
pub mod clock;
pub mod commands;
pub mod completions;
pub mod config;
pub mod context;
pub mod diff;
pub mod git;
pub mod ignore;
pub mod outline;
pub mod provider;
pub mod render;
pub mod scan;
pub mod session;
pub mod sessions;
pub mod symbols;
pub mod tokens;
pub mod tools;
