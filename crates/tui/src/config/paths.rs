//! Filesystem path resolution helpers for config/cache/workspace locations.
//!
//! Pure path-building helpers extracted verbatim from `config.rs`. They depend
//! only on `std`, `dirs`, and `shellexpand` plus one another, so they form a
//! clean leaf. `config.rs` pulls them back in (`use paths::{...}`) for the
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

pub(crate) fn default_config_path() -> Option<PathBuf> {
    env_config_path().or_else(home_config_path)
}

pub(crate) fn codewhale_home_dir() -> Option<PathBuf> {
    std::env::var_os("CODEWHALE_HOME").and_then(|path| {
        let path = PathBuf::from(path);
        (!path.as_os_str().is_empty()).then_some(path)
    })
}

pub(crate) fn home_config_path() -> Option<PathBuf> {
    if let Some(home) = codewhale_home_dir() {
        return Some(home.join("config.toml"));
    }

    effective_home_dir().map(|home| {
        let primary = home.join(".codewhale").join("config.toml");
        if primary.exists() {
            return primary;
        }
        let legacy = home.join(".deepseek").join("config.toml");
        if legacy.exists() {
            return legacy;
        }
        primary
    })
}

pub(crate) fn workspace_config_key(workspace: &Path) -> String {
    canonicalize_or_keep(workspace)
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn canonicalize_or_keep(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn env_config_path() -> Option<PathBuf> {
    #[cfg(test)]
    {
        crate::test_support::with_test_env_lock(env_config_path_unlocked)
    }
    #[cfg(not(test))]
    {
        env_config_path_unlocked()
    }
}

fn env_config_path_unlocked() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("CODEWHALE_CONFIG_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Some(expand_path(trimmed));
        }
    }
    if let Ok(path) = std::env::var("DEEPSEEK_CONFIG_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Some(expand_path(trimmed));
        }
    }
    None
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
    effective_home_dir().map(|home| home.join(".codewhale").join("skills"))
}

pub(crate) fn default_mcp_config_path() -> Option<PathBuf> {
    effective_home_dir().map(|home| {
        let primary = home.join(".codewhale").join("mcp.json");
        if primary.exists() {
            return primary;
        }
        let legacy = home.join(".deepseek").join("mcp.json");
        if legacy.exists() {
            return legacy;
        }
        primary
    })
}

pub(crate) fn default_notes_path() -> Option<PathBuf> {
    effective_home_dir().map(|home| {
        let primary = home.join(".codewhale").join("notes.txt");
        if primary.exists() {
            return primary;
        }
        let legacy = home.join(".deepseek").join("notes.txt");
        if legacy.exists() {
            return legacy;
        }
        primary
    })
}

pub(crate) fn default_memory_path() -> Option<PathBuf> {
    effective_home_dir().map(|home| {
        let primary = home.join(".codewhale").join("memory.md");
        if primary.exists() {
            return primary;
        }
        let legacy = home.join(".deepseek").join("memory.md");
        if legacy.exists() {
            return legacy;
        }
        primary
    })
}
