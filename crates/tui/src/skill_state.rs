//! Persistent enable/disable state for runtime API skill listings.
//!
//! Backs `GET /v1/skills` (`enabled` field per skill) and
//! `POST /v1/skills/{name}` (toggle). This is separate from the
//! filesystem-discovered `SkillRegistry`: the registry tells us which skills
//! exist on disk, and this store tells API clients which ones are marked active.
//!
//! Storage shape (TOML at `~/.codewhale/skills_state.toml`, legacy `~/.deepseek/skills_state.toml`):
//!
//! ```toml
//! disabled = ["skill-name-1", "skill-name-2"]
//! ```
//!
//! Default state when the file does not exist: empty list (everything enabled).
//! A corrupt file is logged and treated as the default, so upgrades never
//! accidentally hide every skill.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const STATE_FILE_NAME: &str = "skills_state.toml";

#[derive(Debug, Clone, Default)]
pub struct SkillStateStore {
    path: Option<PathBuf>,
    disabled: BTreeSet<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct OnDiskState {
    #[serde(default)]
    disabled: Vec<String>,
}

impl SkillStateStore {
    pub fn load_default() -> Result<Self> {
        let path = default_state_path()?;
        Self::load_from(path)
    }

    pub fn load_from(path: PathBuf) -> Result<Self> {
        if !path.exists() {
            return Ok(Self {
                path: Some(path),
                disabled: BTreeSet::new(),
            });
        }

        let raw = fs::read_to_string(&path)
            .with_context(|| format!("read skill state at {}", path.display()))?;
        let parsed: OnDiskState = match toml::from_str(&raw) {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(
                    "skills_state.toml at {} is malformed ({}); treating all skills as enabled",
                    path.display(),
                    err
                );
                OnDiskState::default()
            }
        };

        Ok(Self {
            path: Some(path),
            disabled: parsed.disabled.into_iter().collect(),
        })
    }

    pub fn is_enabled(&self, skill_name: &str) -> bool {
        !self.disabled.contains(skill_name)
    }

    pub fn set_enabled(&mut self, skill_name: &str, enabled: bool) -> Result<()> {
        let changed = if enabled {
            self.disabled.remove(skill_name)
        } else {
            self.disabled.insert(skill_name.to_string())
        };
        if !changed {
            return Ok(());
        }
        self.persist()
    }

    #[allow(dead_code)]
    pub fn disabled(&self) -> Vec<String> {
        self.disabled.iter().cloned().collect()
    }

    fn persist(&self) -> Result<()> {
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        let on_disk = OnDiskState {
            disabled: self.disabled.iter().cloned().collect(),
        };
        let body = toml::to_string_pretty(&on_disk).context("serialize skill state")?;
        atomic_write(path, body.as_bytes())
    }
}

fn default_state_path() -> Result<PathBuf> {
    let dir = codewhale_config::ensure_state_dir(".")
        .context("could not resolve or create Codewhale state directory")?;
    Ok(dir.join(STATE_FILE_NAME))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create parent dir for {}", path.display()))?;
    }
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, bytes).with_context(|| format!("write tmp at {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("rename tmp into {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh() -> (TempDir, SkillStateStore) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(STATE_FILE_NAME);
        let store = SkillStateStore::load_from(path).unwrap();
        (dir, store)
    }

    #[test]
    fn missing_file_defaults_to_everything_enabled() {
        let (_dir, store) = fresh();
        assert!(store.is_enabled("anything"));
        assert!(store.disabled().is_empty());
    }

    #[test]
    fn disable_then_reload_persists() {
        let (dir, mut store) = fresh();
        store.set_enabled("foo", false).unwrap();
        assert!(!store.is_enabled("foo"));

        let reloaded = SkillStateStore::load_from(dir.path().join(STATE_FILE_NAME)).unwrap();
        assert!(!reloaded.is_enabled("foo"));
        assert!(reloaded.is_enabled("bar"));
    }

    #[test]
    fn enable_removes_from_disabled_list() {
        let (_dir, mut store) = fresh();
        store.set_enabled("foo", false).unwrap();
        store.set_enabled("foo", true).unwrap();
        assert!(store.is_enabled("foo"));
        assert!(store.disabled().is_empty());
    }

    #[test]
    fn redundant_toggle_is_noop() {
        let (_dir, mut store) = fresh();
        store.set_enabled("foo", true).unwrap();
        assert!(store.disabled().is_empty());
    }

    #[test]
    fn malformed_file_falls_back_to_default() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(STATE_FILE_NAME);
        fs::write(&path, b"this is not toml = { broken").unwrap();
        let store = SkillStateStore::load_from(path).unwrap();
        assert!(store.is_enabled("anything"));
    }

    #[test]
    fn disabled_list_is_deterministic_order() {
        let (_dir, mut store) = fresh();
        store.set_enabled("zeta", false).unwrap();
        store.set_enabled("alpha", false).unwrap();
        store.set_enabled("mu", false).unwrap();
        assert_eq!(
            store.disabled(),
            vec!["alpha".to_string(), "mu".to_string(), "zeta".to_string()]
        );
    }
}
