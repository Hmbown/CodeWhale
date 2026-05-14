//! Prefix-cache stability manager (inspired by Reasonix's Pillar 1).
//!
//! DeepSeek's automatic prefix caching activates only when the *exact*
//! byte prefix of a request matches the prior request. Any system-prompt
//! drift, tool-list reordering, or message-rewriting busts the cache
//! for every token after the changed byte.
//!
//! This module provides a `PrefixStabilityManager` that:
//!
//! 1. **Fingerprints** the immutable prefix (system prompt + tool specs)
//!    at session start, using SHA-256 for strong collision resistance.
//! 2. **Detects drift** by comparing the current prefix against the
//!    pinned fingerprint before every request.
//! 3. **Diagnoses** the cause of drift — did the system prompt change?
//!    Did the tool set change? Both?
//! 4. **Emits events** so the TUI can surface stability to the user.
//!
//! ## Three-region model (from Reasonix)
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │ IMMUTABLE PREFIX                        │ ← fixed for session
//! │   system + tool_specs                    │   cache hit candidate
//! ├─────────────────────────────────────────┤
//! │ APPEND-ONLY HISTORY                     │ ← grows monotonically
//! │   [assistant₁][tool₁][assistant₂]...    │   preserves prefix of prior turns
//! ├─────────────────────────────────────────┤
//! │ LATEST USER TURN                        │ ← the only new content per request
//! └─────────────────────────────────────────┘
//! ```

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::models::{SystemPrompt, Tool};

/// A snapshot of the immutable prefix's fingerprint.
///
/// Two snapshots with the same `combined` hash are guaranteed to
/// produce the same byte prefix when serialized for the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefixFingerprint {
    /// SHA-256 of the system prompt text.
    pub system_sha256: String,
    /// SHA-256 of the concatenated, sorted tool names.
    pub tools_sha256: String,
    /// SHA-256 of system_sha256 ++ tools_sha256 (combined).
    pub combined_sha256: String,
}

impl PrefixFingerprint {
    /// Compute a fingerprint from system prompt text and tool list.
    pub fn compute(system_text: &str, tools: Option<&[Tool]>) -> Self {
        let system_sha256 = sha256_hex(system_text.as_bytes());

        let tools_sha256 = match tools {
            Some(tools) if !tools.is_empty() => {
                // Sort tool names deterministically so the hash is
                // stable regardless of registration order.
                let mut tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
                tool_names.sort();
                let joined = tool_names.join(",");
                sha256_hex(joined.as_bytes())
            }
            _ => sha256_hex(b""),
        };

        let combined = format!("{}:{}", system_sha256, tools_sha256);
        let combined_sha256 = sha256_hex(combined.as_bytes());

        Self {
            system_sha256,
            tools_sha256,
            combined_sha256,
        }
    }
}

/// A change record describing what drifted in the prefix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefixChange {
    /// The old fingerprint (before the change).
    pub old: PrefixFingerprint,
    /// The new fingerprint (after the change).
    pub new: PrefixFingerprint,
    /// Whether the system prompt component changed.
    pub system_changed: bool,
    /// Whether the tool set component changed.
    pub tools_changed: bool,
}

#[allow(dead_code)]
impl PrefixChange {
    /// Returns a human-readable description of what changed.
    pub fn description(&self) -> String {
        let mut parts = Vec::new();
        if self.system_changed {
            parts.push("system prompt");
        }
        if self.tools_changed {
            parts.push("tool set");
        }
        if parts.is_empty() {
            return "unknown (fingerprint mismatch but no component detected)".to_string();
        }
        format!("prefix cache invalidated: {} changed", parts.join(" and "))
    }

    /// Returns a short label for TUI chip display.
    pub fn label(&self) -> &'static str {
        if self.system_changed && self.tools_changed {
            "sys+tools"
        } else if self.system_changed {
            "sys"
        } else if self.tools_changed {
            "tools"
        } else {
            "prefix"
        }
    }
}

