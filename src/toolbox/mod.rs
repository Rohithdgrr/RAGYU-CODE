//! The GOVINDA "toolbox" — high-leverage tools that make the agent more
//! efficient, rich, stable, and engaging.
//!
//! Each tool lives in its own focused module so they can be developed and
//! tested independently. The [`registry`] module wires them all into the
//! tool executor with consistent argument parsing, error handling, and
//! confirmation gating.

pub mod clipboard;
pub mod template;
pub mod package;
pub mod lint;
pub mod formatter;
pub mod build;
pub mod http;
pub mod process;
pub mod env;
pub mod jsonquery;
pub mod regex_search;
pub mod diff_apply;
pub mod template_fill;
pub mod git_diff_apply;
pub mod scaffold_test;
pub mod memory_search;
pub mod screenshot;
pub mod image_view;
pub mod bulk_read;
pub mod bulk_shell;
pub mod bulk_crud;
pub mod docs;
pub mod html;
pub mod screenrec;
pub mod format_setter;
pub mod code_format;
pub mod git_tools;
pub mod find_issues;
pub mod parallel;
pub mod registry;
