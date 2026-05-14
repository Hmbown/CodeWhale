#![allow(dead_code)]
//! Per‑call approval cache with fingerprint keys (§5.A).
//!
//! Instead of caching by tool name alone (which would let an approved
//! `exec_shell "cat foo"` silently pass `exec_shell "rm -rf /"`), the
//! cache keys off a **call fingerprint** — a digest of the tool name and
//! the semantically‑relevant portion of its arguments.
//!
//! ## Fingerprint shape
//!
//! | Tool           | Key                                      |
//! |---------------|------------------------------------------|
//! | `apply_patch`  | `patch:<hash of file paths>`             |
//! | `exec_shell`   | `shell:<command prefix (first 3 tokens)>` |
//! | `fetch_url`    | `net:<hostname>`                         |
//! | everything else| `tool:<tool_name>:<hash of input>`       |
//!
//! The cache is **session‑keyed**: entries carry an
//! `ApprovedForSession` flag. When true, the approval is reused for the
//! remainder of the session; when false, it is a one‑shot grant (future
//! calls with the same fingerprint still prompt).

use std::collections::HashMap;
use std::time::Instant;

use crate::command_safety::classify_command;

/// The fingerprint of a tool call — stable enough to match repeated
/// calls but specific enough to avoid privilege confusion.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ApprovalKey(pub String);

/// Status of a previously‑rendered approval decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalCacheStatus {
    /// Call fingerprint matched and the session‑level flag says reuse.
    Approved,
    /// Call fingerprint matched but the grant was one‑shot (already consumed).
    Denied,
    /// No match — requires fresh approval.
    Unknown,
}

/// A single cache entry.
#[derive(Debug, Clone)]
struct ApprovalCacheEntry {
    /// When this entry was created.
    created: Instant,
    /// Whether the approval should be reused across the session.
    approved_for_session: bool,
}

/// An approval cache backed by tool‑call fingerprints.
#[derive(Debug, Default)]
pub struct ApprovalCache {
    entries: HashMap<ApprovalKey, ApprovalCacheEntry>,
}

impl ApprovalCache {
    /// Construct an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Look up a previously‑rendered approval decision.
    pub fn check(&self, key: &ApprovalKey) -> ApprovalCacheStatus {
        let Some(entry) = self.entries.get(key) else {
            return ApprovalCacheStatus::Unknown;
        };
        if entry.approved_for_session {
            ApprovalCacheStatus::Approved
        } else {
            ApprovalCacheStatus::Denied
        }
    }

    /// Record an approval decision under the given fingerprint.
    ///
    /// When `approved_for_session` is true, subsequent calls with the
    /// same key will auto‑approve for the remainder of the session.
    pub fn insert(&mut self, key: ApprovalKey, approved_for_session: bool) {
        self.entries.insert(
            key,
            ApprovalCacheEntry {
                created: Instant::now(),
                approved_for_session,
            },
        );
    }

    /// Clear all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Number of cached entries.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ── Fingerprint helpers ────────────────────────────────────────────

/// Build the approval‑cache key for a tool call.
///
/// The key incorporates the tool name and a lossy digest of the
/// arguments so that the cache can distinguish `exec_shell "ls"`
/// from `exec_shell "rm -rf /"` while still recognising repeated
/// invocations of the same harmless command.
#[must_use]
pub fn build_approval_key(tool_name: &str, input: &serde_json::Value) -> ApprovalKey {
    let fingerprint = match tool_name {
        "apply_patch" => {
            let paths_hash = hash_patch_paths(input);
            format!("patch:{paths_hash}")
        }
        "exec_shell"
        | "exec_shell_wait"
        | "exec_shell_interact"
        | "exec_wait"
        | "exec_interact" => {
            let prefix = command_prefix(input);
            format!("shell:{prefix}")
        }
        "fetch_url" | "web.fetch" | "web_fetch" => {
            let host = parse_host(input);
            format!("net:{host}")
        }
        _ => {
            let input_hash = hash_json_input(input);
            format!("tool:{tool_name}:{input_hash}")
        }
    };
    ApprovalKey(fingerprint)
}

/// Return the canonical command prefix for the shell command in `input`.
///
/// Uses [`classify_command`] from the arity dictionary so that
/// `auto_allow = ["git status"]` correctly matches `git status -s` and
/// `git status --porcelain` without also matching `git push`.
fn command_prefix(input: &serde_json::Value) -> String {
    let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    if tokens.is_empty() {
        return "<empty>".to_string();
    }
    classify_command(&tokens)
}

/// Hash the sorted set of file paths referenced by a patch input.
fn hash_patch_paths(input: &serde_json::Value) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut paths: Vec<&str> = Vec::new();

