//! Cross-session persistence for the immutable base section of the system
//! prompt.
//!
//! ## Why
//!
//! DeepSeek's KV prefix cache matches byte sequences from the start of the
//! system prompt. The base section (mode prompt, project context, skills,
//! context management, compaction template) is stable across sessions for
//! the same workspace. By caching this section on disk and reusing it when
//! the SHA-256 matches, we can skip the entire base-section assembly on
//! session start and immediately provide byte-identical bytes to the API.
//!
//! This is especially valuable for the DeepSeek service-side prefix cache:
//! when the base section bytes are identical across sessions, the server
//! can reuse its cached KV states for the entire base section, giving
//! ~90% discount on cached tokens.
//!
//! ## Cache layout
//!
//! ```text
//! ~/.codewhale/prompt_cache/
//!   <system_hash>.bin   — the serialized base section text
//!   <system_hash>.meta  — JSON metadata (workspace path, mtime, timestamp)
//! ```
//!
//! The cache key is the SHA-256 of the base section text, computed by
//! `PrefixFingerprint::compute`. The metadata file includes the workspace
//! path and its mtime so that workspace changes invalidate the cache even
//! if the base section hash happens to collide (extremely unlikely with
//! SHA-256, but cheap to guard against).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::logging;

/// Metadata stored alongside a cached base section.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
struct CacheMetadata {
    /// Absolute path to the workspace that produced this base section.
    workspace: PathBuf,
    /// Modification time of the workspace directory at cache-write time.
    /// Used as a secondary invalidation signal: if the workspace mtime
    /// changed, the cache is stale even if the base section hash matches
    /// (which would require a hash collision).
    workspace_mtime_secs: u64,
    /// Unix timestamp when the cache was written.
    cached_at_secs: u64,
}

/// Return the directory where prompt caches are stored.
///
/// Creates the directory if it doesn't exist.
#[allow(dead_code)]
fn cache_dir() -> Option<PathBuf> {
    // Override hook so tests never touch the real ~/.codewhale cache (and
    // never race each other through it).
    let dir = match std::env::var_os("CODEWHALE_PROMPT_CACHE_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => dirs::home_dir()?.join(".codewhale").join("prompt_cache"),
    };
    if let Err(err) = fs::create_dir_all(&dir) {
        logging::warn(format!("Failed to create prompt cache dir: {err}"));
        return None;
    }
    Some(dir)
}

