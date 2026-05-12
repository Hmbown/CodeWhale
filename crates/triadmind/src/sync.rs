//! # Sync — File change detection and topology rebuild scheduling
//!
//! Ported from triadmind-core/sync.ts
//!
//! Detects source file changes by comparing SHA-256 manifests, and triggers
//! topology map regeneration when needed.
//!
//! @LeftBranch: build_manifest, sync_triad_map, is_source_file
//! @RightBranch: SyncManifest, SyncResult

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{should_skip_walk_path, TriadConfig, WorkspacePaths};

// ── Manifest Types ──────────────────────────────────────────────────

/// A single source file entry in the sync manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceFileDigest {
    /// Relative path from project root, forward-slash normalized.
    pub path: String,
    /// SHA-256 hex digest of the file contents.
    pub sha256: String,
}

/// Manifest tracking source file state for change detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncManifest {
    #[serde(rename = "schemaVersion")]
    pub schema_version: String,
    #[serde(rename = "generatedAt")]
    pub generated_at: String,
    #[serde(rename = "parserEngine")]
    pub parser_engine: String,
    #[serde(rename = "configHash")]
    pub config_hash: String,
    pub files: Vec<SourceFileDigest>,
}

/// Result of a sync operation.
#[derive(Debug, Clone)]
pub struct SyncResult {
    /// Whether any files changed since the last sync.
    pub changed: bool,
    /// Number of source files tracked.
    pub file_count: usize,
    /// Reason for change (empty if unchanged).
    pub reason: String,
}

// ── File Collection ─────────────────────────────────────────────────

/// File extensions considered "source files" for scanning.
const SOURCE_EXTENSIONS: &[&str] = &[
    "ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs", "py", "go", "rs", "cpp", "cc", "cxx",
    "hpp", "hh", "h", "java",
];

/// Check if a relative path is a recognizable source file.
pub fn is_source_file(rel_path: &str) -> bool {
    let normalized = rel_path.replace('\\', "/");
    let lower = normalized.to_lowercase();

    SOURCE_EXTENSIONS
        .iter()
        .any(|ext| lower.ends_with(&format!(".{ext}")))
}

// ── Hashing ─────────────────────────────────────────────────────────

/// Compute SHA-256 hex digest of a file.
fn hash_file(path: &Path) -> Result<String, std::io::Error> {
    let bytes = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

/// Compute a config hash for manifest comparison.
fn hash_config(config: &TriadConfig) -> String {
    // Use serde_json to produce a stable-ish hash
    let json = serde_json::to_string(config).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(json.as_bytes());
    format!("{:x}", hasher.finalize())
}

// ── Manifest Operations ─────────────────────────────────────────────

/// Build a fresh manifest by scanning source files in the project.
pub fn build_manifest(paths: &WorkspacePaths, config: &TriadConfig) -> SyncManifest {
    let mut files: Vec<SourceFileDigest> = Vec::new();

    let walker = walkdir::WalkDir::new(&paths.project_root)
        .max_depth(10)
        .into_iter()
        .filter_entry(|e| crate::config::should_skip_walk_entry(e));

    for entry in walker.flatten() {
        if !entry.file_type().is_file() {
            continue;
        }

        let abs_path = entry.path();
        let Ok(rel_path) = abs_path.strip_prefix(&paths.project_root) else {
            continue;
        };

        let rel_str = rel_path.to_string_lossy().replace('\\', "/");

        if !is_source_file(&rel_str) {
            continue;
        }

        if should_skip_walk_path(&rel_str) {
            continue;
        }

        // Try to hash; skip files we can't read
        if let Ok(sha) = hash_file(abs_path) {
            files.push(SourceFileDigest {
                path: rel_str,
                sha256: sha,
            });
        }
    }

    // Sort for deterministic output
    files.sort_by(|a, b| a.path.cmp(&b.path));

    SyncManifest {
        schema_version: "1.0".into(),
        generated_at: chrono_now(),
        parser_engine: format!("{:?}", config.architecture.parser_engine).to_lowercase(),
        config_hash: hash_config(config),
        files,
    }
}

/// Read a previously-saved manifest from disk.
pub fn read_manifest(paths: &WorkspacePaths) -> Option<SyncManifest> {
    let content = std::fs::read_to_string(&paths.sync_cache_file).ok()?;
    let trimmed = content.trim().trim_start_matches('\u{FEFF}');
    serde_json::from_str(trimmed).ok()
}

/// Check if two manifests describe the same source state.
pub fn is_same_manifest(prev: &SyncManifest, current: &SyncManifest) -> bool {
    // Quick check: file count
    if prev.files.len() != current.files.len() {
        return false;
    }

    // Config hash must match
    if prev.config_hash != current.config_hash {
        return false;
    }

    // Build sets of (path, sha256) for comparison
    let prev_set: HashSet<(&str, &str)> = prev
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.sha256.as_str()))
        .collect();
    let curr_set: HashSet<(&str, &str)> = current
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.sha256.as_str()))
        .collect();

    prev_set == curr_set
}

