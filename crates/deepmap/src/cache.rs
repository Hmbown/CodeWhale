//! Persistent cache for RepoGraph symbol/edge data.
//!
//! Saves and loads serialised graphs to `~/.cache/deepmap/{name}_{hash}/`.
//! Cache validity is guarded by a schema version and an optional scan
//! fingerprint (hash of file paths + sizes + mtimes).

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::types::{Edge, Symbol};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Current cache schema version.  Increment when the serialised format
/// changes in a backward-incompatible way.
pub const CACHE_SCHEMA_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// SymbolCache
// ---------------------------------------------------------------------------

/// On-disk representation of a scanned project graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolCache {
    pub symbols: HashMap<String, Symbol>,
    pub edges: Vec<Edge>,
    /// ISO-8601 timestamp of when the scan was performed.
    pub scan_time: String,
    pub project_path: String,
    pub symbol_count: usize,
    pub edge_count: usize,
    pub schema_version: u32,
}

// ---------------------------------------------------------------------------
// Cache directory
// ---------------------------------------------------------------------------

/// Return the cache directory for `project_path`.
///
/// Directory layout: `~/.cache/deepmap/{dir_name}_{path_hash}/`
/// where `path_hash` is the first 16 hex chars of the SHA-256 of the
/// canonicalised project path.
pub fn get_cache_dir(project_path: &Path) -> PathBuf {
    let name = project_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project");
    let hash = project_path_hash(project_path);
    let base = dirs::cache_dir()
        .map(|d| d.join("deepmap"))
        .unwrap_or_else(|| {
            // Fallback: ~/.cache/deepmap
            let home = std::env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("."));
            home.join(".cache").join("deepmap")
        });
    base.join(format!("{}_{}", name, hash))
}

// ---------------------------------------------------------------------------
// Save / Load
// ---------------------------------------------------------------------------

/// Serialise `symbols` and `edges` to the cache directory for
/// `project_path`.
///
/// The write is **atomic**: data is first written to a temporary file
/// in the same directory, then renamed over the final path.  A previous
/// cache (if any) is backed up before overwriting.
pub fn save_cache(
    project_path: &Path,
    symbols: &HashMap<String, Symbol>,
    edges: &Vec<Edge>,
) -> Result<PathBuf, String> {
    let cache_dir = get_cache_dir(project_path);
    fs::create_dir_all(&cache_dir)
        .map_err(|e| format!("Failed to create cache dir: {}", e))?;

    let cache_path = cache_dir.join("symbol_cache.json");
    let temp_path = cache_dir.join("symbol_cache.json.tmp");

    let scan_time = chrono::Utc::now().to_rfc3339();

    let cache = SymbolCache {
        symbols: symbols.clone(),
        edges: edges.clone(),
        scan_time,
        project_path: project_path.to_string_lossy().to_string(),
        symbol_count: symbols.len(),
        edge_count: edges.len(),
        schema_version: CACHE_SCHEMA_VERSION,
    };

    let json = serde_json::to_string_pretty(&cache)
        .map_err(|e| format!("Serialization failed: {}", e))?;

    // Write to temp file, then rename for atomicity.
    {
        let mut tmp = fs::File::create(&temp_path)
            .map_err(|e| format!("Failed to create temp file: {}", e))?;
        tmp.write_all(json.as_bytes())
            .map_err(|e| format!("Failed to write temp file: {}", e))?;
        tmp.flush()
            .map_err(|e| format!("Failed to flush temp file: {}", e))?;
    }

    // Backup existing cache if present.
    if cache_path.exists() {
        let backup_path = cache_dir.join("symbol_cache.json.bak");
        let _ = fs::rename(&cache_path, &backup_path);
    }

    fs::rename(&temp_path, &cache_path)
        .map_err(|e| format!("Failed to rename cache file: {}", e))?;

    Ok(cache_path)
}

/// Load a previously saved cache for `project_path`.
///
/// Returns `None` when:
/// - The cache file does not exist.
/// - Deserialisation fails.
/// - The schema version does not match (the stale file is removed).
pub fn load_cache(project_path: &Path) -> Option<SymbolCache> {
    let cache_path = get_cache_dir(project_path).join("symbol_cache.json");

    if !cache_path.exists() {
        return None;
    }

    let data = fs::read_to_string(&cache_path).ok()?;
    let cache: SymbolCache = serde_json::from_str(&data).ok()?;

    if cache.schema_version != CACHE_SCHEMA_VERSION {
        // Schema mismatch: clean up stale file and return None.
        let _ = fs::remove_file(&cache_path);
        return None;
    }

    Some(cache)
}

// ---------------------------------------------------------------------------
// Scan fingerprint
// ---------------------------------------------------------------------------

/// Compute a deterministic hash of the current working-tree state for
/// `files` under `project_path`.
///
/// The hash is built from `(file_path, file_size, mtime_nanos)` tuples
/// and uses SHA-256 so that the cache can be invalidated when any file
/// changes.
pub fn scan_fingerprint(project_path: &Path, files: &[String]) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();

    for file in files {
        let full = project_path.join(file);
        // Record the relative path as part of the fingerprint.
        hasher.update(file.as_bytes());
        hasher.update(b":");

        match fs::metadata(&full) {
            Ok(meta) => {
                hasher.update(meta.len().to_string().as_bytes());
                hasher.update(b":");
                if let Ok(mtime) = meta.modified() {
                    if let Ok(dur) = mtime.duration_since(std::time::UNIX_EPOCH) {
                        hasher.update(dur.as_nanos().to_string().as_bytes());
                    }
                }
            }
            Err(_) => {
                // File vanished: record a sentinel so the fingerprint
                // changes.
                hasher.update(b"__missing__");
            }
        }
        hasher.update(b"\n");
    }

    format!("{:x}", hasher.finalize())
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Produce a short hex digest of the canonicalised project path.
fn project_path_hash(project_path: &Path) -> String {
    use sha2::{Digest, Sha256};

    let canonical = project_path
        .canonicalize()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| project_path.to_string_lossy().to_string());

    let hash = Sha256::digest(canonical.as_bytes());
    // Take the first 16 hex chars (8 bytes).
    hex_encode(&hash[..8])
}

/// Encode a byte slice as a lowercase hex string (avoids pulling in the
/// `hex` crate for one helper).
fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{:02x}", b));
    }
    out
}
