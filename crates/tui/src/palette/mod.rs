//! DeepSeek color palette and semantic roles.
//!
//! This module defines the color system for the TUI in three layers:
//!
//! 1. **RGB tuples** (`*_RGB` constants) — raw color values used by theme
//!    generation and runtime palette construction.
//! 2. **Semantic `Color` constants** — pre-computed `ratatui::style::Color`
//!    values mapped to UI roles (surface, text, accent, status, mode).
//! 3. **Backward-compatible aliases** (`DEEPSEEK_*`) — legacy names that
//!    delegate to the current Whale palette constants.

mod adapt;
mod detect;
mod themes;
mod tokens;

#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub use adapt::*;
#[allow(unused_imports)]
pub use detect::*;
#[allow(unused_imports)]
pub use themes::*;
pub use tokens::*;
