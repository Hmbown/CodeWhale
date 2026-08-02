//! DeepSeek pro actual-effort mapping — the single source of truth (#5055).
//!
//! [`DEEPSEEK_EFFORT_MAP`] pins how CodeWhale's requested reasoning-effort
//! tiers translate to the effort labels DeepSeek accepts on the wire, for
//! BOTH request dialects:
//!
//! - Chat Completions (`super::apply_reasoning_effort`): top-level
//!   `reasoning_effort` plus the `thinking` enable/disable toggle.
//! - Responses (`super::responses::responses_reasoning_effort`): the
//!   `reasoning.effort` field.
//!
//! Mapping per the DeepSeek API docs dated 2026-07-31 (the same doc revision
//! as the DeepSeek-V4-Flash-0731 Responses contract). DeepSeek documents that
//! the pro actual-effort mapping changes in early August 2026 — when it does,
//! edit this table (and the doc date here), not the call sites. The former
//! inline sites carry pointer comments back to this module so the labels do
//! not get re-inlined.

use serde_json::{Value, json};

/// What the Chat Completions path emits for a requested effort tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChatEffort {
    /// Emit `"thinking": {"type": "disabled"}` and no `reasoning_effort`.
    DisableThinking,
    /// Emit `"reasoning_effort": <label>` and `"thinking": {"type": "enabled"}`.
    Label(&'static str),
}

/// One row of the DeepSeek effort mapping.
pub(super) struct DeepseekEffortRow {
    /// Normalized (trimmed, ASCII-lowercased) requested effort tier.
    pub alias: &'static str,
    /// Chat Completions behavior. `None` leaves the body untouched — the
    /// Chat path does not recognize the alias today.
    pub chat: Option<ChatEffort>,
    /// Responses `reasoning.effort` label.
    pub responses: &'static str,
}

/// Responses-path label for any tier absent from the table: DeepSeek maps
/// every remaining/automatic tier to its normal high tier in thinking mode.
/// (The Chat path instead ignores unknown tiers entirely.)
pub(super) const DEEPSEEK_RESPONSES_DEFAULT_EFFORT: &str = "high";

/// DeepSeek pro actual-effort mapping, per the docs dated 2026-07-31.
///
/// Asymmetries between the two columns are deliberate and pre-date this
/// table; they are pinned by tests and must only change when the DeepSeek
/// docs do:
/// - Chat collapses `low`/`minimal` to `high` (DeepSeek maps both low and
///   medium to its normal high tier in thinking mode), while Responses
///   preserves an explicit `low` for Codex compatibility.
/// - Responses has no off switch, so disabled tiers send its lowest
///   documented effort; Chat disables thinking outright.
/// - `maximum` is only recognized by the Responses path, and `highest` only
///   reaches `max` on the Chat path.
pub(super) const DEEPSEEK_EFFORT_MAP: &[DeepseekEffortRow] = &[
    DeepseekEffortRow {
        alias: "off",
        chat: Some(ChatEffort::DisableThinking),
        responses: "low",
    },
    DeepseekEffortRow {
        alias: "disabled",
        chat: Some(ChatEffort::DisableThinking),
        responses: "low",
    },
    DeepseekEffortRow {
        alias: "none",
        chat: Some(ChatEffort::DisableThinking),
        responses: "low",
    },
    DeepseekEffortRow {
        alias: "false",
        chat: Some(ChatEffort::DisableThinking),
        responses: "low",
    },
    DeepseekEffortRow {
        alias: "minimal",
        chat: Some(ChatEffort::Label("high")),
        responses: "low",
    },
    DeepseekEffortRow {
        alias: "low",
        chat: Some(ChatEffort::Label("high")),
        responses: "low",
    },
    DeepseekEffortRow {
        alias: "medium",
        chat: Some(ChatEffort::Label("high")),
        responses: "high",
    },
    DeepseekEffortRow {
        alias: "mid",
        chat: Some(ChatEffort::Label("high")),
        responses: "high",
    },
    // Empty string is the "unspecified" tier the Chat path groups with high.
    DeepseekEffortRow {
        alias: "",
        chat: Some(ChatEffort::Label("high")),
        responses: "high",
    },
    DeepseekEffortRow {
        alias: "high",
        chat: Some(ChatEffort::Label("high")),
        responses: "high",
    },
    DeepseekEffortRow {
        alias: "xhigh",
        chat: Some(ChatEffort::Label("max")),
        responses: "max",
    },
    DeepseekEffortRow {
        alias: "max",
        chat: Some(ChatEffort::Label("max")),
        responses: "max",
    },
    DeepseekEffortRow {
        alias: "maximum",
        chat: None,
        responses: "max",
    },
    DeepseekEffortRow {
        alias: "highest",
        chat: Some(ChatEffort::Label("max")),
        responses: DEEPSEEK_RESPONSES_DEFAULT_EFFORT,
    },
    DeepseekEffortRow {
        alias: "ultracode",
        chat: Some(ChatEffort::Label("max")),
        responses: "max",
    },
];

/// Chat Completions mapping for a normalized requested tier.
pub(super) fn chat_effort(normalized: &str) -> Option<ChatEffort> {
    DEEPSEEK_EFFORT_MAP
        .iter()
        .find(|row| row.alias == normalized)
        .and_then(|row| row.chat)
}

/// Responses `reasoning.effort` label for a normalized requested tier.
pub(super) fn responses_effort(normalized: &str) -> &'static str {
    DEEPSEEK_EFFORT_MAP
        .iter()
        .find(|row| row.alias == normalized)
        .map_or(DEEPSEEK_RESPONSES_DEFAULT_EFFORT, |row| row.responses)
}

/// Applies the DeepSeek Chat Completions mapping for `normalized` (a trimmed,
/// ASCII-lowercased requested tier) to a Chat request `body`.
pub(super) fn apply_deepseek_chat_effort(body: &mut Value, normalized: &str) {
    match chat_effort(normalized) {
        Some(ChatEffort::DisableThinking) => {
            body["thinking"] = json!({ "type": "disabled" });
        }
        Some(ChatEffort::Label(label)) => {
            body["reasoning_effort"] = json!(label);
            body["thinking"] = json!({ "type": "enabled" });
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_column_pins_2026_07_31_docs() {
        for alias in ["off", "disabled", "none", "false"] {
            assert_eq!(
                chat_effort(alias),
                Some(ChatEffort::DisableThinking),
                "{alias}"
            );
        }
        for alias in ["minimal", "low", "medium", "mid", "", "high"] {
            assert_eq!(
                chat_effort(alias),
                Some(ChatEffort::Label("high")),
                "{alias}"
            );
        }
        for alias in ["xhigh", "max", "highest", "ultracode"] {
            assert_eq!(
                chat_effort(alias),
                Some(ChatEffort::Label("max")),
                "{alias}"
            );
        }
        // Unrecognized on the Chat path: leave the body untouched.
        assert_eq!(chat_effort("maximum"), None);
        assert_eq!(chat_effort("unknown-tier"), None);
    }

    #[test]
    fn responses_column_pins_2026_07_31_docs() {
        for alias in ["off", "disabled", "none", "false", "minimal", "low"] {
            assert_eq!(responses_effort(alias), "low", "{alias}");
        }
        for alias in ["medium", "mid", "", "high", "highest"] {
            assert_eq!(responses_effort(alias), "high", "{alias}");
        }
        for alias in ["xhigh", "max", "maximum", "ultracode"] {
            assert_eq!(responses_effort(alias), "max", "{alias}");
        }
        // Unknown tiers fall back to the documented default.
        assert_eq!(responses_effort("unknown-tier"), "high");
    }
}