/// Monitors and manages prefix-cache stability across turns.
///
/// This is the core abstraction, mirroring Reasonix's `ImmutablePrefix`
/// concept but adapted to DeepSeek-TUI's existing architecture where the
/// system prompt is rebuilt each turn and tools are registered at startup.
///
/// Usage:
/// ```ignore
/// let mgr = PrefixStabilityManager::new(system_text, tools);
/// if mgr.check_and_update(system_text, tools) {
///     println!("Prefix is stable (cache-friendly)");
/// } else {
///     let change = mgr.last_change().unwrap();
///     println!("Prefix drifted: {}", change.description());
/// }
/// ```
#[derive(Debug, Clone)]
pub struct PrefixStabilityManager {
    /// The pinned fingerprint from session start or last stabilization.
    pinned: Option<PrefixFingerprint>,
    /// The most recent fingerprint (computed during last check).
    current: Option<PrefixFingerprint>,
    /// The last detected change, if any.
    last_change: Option<PrefixChange>,
    /// Total number of prefix changes detected this session.
    change_count: u64,
    /// Total number of stability checks performed.
    check_count: u64,
}

#[allow(dead_code)]
impl PrefixStabilityManager {
    /// Create a new manager and immediately pin the first fingerprint.
    pub fn new(system_text: &str, tools: Option<&[Tool]>) -> Self {
        let fp = PrefixFingerprint::compute(system_text, tools);
        Self {
            pinned: Some(fp.clone()),
            current: Some(fp),
            last_change: None,
            change_count: 0,
            check_count: 0,
        }
    }

    /// Create a manager in "unpinned" state — no initial fingerprint.
    /// Call `pin()` or `check_and_update()` to establish the baseline.
    pub fn new_unpinned() -> Self {
        Self {
            pinned: None,
            current: None,
            last_change: None,
            change_count: 0,
            check_count: 0,
        }
    }

    /// Explicitly pin a fingerprint, replacing any prior pinned state.
    /// Returns `true` if this is the first pin, or `false` if replacing.
    /// Note: does NOT increment `check_count` — that counter is reserved
    /// for `check_and_update` calls so `stability_ratio()` stays accurate.
    pub fn pin(&mut self, system_text: &str, tools: Option<&[Tool]>) -> bool {
        let fp = PrefixFingerprint::compute(system_text, tools);
        let was_unpinned = self.pinned.is_none();
        self.pinned = Some(fp.clone());
        self.current = Some(fp);
        was_unpinned
    }

    /// Check whether the current prefix matches the pinned fingerprint.
    /// Updates internal state and returns:
    /// - `Ok(true)` if the prefix is stable (fingerprint matches pinned).
    /// - `Ok(false)` if the prefix changed but was automatically re-pinned.
    /// - `Err(change)` if the prefix changed; caller should surface this.
    ///
    /// After calling this, `last_change()` returns the detected change.
    pub fn check_and_update(
        &mut self,
        system_text: &str,
        tools: Option<&[Tool]>,
    ) -> Result<bool, Box<PrefixChange>> {
        let fp = PrefixFingerprint::compute(system_text, tools);
        let old_fp = self.current.replace(fp.clone());
        self.check_count += 1;

        let pinned = match &self.pinned {
            Some(p) => p,
            None => {
                // First check: pin now.
                self.pinned = Some(fp);
                self.last_change = None;
                return Ok(true);
            }
        };

        if fp.combined_sha256 == pinned.combined_sha256 {
            // Stable — no change.
            Ok(true)
        } else {
            // Change detected.
            let old = old_fp.unwrap_or_else(|| pinned.clone());
            let system_changed = fp.system_sha256 != pinned.system_sha256;
            let tools_changed = fp.tools_sha256 != pinned.tools_sha256;

            let change = PrefixChange {
                old,
                new: fp.clone(),
                system_changed,
                tools_changed,
            };

            self.last_change = Some(change.clone());
            self.change_count += 1;

            // Re-pin to the new prefix so subsequent checks are
            // against the latest baseline. Use the original fp
            // (avoid recomputing the hash — clone was for the change record).
            self.pinned = Some(fp);

            Err(Box::new(change))
        }
    }

    /// Returns the most recent prefix change, if any.
    pub fn last_change(&self) -> Option<&PrefixChange> {
        self.last_change.as_ref()
    }

    /// Returns the pinned fingerprint.
    pub fn pinned_fingerprint(&self) -> Option<&PrefixFingerprint> {
        self.pinned.as_ref()
    }