    if let Some(changes) = input.get("changes").and_then(|v| v.as_array()) {
        for change in changes {
            if let Some(path) = change.get("path").and_then(|v| v.as_str()) {
                paths.push(path);
            }
        }
    } else if let Some(patch_text) = input.get("patch").and_then(|v| v.as_str()) {
        for line in patch_text.lines() {
            if let Some(rest) = line.strip_prefix("+++ b/") {
                paths.push(rest.trim());
            }
        }
    }

    paths.sort();
    paths.dedup();

    if paths.is_empty() {
        return "no_files".to_string();
    }

    let mut hasher = DefaultHasher::new();
    for path in &paths {
        path.hash(&mut hasher);
    }
    format!("{:x}", hasher.finish())
}

fn hash_json_input(input: &serde_json::Value) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    serde_json::to_string(input)
        .unwrap_or_default()
        .hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

/// Parse the host portion from a URL input.
fn parse_host(input: &serde_json::Value) -> String {
    let url = input.get("url").and_then(|v| v.as_str()).unwrap_or("");

    if let Ok(parsed) = reqwest::Url::parse(url) {
        parsed.host_str().unwrap_or(url).to_string()
    } else {
        url.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cache_hit_returns_approved_for_session() {
        let mut cache = ApprovalCache::new();
        let key = build_approval_key("exec_shell", &json!({"command": "ls -la"}));
        cache.insert(key.clone(), true);
        assert_eq!(cache.check(&key), ApprovalCacheStatus::Approved);
    }

    #[test]
    fn cache_one_shot_is_not_reused() {
        let mut cache = ApprovalCache::new();
        let key = build_approval_key("exec_shell", &json!({"command": "cargo build"}));
        cache.insert(key.clone(), false);
        assert_eq!(cache.check(&key), ApprovalCacheStatus::Denied);
    }

    #[test]
    fn cache_miss_is_unknown() {
        let cache = ApprovalCache::new();
        let key = build_approval_key("exec_shell", &json!({"command": "ls"}));
        assert_eq!(cache.check(&key), ApprovalCacheStatus::Unknown);
    }

    #[test]
    fn different_commands_different_keys() {
        let key_a = build_approval_key("exec_shell", &json!({"command": "ls"}));
        let key_b = build_approval_key("exec_shell", &json!({"command": "rm -rf /tmp"}));
        assert_ne!(key_a, key_b);
    }

    #[test]
    fn same_command_same_key() {
        let key_a = build_approval_key("exec_shell", &json!({"command": "cargo build --release"}));
        let key_b = build_approval_key("exec_shell", &json!({"command": "cargo build --release"}));
        assert_eq!(key_a, key_b);
    }

    #[test]
    fn command_prefix_drops_flags() {
        let key_a = build_approval_key("exec_shell", &json!({"command": "cargo build"}));
        let key_b = build_approval_key("exec_shell", &json!({"command": "cargo build --release"}));
        assert_eq!(key_a, key_b);
    }

    #[test]
    fn patch_keys_differ_by_path() {
        let key_a = build_approval_key(
            "apply_patch",
            &json!({"changes": [{"path": "a.rs", "content": "x"}]}),
        );
        let key_b = build_approval_key(
            "apply_patch",
            &json!({"changes": [{"path": "b.rs", "content": "x"}]}),
        );
        assert_ne!(key_a, key_b);
    }

    #[test]
    fn net_keys_differ_by_host() {
        let key_a = build_approval_key("fetch_url", &json!({"url": "https://example.com"}));
        let key_b = build_approval_key("fetch_url", &json!({"url": "https://other.org"}));
        assert_ne!(key_a, key_b);
    }

    #[test]
    fn generic_tool_keys_include_arguments() {
        let key_a = build_approval_key("read_file", &json!({"path": "a.txt"}));
        let key_b = build_approval_key("read_file", &json!({"path": "b.txt"}));
        assert_ne!(key_a, key_b);
        assert!(key_a.0.starts_with("tool:read_file:"));
    }

    #[test]
    fn generic_tool_same_arguments_reuse_key() {
        let input = json!({"path": "a.txt"});
        let key_a = build_approval_key("edit_file", &input);
        let key_b = build_approval_key("edit_file", &input);
        assert_eq!(key_a, key_b);
    }
}
