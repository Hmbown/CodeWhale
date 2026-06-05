//! Config key handlers — Strategy pattern for set_config_value.
//!
//! Each config key can have its own handler implementing [`ConfigHandler`].
//! Adding a handler for a key removes that arm from the big match in
//! `shared/config.rs`. Handlers are checked first; if none matches the
//! key, the original match handles it.
//!
//! To migrate a key:
//!   1. Create a struct implementing `ConfigHandler` for the key
//!   2. Add it to the appropriate `handlers()` function below
//!   3. Remove the arm from `set_config_value_match` in `shared/config.rs`

use std::sync::OnceLock;

use crate::commands::CommandResult;
use crate::tui::app::App;

/// A handler for a single config key.
pub trait ConfigHandler: Send + Sync {
    fn key(&self) -> &'static str;
    fn handle(&self, app: &mut App, value: &str, persist: bool) -> CommandResult;
}

/// Registry of registered config key handlers.
static REGISTRY: OnceLock<Vec<&'static dyn ConfigHandler>> = OnceLock::new();

fn registry() -> &'static [&'static dyn ConfigHandler] {
    REGISTRY.get_or_init(|| {
        let mut v: Vec<&'static dyn ConfigHandler> = Vec::new();
        v.append(&mut model::handlers());
        v.append(&mut display::handlers());
        v.append(&mut behavior::handlers());
        v.append(&mut editor::handlers());
        v.append(&mut misc::handlers());
        v
    })
}

/// Try to dispatch a config key via registered handlers.
/// Returns `None` if no handler matches (caller should fall through to the
/// legacy match in `shared/config.rs::set_config_value`).
pub fn handle_config(app: &mut App, key: &str, value: &str, persist: bool) -> Option<CommandResult> {
    for handler in registry() {
        if handler.key() == key {
            return Some(handler.handle(app, value, persist));
        }
    }
    None
}

// ── Handler group modules (placeholders for incremental migration) ─────────

mod model;
mod display;
mod behavior;
mod editor;
mod misc;
