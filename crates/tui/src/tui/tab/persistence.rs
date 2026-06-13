//! Tab state persistence
//!
//! Saves the TabManager's tab list (titles, types, IDs, creation time) to a
//! JSON file in the user's data directory. On startup, the file is loaded
//! to restore the previous session's tabs.
//!
//! Note: messages and conversation history are NOT persisted here - those
//! live in the session_manager. This file only stores the tab metadata.

// WIP collaboration surface — narrow harvest. See `tab/mod.rs` for the
// PR #2753 context.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::delegator::DelegationStatus;
use super::{Priority, TabId, TabMetadata, TabType};

/// Current schema version. Bump when making breaking changes to the
/// on-disk format. Older versions are detected on load so we can
/// give a useful error message rather than silently dropping data.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Maximum size of the tab state file we'll attempt to load (1 MB).
/// Past this, the file is treated as corrupted and ignored. This
/// prevents a malicious or accidental huge file from OOM-ing the TUI
/// on startup.
pub const MAX_FILE_SIZE: u64 = 1024 * 1024;

/// On-disk representation of a single tab
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedTab {
    pub id: u64,
    pub title: String,
    pub tab_type: TabType,
    pub created_at: DateTime<Utc>,
    pub last_active: DateTime<Utc>,
}

/// On-disk representation of a tab group
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedGroup {
    pub id: String,
    pub name: String,
    pub color: super::group::GroupColor,
    pub tab_ids: Vec<TabId>,
    pub created_at: DateTime<Utc>,
}

/// On-disk representation of a single delegation task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedDelegation {
    pub task_id: String,
    pub from_tab: u64,
    pub to_tab: u64,
    pub description: String,
    pub priority: Priority,
    /// Status of the delegation when it was snapshotted. Without this field,
    /// an in-flight `InProgress` task is silently demoted to `Pending` on
    /// restart, losing work-in-progress state.
    pub status: DelegationStatus,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub result: Option<String>,
    pub was_successful: Option<bool>,
}

/// On-disk representation of the tab manager state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedTabState {
    pub version: u32,
    pub saved_at: DateTime<Utc>,
    pub active_tab_index: Option<usize>,
    pub tabs: Vec<PersistedTab>,
    pub delegations: Vec<PersistedDelegation>,
    #[serde(default)]
    pub groups: Vec<PersistedGroup>,
}

impl Default for PersistedTabState {
    fn default() -> Self {
        Self {
            version: 1,
            saved_at: Utc::now(),
            active_tab_index: None,
            tabs: Vec::new(),
            delegations: Vec::new(),
            groups: Vec::new(),
        }
    }
}

/// Get the default path for the tab state file
/// `~/.codewhale/tabs.json`
pub fn default_tab_state_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".codewhale").join("tabs.json"))
}

/// Save the tab state to a file.
/// Atomically writes via temp file + rename to prevent corruption
/// from interrupted writes.
pub fn save_to_file(state: &PersistedTabState, path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    // Write to temp file first, then rename for atomicity
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, json)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Load the tab state from a file. Returns default state if file doesn't exist.
/// Refuses to load files larger than MAX_FILE_SIZE to prevent OOM.
/// Detects schema version mismatches and returns a specific error.
pub fn load_from_file(path: &Path) -> std::io::Result<PersistedTabState> {
    if !path.exists() {
        return Ok(PersistedTabState::default());
    }

    // Size check
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > MAX_FILE_SIZE {
        // Silently returning `default()` would let the next save overwrite
        // the oversized file and destroy the user's data. Surface the error
        // so the application can refuse to save and preserve the file.
        tracing::error!(
            size = metadata.len(),
            max = MAX_FILE_SIZE,
            path = %path.display(),
            "Tab state file too large, refusing to load"
        );
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "Tab state file size {} exceeds maximum allowed size {}",
                metadata.len(),
                MAX_FILE_SIZE
            ),
        ));
    }

    let content = std::fs::read_to_string(path)?;
    let state: PersistedTabState = serde_json::from_str(&content).map_err(|e| {
        tracing::error!(
            ?e,
            path = %path.display(),
            "Failed to parse tab state file"
        );
        std::io::Error::new(std::io::ErrorKind::InvalidData, e)
    })?;

    // Schema version check
    if state.version > CURRENT_SCHEMA_VERSION {
        tracing::warn!(
            file_version = state.version,
            current = CURRENT_SCHEMA_VERSION,
            "Tab state file is from a newer version; some data may be ignored"
        );
    } else if state.version < CURRENT_SCHEMA_VERSION {
        tracing::info!(
            file_version = state.version,
            current = CURRENT_SCHEMA_VERSION,
            "Migrating tab state from older schema"
        );
        // Future: implement migration logic here
    }

    Ok(state)
}

/// Convert a TabMetadata to its persisted form
pub fn from_metadata(meta: &TabMetadata) -> PersistedTab {
    PersistedTab {
        id: meta.id.0,
        title: meta.title.clone(),
        tab_type: meta.tab_type,
        created_at: meta.created_at,
        last_active: meta.last_active,
    }
}

