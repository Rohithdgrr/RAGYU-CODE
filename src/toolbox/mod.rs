//! The GOVINDA "toolbox" — high-leverage tools that make the agent more
//! efficient, rich, stable, and engaging.
//!
//! Each tool lives in its own focused module so they can be developed and
//! tested independently. The [`registry`] module wires them all into the
//! tool executor with consistent argument parsing, error handling, and
//! confirmation gating.

pub mod build;
pub mod bulk_crud;
pub mod bulk_read;
pub mod bulk_shell;
pub mod clipboard;
pub mod code_format;
pub mod diff_apply;
pub mod docs;
pub mod env;
pub mod find_issues;
pub mod format_setter;
pub mod formatter;
pub mod git_diff_apply;
pub mod git_tools;
pub mod html;
pub mod http;
pub mod image_view;
pub mod jsonquery;
pub mod lint;
pub mod memory_search;
pub mod package;
pub mod parallel;
pub mod process;
pub mod regex_search;
pub mod registry;
pub mod scaffold_test;
pub mod screenrec;
pub mod screenshot;
pub mod template;
pub mod template_fill;
