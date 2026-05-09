//! Pure Rust runtime core for the planned Game TUI framework.
//!
//! This crate intentionally has no dependency on the TUI, ratatui, model
//! clients, shell execution, network clients, Python, or external game runtimes.

pub mod agents;
pub mod demo;
pub mod driver;
pub mod error;
pub mod interaction;
pub mod lookup;
pub mod manifest;
pub mod paths;
pub mod render;
pub mod save;
pub mod script;

pub use error::{GameError, Result};

#[cfg(test)]
mod tests;
