//! Filesystem path resolution helpers for config/cache/workspace locations.
//!
//! Pure path-building helpers extracted verbatim from `config.rs`. They depend
//! only on `std`, `codewhale-paths`, and `shellexpand` plus one another, so they
//! form a clean leaf. `config.rs` pulls them back in (`use paths::{...}`) for the
//! workspace-trust and config-loading logic that stays there, and re-exports
//! the two `pub(crate)` entry points (`effective_home_dir`, `expand_path`) so
//! external `crate::config::` callers resolve unchanged (#3311).
//!
//! Visibility note: helpers that were file-private `fn` in `config.rs` are
//! `pub(crate)` here purely so the parent module can name them; none are
//! re-exported publicly, so the crate's external surface is unchanged.

use std::path::{Path, PathBuf};

/// Re-exported so `config::effective_home_dir` and every `use paths::{...}`
/// caller resolve unchanged. It lives in `home.rs` because this module is not
/// includable from an integration test binary — see that file's header.
pub(crate) use super::home::effective_home_dir;

pub(crate) fn default_config_path() -> anyhow::Result<PathBuf> {
    try_default_config_path()
}

pub(crate) fn try_default_config_path() -> anyhow::Result<PathBuf> {
    #[cfg(test)]
    {
        let honor_guarded_environment = crate::test_support::current_thread_holds_test_env_lock();
        crate::test_support::with_test_env_lock(|| {
            if honor_guarded_environment {
                try_default_config_path_from_environment()
            } else {
                Ok(crate::test_support::isolated_test_state_root()
                    .join(codewhale_config::CONFIG_FILE_NAME))
            }
        })
    }

    #[cfg(not(test))]
    try_default_config_path_from_environment()
}

fn try_default_config_path_from_environment() -> anyhow::Result<PathBuf> {
    codewhale_config::resolve_config_path(None)
}

pub(crate) fn codewhale_home_dir() -> Result<Option<PathBuf>, codewhale_paths::PathOverrideError> {
    codewhale_paths::codewhale_home_override()
}

pub(crate) fn workspace_config_key(workspace: &Path) -> String {
    canonicalize_or_keep(workspace)
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn canonicalize_or_keep(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn env_config_path() -> Result<Option<PathBuf>, codewhale_paths::PathOverrideError> {
    #[cfg(test)]
    {
        crate::test_support::with_test_env_lock(env_config_path_unlocked)
    }
    #[cfg(not(test))]
    {
        env_config_path_unlocked()
    }
}

fn env_config_path_unlocked() -> Result<Option<PathBuf>, codewhale_paths::PathOverrideError> {
    codewhale_paths::config_path_override()
}

pub(crate) fn expand_pathbuf(path: PathBuf) -> PathBuf {
    if let Some(raw) = path.to_str() {
        return expand_path(raw);
    }
    path
}

pub(crate) fn default_managed_config_path() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        Some(PathBuf::from("/etc/deepseek/managed_config.toml"))
    }
    #[cfg(not(unix))]
    {
        effective_home_dir().map(|home| {
            let primary = home.join(".codewhale").join("managed_config.toml");
            if primary.exists() {
                return primary;
            }
            home.join(".deepseek").join("managed_config.toml")
        })
    }
}

pub(crate) fn default_requirements_path() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        Some(PathBuf::from("/etc/deepseek/requirements.toml"))
    }
    #[cfg(not(unix))]
    {
        effective_home_dir().map(|home| {
            let primary = home.join(".codewhale").join("requirements.toml");
            if primary.exists() {
                return primary;
            }
            home.join(".deepseek").join("requirements.toml")
        })
    }
}

pub(crate) fn expand_path(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix('~')
        && (stripped.is_empty() || stripped.starts_with('/') || stripped.starts_with('\\'))
        && let Some(mut home) = effective_home_dir()
    {
        let suffix = stripped.trim_start_matches(['/', '\\']);
        if !suffix.is_empty() {
            home.push(suffix);
        }
        return home;
    }

    let expanded = shellexpand::tilde(path);
    PathBuf::from(expanded.as_ref())
}

pub(crate) fn default_skills_dir() -> Option<PathBuf> {
    default_user_state_path("skills")
}

pub(crate) fn default_mcp_config_path() -> Option<PathBuf> {
    default_user_state_path("mcp.json")
}

pub(crate) fn default_notes_path() -> Option<PathBuf> {
    default_user_state_path("notes.txt")
}

pub(crate) fn default_memory_path() -> Option<PathBuf> {
    default_user_state_path("memory.md")
}

fn default_user_state_path(name: &str) -> Option<PathBuf> {
    match codewhale_home_dir() {
        Ok(Some(home)) => return Some(home.join(name)),
        Ok(None) => {}
        Err(error) => {
            tracing::error!(
                error = %error,
                "invalid Codewhale home override; refusing to substitute a different state root"
            );
            return None;
        }
    }
    effective_home_dir().map(|home| {
        let primary = home.join(".codewhale").join(name);
        if primary.exists() {
            return primary;
        }
        let legacy = home.join(".deepseek").join(name);
        if legacy.exists() {
            return legacy;
        }
        primary
    })
}
