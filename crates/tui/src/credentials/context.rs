//! Injectable ambient-environment access for credential resolution.
//!
//! Ported from pi-mono `packages/ai/src/auth/context.ts` and the `AuthContext`
//! interface in `packages/ai/src/auth/types.ts` (MIT, Copyright (c) 2025 Mario
//! Zechner — full notice in the parent module).
//!
//! pi's motivation applies here unchanged: resolution that reads
//! `process.env` / `std::env` directly is untestable without mutating the real
//! process. Every ambient read the resolver performs itself goes through this
//! trait, so a test can state exactly which variables and files exist.

#[cfg(test)]
use std::collections::BTreeMap;
use std::path::Path;

/// Ambient environment access for credential resolution.
pub(crate) trait AuthContext: Send + Sync {
    /// Read an environment variable, treating blank values as unset — a blank
    /// `DEEPSEEK_API_KEY=` is a leftover export, not a credential.
    fn env(&self, name: &str) -> Option<String>;

    /// Whether a path exists. Used to report *which* credential file a
    /// provider would read, without opening or parsing it.
    fn file_exists(&self, path: &Path) -> bool;
}

/// The real process environment and filesystem.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ProcessAuthContext;

impl AuthContext for ProcessAuthContext {
    fn env(&self, name: &str) -> Option<String> {
        std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
    }

    fn file_exists(&self, path: &Path) -> bool {
        path.exists()
    }
}

/// Test double: a fixed set of variables and existing paths.
#[cfg(test)]
#[derive(Debug, Clone, Default)]
pub(crate) struct MapAuthContext {
    env: BTreeMap<String, String>,
    files: Vec<std::path::PathBuf>,
}

#[cfg(test)]
impl MapAuthContext {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn with_env(mut self, name: &str, value: &str) -> Self {
        self.env.insert(name.to_string(), value.to_string());
        self
    }

    pub(crate) fn with_file(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.files.push(path.into());
        self
    }
}

#[cfg(test)]
impl AuthContext for MapAuthContext {
    fn env(&self, name: &str) -> Option<String> {
        self.env
            .get(name)
            .filter(|value| !value.trim().is_empty())
            .cloned()
    }

    fn file_exists(&self, path: &Path) -> bool {
        self.files.iter().any(|candidate| candidate == path)
    }
}
