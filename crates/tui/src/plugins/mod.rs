pub mod injection;
pub mod registry;

mod discovery;
mod manifest;

pub use discovery::discover_all;
pub use injection::render_plugin_block;
#[allow(unused_imports)]
pub use manifest::{LoadedPlugin, PluginManifest, PluginSource};
pub use registry::PluginRegistry;

use std::sync::{Mutex, OnceLock};

static PLUGIN_REGISTRY: OnceLock<Mutex<PluginRegistry>> = OnceLock::new();

pub fn init_registry(config_disabled: &[String]) {
    let _ = PLUGIN_REGISTRY.set(Mutex::new(discover_all(config_disabled)));
}

pub fn with_registry<F, R>(f: F) -> R
where
    F: FnOnce(&PluginRegistry) -> R,
{
    let lock = PLUGIN_REGISTRY
        .get()
        .expect("PluginRegistry not initialized — call init_registry() first");
    let guard = lock.lock().expect("PluginRegistry lock poisoned");
    f(&*guard)
}

pub fn try_with_registry<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&PluginRegistry) -> R,
{
    let lock = PLUGIN_REGISTRY.get()?;
    let guard = lock.lock().ok()?;
    Some(f(&*guard))
}

pub fn with_registry_mut<F, R>(f: F) -> R
where
    F: FnOnce(&mut PluginRegistry) -> R,
{
    let lock = PLUGIN_REGISTRY
        .get()
        .expect("PluginRegistry not initialized — call init_registry() first");
    let mut guard = lock.lock().expect("PluginRegistry lock poisoned");
    f(&mut *guard)
}

#[cfg(test)]
#[path = "tests.rs"]
mod integration_tests;