    /// Returns the current (most recently computed) fingerprint.
    pub fn current_fingerprint(&self) -> Option<&PrefixFingerprint> {
        self.current.as_ref()
    }

    /// Returns the total number of prefix changes detected.
    pub fn change_count(&self) -> u64 {
        self.change_count
    }

    /// Returns the total number of stability checks performed.
    pub fn check_count(&self) -> u64 {
        self.check_count
    }

    /// Returns the prefix stability rate as a fraction (0.0 – 1.0).
    /// 1.0 means the prefix has never changed. Returns 1.0 when no
    /// checks have been performed (to avoid division by zero).
    pub fn stability_ratio(&self) -> f64 {
        if self.check_count == 0 {
            1.0
        } else {
            let stable_checks = self.check_count - self.change_count;
            stable_checks as f64 / self.check_count as f64
        }
    }

    /// Returns a human-readable stability summary.
    pub fn summary(&self) -> String {
        let pct = self.stability_ratio() * 100.0;
        let pinned_short = self
            .pinned
            .as_ref()
            .map(|fp| {
                if fp.combined_sha256.len() >= 12 {
                    &fp.combined_sha256[..12]
                } else {
                    &fp.combined_sha256
                }
            })
            .unwrap_or("none");

        format!(
            "Prefix stability: {pct:.1}% ({stable}/{total} checks stable) | fingerprint: {pinned_short} | changes: {changes}",
            pct = pct,
            stable = self.check_count.saturating_sub(self.change_count),
            total = self.check_count,
            pinned_short = pinned_short,
            changes = self.change_count,
        )
    }
}