/// Save a manifest to the sync cache file.
pub fn write_manifest(paths: &WorkspacePaths, manifest: &SyncManifest) -> Result<(), std::io::Error> {
    std::fs::create_dir_all(&paths.cache_dir)?;
    let json = serde_json::to_string_pretty(manifest)?;
    std::fs::write(&paths.sync_cache_file, json)?;
    Ok(())
}

// ── Main Sync Entry Point ───────────────────────────────────────────

/// Run a sync cycle: check for changes, and optionally trigger topology rebuild.
///
/// Returns `SyncResult` indicating whether changes were detected.
///
/// Note: The actual topology rebuild (calling into the parser) is not yet
/// implemented in Rust. This function detects changes and returns the result;
/// the caller is responsible for triggering the appropriate parser.
pub fn sync_triad_map(paths: &WorkspacePaths, force: bool) -> Result<SyncResult, std::io::Error> {
    std::fs::create_dir_all(&paths.cache_dir)?;

    let config = crate::config::load_triad_config(paths);
    let current_manifest = build_manifest(paths, &config);

    let previous_manifest = read_manifest(paths);

    let changed = if force {
        true
    } else if previous_manifest.is_none() {
        true
    } else {
        !is_same_manifest(previous_manifest.as_ref().unwrap(), &current_manifest)
    };

    let reason = if force {
        "forced".into()
    } else if previous_manifest.is_none() {
        "no previous manifest".into()
    } else if changed {
        "source files changed".into()
    } else {
        String::new()
    };

    // Always write the current manifest to track state
    write_manifest(paths, &current_manifest)?;

    Ok(SyncResult {
        changed,
        file_count: current_manifest.files.len(),
        reason,
    })
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Get current time as ISO 8601 string.
pub(crate) fn chrono_now() -> String {
    // Simple UTC timestamp using std
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    // Simple ISO 8601 format
    format!("{}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        secs / 31536000 + 1970,
        (secs % 31536000) / 2592000 + 1,
        (secs % 2592000) / 86400 + 1,
        (secs % 86400) / 3600,
        (secs % 3600) / 60,
        secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_source_file() {
        assert!(is_source_file("src/main.rs"));
        assert!(is_source_file("lib/index.ts"));
        assert!(is_source_file("foo/bar.py"));
        assert!(!is_source_file("README.md"));
        assert!(!is_source_file("Cargo.toml"));
    }

    #[test]
    fn test_is_same_manifest_identical() {
        let m1 = SyncManifest {
            schema_version: "1.0".into(),
            generated_at: "2024-01-01".into(),
            parser_engine: "tree_sitter".into(),
            config_hash: "abc".into(),
            files: vec![SourceFileDigest {
                path: "src/main.rs".into(),
                sha256: "deadbeef".into(),
            }],
        };
        let m2 = m1.clone();
        assert!(is_same_manifest(&m1, &m2));
    }

    #[test]
    fn test_is_same_manifest_different() {
        let m1 = SyncManifest {
            schema_version: "1.0".into(),
            generated_at: "2024-01-01".into(),
            parser_engine: "tree_sitter".into(),
            config_hash: "abc".into(),
            files: vec![SourceFileDigest {
                path: "src/main.rs".into(),
                sha256: "deadbeef".into(),
            }],
        };
        let m2 = SyncManifest {
            schema_version: "1.0".into(),
            generated_at: "2024-01-02".into(),
            parser_engine: "tree_sitter".into(),
            config_hash: "abc".into(),
            files: vec![SourceFileDigest {
                path: "src/main.rs".into(),
                sha256: "different".into(),
            }],
        };
        assert!(!is_same_manifest(&m1, &m2));
    }
}
