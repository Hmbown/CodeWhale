//! Terminal UI (TUI) module for `DeepSeek` CLI.

// The rendering layer runs inside the alt-screen. Raw stdio prints
// produce the scroll demon (see `runtime_log` for full context). Use
// `tracing::*` for diagnostics — `runtime_log` captures it to disk.
// `ui::run_event_loop` legitimately prints a post-exit resume hint
// AFTER `LeaveAlternateScreen`; that single site uses
// `#[allow(clippy::print_stdout)]` locally.
#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]

// === Submodules ===

pub mod active_cell;
pub mod app;
pub mod approval;
pub mod auto_router;
pub mod backtrack;
pub mod clipboard;
pub mod color_compat;
pub mod command_palette;
pub mod composer_ui;
pub mod context_inspector;
pub mod context_menu;
pub mod diff_render;
pub mod event_broker;
pub mod external_editor;
pub mod feedback_picker;
pub mod file_frecency;
pub mod file_mention;
pub mod file_picker;
pub mod file_picker_relevance;
pub mod file_tree;
pub mod footer_ui;
pub mod format_helpers;
pub mod frame_rate_limiter;
pub mod history;
pub mod key_shortcuts;
pub mod keybindings;
pub mod live_transcript;
pub mod markdown_render;
mod mcp_routing;
pub mod model_picker;
pub mod mouse_ui;
pub mod notifications;
pub mod onboarding;
pub mod osc8;
pub mod pager;
pub mod paste;
pub mod paste_burst;
pub mod persistence_actor;
pub mod plan_prompt;
pub mod provider_picker;
pub mod scrolling;
pub mod selection;
pub mod session_picker;
mod shell_job_routing;
pub mod sidebar;
pub mod slash_menu;
pub mod streaming;
pub mod streaming_thinking;
mod subagent_routing;
pub mod theme_picker;
mod tool_routing;
pub mod transcript;
pub mod transcript_cache;
pub mod translation;
pub mod ui;
mod ui_text;
pub mod user_input;
pub mod views;
pub mod vim_mode;
pub mod widgets;
pub mod workspace_context;

// === Re-exports ===

pub use app::TuiOptions;
pub use ui::run_tui;
