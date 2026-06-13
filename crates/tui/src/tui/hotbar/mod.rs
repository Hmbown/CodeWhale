//! Hotbar action registry foundation.
//!
//! Later hotbar slices add config, sidebar rendering, and key dispatch. This
//! module only defines the action surface and the built-in actions that those
//! layers will consume.

pub mod actions;

pub use actions::HotbarActionRegistry;