/// Get the modification time of a directory as seconds since epoch.
#[allow(dead_code)]
fn dir_mtime_secs(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Try to load a cached base section from disk.
///
/// Returns `Some(text)` if a valid cache entry exists for the given hash
/// and workspace, or `None` if the cache is missing, stale, or corrupt.
#[allow(dead_code)]
pub fn load_cached_base_section(base_hash: &str, workspace: &Path) -> Option<String> {
    let dir = cache_dir()?;
    let bin_path = dir.join(format!("{base_hash}.bin"));
    let meta_path = dir.join(format!("{base_hash}.meta"));

    // Check that both files exist.
    if !bin_path.exists() || !meta_path.exists() {
        return None;
    }

    // Read and validate metadata.
    let meta_bytes = fs::read(&meta_path).ok()?;
    let meta: CacheMetadata = serde_json::from_slice(&meta_bytes).ok()?;

    // Verify workspace path matches.
    if meta.workspace != workspace {
        return None;
    }

    // Verify workspace mtime hasn't changed (guards against hash collisions).
    let current_mtime = dir_mtime_secs(workspace);
    if current_mtime != meta.workspace_mtime_secs {
        logging::info(format!(
            "Prompt cache stale: workspace mtime changed ({meta_mtime} → {current_mtime})",
            meta_mtime = meta.workspace_mtime_secs
        ));
        return None;
    }

    // Read the cached base section.
    let text = fs::read_to_string(&bin_path).ok()?;
    logging::info(format!(
        "Prompt cache hit: {base_hash} ({} bytes)",
        text.len()
    ));
    Some(text)
}

/// Save a base section to disk for cross-session reuse.
///
/// The cache key is `base_hash` (SHA-256 of the base section text). The
/// metadata includes the workspace path and its mtime for invalidation.
#[allow(dead_code)]
pub fn save_cached_base_section(base_hash: &str, base_text: &str, workspace: &Path) {
    let dir = match cache_dir() {
        Some(d) => d,
        None => return,
    };

    let bin_path = dir.join(format!("{base_hash}.bin"));
    let meta_path = dir.join(format!("{base_hash}.meta"));

    // Write the base section text.
    if let Err(err) = fs::write(&bin_path, base_text) {
        logging::warn(format!("Failed to write prompt cache bin: {err}"));
        return;
    }

    // Write the metadata.
    let meta = CacheMetadata {
        workspace: workspace.to_path_buf(),
        workspace_mtime_secs: dir_mtime_secs(workspace),
        cached_at_secs: SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    if let Err(err) = fs::write(&meta_path, serde_json::to_vec(&meta).unwrap_or_default()) {
        logging::warn(format!("Failed to write prompt cache meta: {err}"));
    }

    logging::info(format!("Prompt cache saved: {base_hash}"));
}

/// Evict stale cache entries.
///
/// Removes cache entries older than `max_age_secs` or whose workspace
/// mtime no longer matches. This is a best-effort cleanup; it runs
/// lazily when the cache is accessed.
#[allow(dead_code)]
pub fn evict_stale_entries(max_age_secs: u64) {
    let dir = match cache_dir() {
        Some(d) => d,
        None => return,
    };

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "meta")
            && let Ok(bytes) = fs::read(&path)
            && let Ok(meta) = serde_json::from_slice::<CacheMetadata>(&bytes)
        {
            let stale = now.saturating_sub(meta.cached_at_secs) > max_age_secs;
            let workspace_gone = !meta.workspace.exists();
            let mtime_changed =
                workspace_gone || dir_mtime_secs(&meta.workspace) != meta.workspace_mtime_secs;

            if stale || workspace_gone || mtime_changed {
                let hash = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown");
                let _ = fs::remove_file(&path);
                let _ = fs::remove_file(path.with_extension("bin"));
                logging::info(format!("Evicted prompt cache: {hash}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Point the cache at a private tempdir for the duration of `f`.
    ///
    /// The env var is process-global, so a mutex serializes these tests;
    /// without it they all share ~/.codewhale/prompt_cache and the eviction
    /// test can delete another test's entry mid-flight (and the suite
    /// pollutes the developer's real cache).
    fn with_isolated_cache_dir<T>(f: impl FnOnce() -> T) -> T {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cache_tmp = tempdir().expect("tempdir");
        // SAFETY: serialized by ENV_LOCK; only this module reads the var.
        unsafe { std::env::set_var("CODEWHALE_PROMPT_CACHE_DIR", cache_tmp.path()) };
        let out = f();
        unsafe { std::env::remove_var("CODEWHALE_PROMPT_CACHE_DIR") };
        out
    }

    #[test]
    fn save_and_load_round_trip() {
        with_isolated_cache_dir(|| {
            let tmp = tempdir().expect("tempdir");
            let workspace = tmp.path();
            let hash = "abc123";
            let text = "Hello, world!";

            save_cached_base_section(hash, text, workspace);
            let loaded = load_cached_base_section(hash, workspace);
            assert_eq!(loaded.as_deref(), Some(text));
        });
    }

    #[test]
    fn load_returns_none_for_missing_cache() {
        with_isolated_cache_dir(|| {
            let tmp = tempdir().expect("tempdir");
            assert!(load_cached_base_section("nonexistent", tmp.path()).is_none());
        });
    }

    #[test]
    fn load_returns_none_for_wrong_workspace() {
        with_isolated_cache_dir(|| {
            let tmp1 = tempdir().expect("tempdir");
            let tmp2 = tempdir().expect("tempdir");
            let hash = "def456";
            let text = "cached content";

            save_cached_base_section(hash, text, tmp1.path());
            assert!(load_cached_base_section(hash, tmp2.path()).is_none());
        });
    }

    #[test]
    fn evict_preserves_fresh_entries() {
        with_isolated_cache_dir(|| {
            let tmp = tempdir().expect("tempdir");
            let workspace = tmp.path();
            let hash = "fresh_entry";
            let text = "fresh content";

            save_cached_base_section(hash, text, workspace);

            // Evict entries older than 3600 seconds (1 hour). Fresh entries
            // should survive.
            evict_stale_entries(3600);

            // The entry should still be there since it was just saved.
            assert_eq!(
                load_cached_base_section(hash, workspace).as_deref(),
                Some(text)
            );
        });
    }
}