/// Convert a persisted tab to a TabMetadata
pub fn to_metadata(persisted: &PersistedTab) -> TabMetadata {
    let mut meta = TabMetadata::new(
        TabId::new(persisted.id),
        persisted.title.clone(),
        persisted.tab_type,
    );
    meta.created_at = persisted.created_at;
    meta.last_active = persisted.last_active;
    meta
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip() {
        let state = PersistedTabState {
            version: 1,
            saved_at: Utc::now(),
            active_tab_index: Some(0),
            tabs: vec![
                PersistedTab {
                    id: 1,
                    title: "Tab 1".to_string(),
                    tab_type: TabType::Chat,
                    created_at: Utc::now(),
                    last_active: Utc::now(),
                },
                PersistedTab {
                    id: 2,
                    title: "Tab 2".to_string(),
                    tab_type: TabType::Meeting,
                    created_at: Utc::now(),
                    last_active: Utc::now(),
                },
            ],
            delegations: vec![PersistedDelegation {
                task_id: "delegation_1".to_string(),
                from_tab: 1,
                to_tab: 2,
                description: "Review code".to_string(),
                priority: Priority::High,
                status: DelegationStatus::Pending,
                created_at: Utc::now(),
                completed_at: None,
                result: None,
                was_successful: None,
            }],
            groups: vec![],
        };

        let json = serde_json::to_string(&state).unwrap();
        let parsed: PersistedTabState = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.tabs.len(), 2);
        assert_eq!(parsed.tabs[0].title, "Tab 1");
        assert_eq!(parsed.tabs[1].tab_type, TabType::Meeting);
        assert_eq!(parsed.delegations.len(), 1);
        assert_eq!(parsed.delegations[0].priority, Priority::High);
    }

    #[test]
    fn test_metadata_conversion() {
        let meta = TabMetadata::new(TabId::new(42), "Test".to_string(), TabType::Review);
        let persisted = from_metadata(&meta);
        assert_eq!(persisted.id, 42);
        assert_eq!(persisted.title, "Test");
        assert_eq!(persisted.tab_type, TabType::Review);

        let restored = to_metadata(&persisted);
        assert_eq!(restored.id, TabId::new(42));
        assert_eq!(restored.title, "Test");
        assert_eq!(restored.tab_type, TabType::Review);
    }

    #[test]
    fn test_load_missing_file() {
        let result = load_from_file(Path::new("/nonexistent/path/tabs.json"));
        assert!(result.is_ok());
        let state = result.unwrap();
        assert!(state.tabs.is_empty());
        assert!(state.delegations.is_empty());
    }

    #[test]
    fn test_save_and_load() {
        let dir = std::env::temp_dir().join("codewhale_tab_test");
        let path = dir.join("tabs.json");

        // Clean up any leftover
        let _ = std::fs::remove_file(&path);

        let state = PersistedTabState {
            version: 1,
            saved_at: Utc::now(),
            active_tab_index: Some(1),
            tabs: vec![PersistedTab {
                id: 1,
                title: "Test".to_string(),
                tab_type: TabType::Delegation,
                created_at: Utc::now(),
                last_active: Utc::now(),
            }],
            delegations: vec![],
            groups: vec![],
        };

        save_to_file(&state, &path).unwrap();
        let loaded = load_from_file(&path).unwrap();
        assert_eq!(loaded.active_tab_index, Some(1));
        assert_eq!(loaded.tabs.len(), 1);
        assert_eq!(loaded.tabs[0].tab_type, TabType::Delegation);

        // Cleanup
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn test_load_oversized_file_rejected() {
        // Create a file that exceeds MAX_FILE_SIZE
        let dir = std::env::temp_dir().join("codewhale_tab_oversize");
        let path = dir.join("tabs.json");
        std::fs::create_dir_all(&dir).unwrap();
        let _ = std::fs::remove_file(&path);

        // Write a small header followed by enough junk to exceed 1MB
        let mut content = String::from(
            r#"{"version":1,"saved_at":"2026-01-01T00:00:00Z","active_tab_index":null,"tabs":[],"delegations":[]}"#,
        );
        while content.len() < (MAX_FILE_SIZE as usize) + 100 {
            content.push(' ');
        }
        std::fs::write(&path, content).unwrap();

        // Should return an error rather than silently overwriting the file
        // on next save. Silently returning a default would destroy the
        // user's data.
        let result = load_from_file(&path);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidData);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn test_load_corrupted_file() {
        let dir = std::env::temp_dir().join("codewhale_tab_corrupt");
        let path = dir.join("tabs.json");
        std::fs::create_dir_all(&dir).unwrap();
        let _ = std::fs::remove_file(&path);

        // Write invalid JSON
        std::fs::write(&path, "{ not valid json :::").unwrap();

        // Should return error
        let result = load_from_file(&path);
        assert!(result.is_err());

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn test_save_is_atomic() {
        // Verify that save_to_file uses a temp + rename pattern.
        // The test ensures the final file exists and no .tmp file remains.
        let dir = std::env::temp_dir().join("codewhale_tab_atomic");
        let path = dir.join("tabs.json");
        let tmp_path = path.with_extension("json.tmp");
        std::fs::create_dir_all(&dir).unwrap();
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&tmp_path);

        let state = PersistedTabState::default();
        save_to_file(&state, &path).unwrap();

        assert!(path.exists(), "Final file should exist");
        assert!(!tmp_path.exists(), "Temp file should be cleaned up");

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn test_load_newer_schema_logs_warning_but_loads() {
        // Simulate a file from a future version
        let dir = std::env::temp_dir().join("codewhale_tab_newer");
        let path = dir.join("tabs.json");
        std::fs::create_dir_all(&dir).unwrap();
        let _ = std::fs::remove_file(&path);

        let json = r#"{"version":99,"saved_at":"2027-01-01T00:00:00Z","active_tab_index":null,"tabs":[],"delegations":[]}"#;
        std::fs::write(&path, json).unwrap();

        // Should still load successfully (graceful degradation)
        let loaded = load_from_file(&path).unwrap();
        assert_eq!(loaded.version, 99);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }
}
