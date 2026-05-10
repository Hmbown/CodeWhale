//! Terminal UI (TUI) module for `DeepSeek` CLI.

// === Submodules ===

pub mod active_cell;
pub mod app;
pub mod approval;
pub mod backtrack;
pub mod clipboard;
mod color_compat;
pub mod command_palette;
pub mod context_inspector;
pub mod context_menu;
pub mod diff_render;
pub mod event_broker;
pub mod external_editor;
pub mod file_frecency;
pub mod file_mention;
pub mod file_picker;
pub mod file_tree;
pub mod frame_rate_limiter;
pub mod history;
pub mod keybindings;
pub mod live_transcript;
pub mod markdown_render;
mod mcp_routing;
pub mod model_picker;
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
mod subagent_routing;
mod tool_routing;
pub mod transcript;
pub mod transcript_cache;
pub mod translation;
pub mod ui;
mod ui_text;
pub mod user_input;
pub mod views;
pub mod widgets;

// === Re-exports ===

pub use app::TuiOptions;
pub use ui::run_tui;
