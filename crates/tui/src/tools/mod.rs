//! Tool system modules and re-exports.

// Tools run inside the TUI alt-screen runtime. Raw `print!` / `eprintln!`
// inside this module tree leaks into ratatui's diff-renderer buffer and
// produces the "scroll demon" regression (#1085 / v0.8.27 follow-up).
// Route status/error reporting through `tracing::*` instead — the
// `runtime_log` subscriber captures it to `~/.deepseek/logs/`.
#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]

pub mod apply_patch;
pub mod approval_cache;
pub mod arg_repair;
pub mod automation;
pub mod diagnostics;
pub mod diff_format;
pub mod file;
pub mod file_search;
pub mod finance;

pub mod fetch_url;
pub mod fim;
pub mod git;
pub mod git_history;
pub mod github;
pub mod large_output_router;
pub mod notify;
pub mod parallel;
pub mod plan;
pub mod project;
pub mod recall_archive;
pub mod registry;
pub mod remember;
pub mod revert_turn;
pub mod review;
pub mod rlm;
pub mod schema_sanitize;
pub mod search;
pub mod shell;
mod shell_output;
pub mod skill;
pub mod spec;
pub mod subagent;
pub mod tasks;
pub mod test_runner;
pub mod todo;
pub mod tool_result_retrieval;
pub mod truncate;
pub mod user_input;
pub mod validate_data;
pub mod web_run;
pub mod web_search;

pub use registry::{ToolRegistry, ToolRegistryBuilder};
pub use review::ReviewOutput;
pub use spec::ToolContext;
pub use user_input::UserInputResponse;
