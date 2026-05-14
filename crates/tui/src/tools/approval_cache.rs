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
//! | file writes    | `file:<tool_name>:<hash of args>`        |
//! | shell tools    | `shell:<tool_name>:<hash of args>`       |
//! | `fetch_url`    | `net:<hostname>`                         |
//! | everything else| `tool:<tool_name>:<hash of args>`        |
//!
//! The cache is **session‑keyed**: entries carry an
//! `ApprovedForSession` flag. When true, the approval is reused for the
//! remainder of the session; when false, it is a one‑shot grant (future
//! calls with the same fingerprint still prompt).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::Instant;

use serde_json::Value;
use sha2::{Digest, Sha256};

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
/// The key incorporates the tool name and a canonical digest of the
/// arguments so that denying one call only suppresses exact retries, not
/// later invocations of the same tool with different parameters.
#[must_use]
pub fn build_approval_key(tool_name: &str, input: &serde_json::Value) -> ApprovalKey {
    let fingerprint = match tool_name {
        "apply_patch" | "write_file" | "edit_file" | "fim_edit" => {
            format!("file:{tool_name}:{}", hash_json_value(input))
        }
        "exec_shell"
        | "task_shell_start"
        | "exec_shell_wait"
        | "exec_shell_interact"
        | "exec_wait"
        | "exec_interact" => {
            format!("shell:{tool_name}:{}", hash_json_value(input))
        }
        "fetch_url" | "web.fetch" | "web_fetch" => {
            let host = parse_host(input);
            format!("net:{host}")
        }
        _ => format!("tool:{tool_name}:{}", hash_json_value(input)),
    };
    ApprovalKey(fingerprint)
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

fn hash_json_value(value: &Value) -> String {
    let mut canonical = String::new();
    push_canonical_json(value, &mut canonical);

    let digest = Sha256::digest(canonical.as_bytes());
    let mut short = String::with_capacity(16);
    for byte in &digest[..8] {
        write!(&mut short, "{byte:02x}").expect("writing to String cannot fail");
    }
    short
}

fn push_canonical_json(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(value) => {
            out.push_str("bool:");
            out.push_str(if *value { "true" } else { "false" });
        }
        Value::Number(value) => {
            out.push_str("number:");
            out.push_str(&value.to_string());
        }
        Value::String(value) => {
            out.push_str("string:");
            let encoded = serde_json::to_string(value).expect("serializing a string cannot fail");
            out.push_str(&encoded);
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                push_canonical_json(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut entries = map.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(key, _)| *key);

            out.push('{');
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                let encoded_key =
                    serde_json::to_string(key).expect("serializing an object key cannot fail");
                out.push_str(&encoded_key);
                out.push(':');
                push_canonical_json(value, out);
            }
            out.push('}');
        }
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
    fn shell_keys_include_full_command_arguments() {
        let key_a = build_approval_key("exec_shell", &json!({"command": "cargo build"}));
        let key_b = build_approval_key("exec_shell", &json!({"command": "cargo build --release"}));
        assert_ne!(key_a, key_b);
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
    fn patch_keys_differ_by_body_for_same_path() {
        let key_a = build_approval_key(
            "apply_patch",
            &json!({"changes": [{"path": "a.rs", "content": "x"}]}),
        );
        let key_b = build_approval_key(
            "apply_patch",
            &json!({"changes": [{"path": "a.rs", "content": "y"}]}),
        );
        assert_ne!(key_a, key_b);
    }

    #[test]
    fn file_write_keys_differ_by_path_and_content() {
        let key_a = build_approval_key("write_file", &json!({"path": "a.txt", "content": "x"}));
        let key_b = build_approval_key("write_file", &json!({"path": "b.txt", "content": "x"}));
        let key_c = build_approval_key("write_file", &json!({"path": "a.txt", "content": "y"}));
        assert_ne!(key_a, key_b);
        assert_ne!(key_a, key_c);
    }

    #[test]
    fn edit_file_keys_differ_by_replacement() {
        let key_a = build_approval_key(
            "edit_file",
            &json!({"path": "a.txt", "search": "old", "replace": "new"}),
        );
        let key_b = build_approval_key(
            "edit_file",
            &json!({"path": "a.txt", "search": "old", "replace": "newer"}),
        );
        assert_ne!(key_a, key_b);
    }

    #[test]
    fn task_shell_keys_differ_by_command() {
        let key_a = build_approval_key("task_shell_start", &json!({"command": "cargo build"}));
        let key_b = build_approval_key("task_shell_start", &json!({"command": "cargo test"}));
        assert_ne!(key_a, key_b);
    }

    #[test]
    fn net_keys_differ_by_host() {
        let key_a = build_approval_key("fetch_url", &json!({"url": "https://example.com"}));
        let key_b = build_approval_key("fetch_url", &json!({"url": "https://other.org"}));
        assert_ne!(key_a, key_b);
    }

    #[test]
    fn generic_tool_uses_tool_name_and_input_hash() {
        let key_a = build_approval_key("read_file", &json!({"path": "a.txt"}));
        let key_b = build_approval_key("read_file", &json!({"path": "b.txt"}));
        assert_ne!(key_a, key_b);
        assert!(key_a.0.starts_with("tool:read_file:"));
    }

    #[test]
    fn input_hash_is_stable_across_object_key_order() {
        let key_a = build_approval_key("write_file", &json!({"path": "a.txt", "content": "x"}));
        let key_b = build_approval_key("write_file", &json!({"content": "x", "path": "a.txt"}));
        assert_eq!(key_a, key_b);
    }

    #[test]
    fn canonical_json_omits_trailing_commas() {
        let mut canonical = String::new();
        push_canonical_json(&json!({"b": [true, false], "a": {"x": 1}}), &mut canonical);

        assert_eq!(
            canonical,
            r#"{"a":{"x":number:1},"b":[bool:true,bool:false]}"#
        );
        assert!(!canonical.contains(",]"));
        assert!(!canonical.contains(",}"));
    }
}