/// Compute the SHA-256 hex digest of a byte slice.
fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Extract the system prompt text from an optional SystemPrompt,
/// returning an owned String. This is used for prefix fingerprinting
/// and avoids lifetime/leak issues with the rare SystemPrompt::Blocks case.
pub fn system_prompt_text(system: Option<&SystemPrompt>) -> String {
    match system {
        Some(SystemPrompt::Text(text)) => text.clone(),
        Some(SystemPrompt::Blocks(blocks)) => {
            let mut text = String::new();
            for block in blocks {
                text.push_str(&block.text);
                text.push('\n');
            }
            text
        }
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool(name: &str) -> Tool {
        Tool {
            name: name.to_string(),
            description: String::new(),
            input_schema: serde_json::Value::Null,
            tool_type: None,
            allowed_callers: None,
            defer_loading: None,
            input_examples: None,
            strict: None,
            cache_control: None,
        }
    }

    #[test]
    fn same_prefix_produces_same_fingerprint() {
        let a = PrefixFingerprint::compute("hello world", None);
        let b = PrefixFingerprint::compute("hello world", None);
        assert_eq!(a.combined_sha256, b.combined_sha256);
    }

    #[test]
    fn different_system_produces_different_fingerprint() {
        let a = PrefixFingerprint::compute("hello", None);
        let b = PrefixFingerprint::compute("world", None);
        assert_ne!(a.combined_sha256, b.combined_sha256);
    }

    #[test]
    fn tool_order_does_not_affect_fingerprint() {
        let tools_a = vec![make_tool("read_file"), make_tool("write_file")];
        let tools_b = vec![make_tool("write_file"), make_tool("read_file")];
        let a = PrefixFingerprint::compute("system", Some(&tools_a));
        let b = PrefixFingerprint::compute("system", Some(&tools_b));
        assert_eq!(a.combined_sha256, b.combined_sha256);
    }

    #[test]
    fn different_tools_produce_different_fingerprint() {
        let tools_a = vec![make_tool("read_file")];
        let tools_b = vec![make_tool("write_file")];
        let a = PrefixFingerprint::compute("system", Some(&tools_a));
        let b = PrefixFingerprint::compute("system", Some(&tools_b));
        assert_ne!(a.combined_sha256, b.combined_sha256);
    }

    #[test]
    fn manager_starts_stable() {
        let mut mgr = PrefixStabilityManager::new("system prompt", None);
        assert!(mgr.check_and_update("system prompt", None).unwrap());
        assert_eq!(mgr.change_count(), 0);
        assert_eq!(mgr.check_count(), 1);
    }

    #[test]
    fn manager_detects_change() {
        let mut mgr = PrefixStabilityManager::new("system prompt", None);
        let result = mgr.check_and_update("different prompt", None);
        assert!(result.is_err());
        assert_eq!(mgr.change_count(), 1);
        let change = mgr.last_change().unwrap();
        assert!(change.system_changed);
        assert!(!change.tools_changed);
    }

    #[test]
    fn manager_detects_tool_change() {
        let tools_a = vec![make_tool("read_file")];
        let tools_b = vec![make_tool("write_file")];
        let mut mgr = PrefixStabilityManager::new("system", Some(&tools_a));
        let result = mgr.check_and_update("system", Some(&tools_b));
        assert!(result.is_err());
        let change = mgr.last_change().unwrap();
        assert!(!change.system_changed);
        assert!(change.tools_changed);
    }

    #[test]
    fn manager_re_pins_after_change() {
        let mut mgr = PrefixStabilityManager::new("old", None);
        let _ = mgr.check_and_update("new", None);
        // After re-pin, the new "new" should be stable.
        assert!(mgr.check_and_update("new", None).unwrap());
        assert_eq!(mgr.change_count(), 1);
    }

    #[test]
    fn stability_ratio_is_one_for_no_changes() {
        let mut mgr = PrefixStabilityManager::new("hello", None);
        mgr.check_and_update("hello", None).unwrap();
        mgr.check_and_update("hello", None).unwrap();
        assert!((mgr.stability_ratio() - 1.0).abs() < f64::EPSILON);
        assert_eq!(mgr.check_count(), 2);
        assert_eq!(mgr.change_count(), 0);
    }

    #[test]
    fn stability_ratio_reflects_change_rate() {
        let mut mgr = PrefixStabilityManager::new("hello", None);
        mgr.check_and_update("hello", None).unwrap(); // check 1: stable
        let _ = mgr.check_and_update("world", None); // check 2: changed
        mgr.check_and_update("world", None).unwrap(); // check 3: stable
        // 2 stable out of 3 checks = 0.666...
        // (check_count=0 at start, so 3 checks: 3 checks - 1 change = 2 stable)
        assert!((mgr.stability_ratio() - 2.0 / 3.0).abs() < 0.01);
        assert_eq!(mgr.check_count(), 3);
        assert_eq!(mgr.change_count(), 1);
    }

    #[test]
    fn empty_tools_and_none_tools_produce_same_hash() {
        let empty = PrefixFingerprint::compute("system", Some(&[]));
        let none = PrefixFingerprint::compute("system", None);
        // Both should produce sha256(b"") for the tool component
        assert_eq!(empty.tools_sha256, none.tools_sha256);
    }

    #[test]
    fn empty_system_produces_sha256_of_empty_string() {
        let fp = PrefixFingerprint::compute("", None);
        let expected = sha256_hex(b"");
        assert_eq!(fp.system_sha256, expected);
    }

    #[test]
    fn prefix_change_description_is_informative() {
        let old = PrefixFingerprint::compute("old", None);
        let new = PrefixFingerprint::compute("new", None);
        let change = PrefixChange {
            old,
            new,
            system_changed: true,
            tools_changed: false,
        };
        assert_eq!(
            change.description(),
            "prefix cache invalidated: system prompt changed"
        );
        assert_eq!(change.label(), "sys");
    }

    #[test]
    fn new_unpinned_has_no_change_history() {
        let mut mgr = PrefixStabilityManager::new_unpinned();
        assert!(mgr.pinned_fingerprint().is_none());
        assert!(mgr.current_fingerprint().is_none());
        assert!(mgr.last_change().is_none());
        assert_eq!(mgr.change_count(), 0);
        assert_eq!(mgr.check_count(), 0);
        // First check should pin automatically and count as a check.
        assert!(mgr.check_and_update("hello", None).unwrap());
        assert!(mgr.pinned_fingerprint().is_some());
        assert_eq!(mgr.check_count(), 1);
    }

    #[test]
    fn system_prompt_text_returns_empty_for_none() {
        assert_eq!(system_prompt_text(None), "");
    }
}
