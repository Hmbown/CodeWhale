//! Behavior config key handlers (placeholder — keys served by legacy match fallback).

use crate::commands::shared::config_handlers::ConfigHandler;

pub fn handlers() -> Vec<&'static dyn ConfigHandler> {
    vec![]
}
