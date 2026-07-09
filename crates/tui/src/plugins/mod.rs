#![allow(dead_code)]

use std::sync::{Mutex, OnceLock};

pub mod discovery;
pub mod manifest;
pub mod registry;

#[cfg(test)]
mod tests;

use discovery::discover_all;
use registry::PluginRegistry;

static REGISTRY: OnceLock<Mutex<PluginRegistry>> = OnceLock::new();

pub fn init_registry(builtin_dirs: &[&str]) {
    let registry = discover_all(builtin_dirs);
    REGISTRY.set(Mutex::new(registry)).ok();
}

pub fn try_with_registry<R>(f: impl FnOnce(&PluginRegistry) -> R) -> Option<R> {
    REGISTRY
        .get()
        .and_then(|lock| lock.lock().ok().map(|registry| f(&registry)))
}

pub fn with_registry<R>(f: impl FnOnce(&mut PluginRegistry) -> R) -> Option<R> {
    REGISTRY
        .get()
        .and_then(|lock| lock.lock().ok().map(|mut registry| f(&mut registry)))
}
