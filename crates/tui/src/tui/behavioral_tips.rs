//! Quiet, action-triggered product guidance.
//!
//! These tips are deliberately event-driven rather than timer-driven. The
//! session gate keeps the TUI calm, while persisted impression counts prevent
//! a useful first-run hint from becoming permanent chrome.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

use crate::localization::{Locale, MessageId, tr};
use crate::settings::Settings;
use crate::tui::app::{App, AppMode, StatusToastLevel};

const MAX_TIPS_PER_SESSION: u8 = 1;
const MAX_LIFETIME_IMPRESSIONS: u8 = 2;
const MAX_TRACKED_MANUAL_COMMANDS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BehavioralTip {
    PlanningMode,
    BackgroundJobReceipt,
    ClearedInputRestore,
    McpValidation,
    RepeatedCommandHotbar,
}

impl BehavioralTip {
    const fn key(self) -> &'static str {
        match self {
            Self::PlanningMode => "planning_mode",
            Self::BackgroundJobReceipt => "background_job_receipt",
            Self::ClearedInputRestore => "cleared_input_restore",
            Self::McpValidation => "mcp_validation",
            Self::RepeatedCommandHotbar => "repeated_command_hotbar",
        }
    }

    const fn message_id(self) -> MessageId {
        match self {
            Self::PlanningMode => MessageId::BehavioralTipPlanning,
            Self::BackgroundJobReceipt => MessageId::BehavioralTipBackgroundReceipt,
            Self::ClearedInputRestore => MessageId::BehavioralTipClearedInput,
            Self::McpValidation => MessageId::BehavioralTipMcpValidation,
            Self::RepeatedCommandHotbar => MessageId::BehavioralTipRepeatedCommand,
        }
    }

    fn message(self, locale: Locale) -> String {
        let template = tr(locale, self.message_id());
        match self {
            Self::PlanningMode => template.replace("{key}", "Tab"),
            Self::BackgroundJobReceipt => template.replace("{key}", "Enter"),
            Self::ClearedInputRestore => template.replace("{chord}", "Ctrl+Z"),
            Self::McpValidation => template.replace("{command}", "codewhale mcp validate"),
            Self::RepeatedCommandHotbar => template.replace("{command}", "/hotbar"),
        }
    }
}

#[derive(Debug, Default)]
pub struct BehavioralTipState {
    shown_this_session: HashSet<BehavioralTip>,
    session_impressions: u8,
    manual_command_counts: HashMap<u64, u8>,
}

impl BehavioralTipState {
    fn eligible_in_session(&self, tip: BehavioralTip) -> bool {
        self.session_impressions < MAX_TIPS_PER_SESSION && !self.shown_this_session.contains(&tip)
    }

    fn eligible(&self, tip: BehavioralTip, lifetime_impressions: u8) -> bool {
        self.eligible_in_session(tip) && lifetime_impressions < MAX_LIFETIME_IMPRESSIONS
    }

    fn record_impression(&mut self, tip: BehavioralTip) {
        self.shown_this_session.insert(tip);
        self.session_impressions = self.session_impressions.saturating_add(1);
    }

    fn note_manual_command(&mut self, input: &str) -> bool {
        let Some(fingerprint) = manual_command_fingerprint(input) else {
            return false;
        };
        if self.manual_command_counts.len() >= MAX_TRACKED_MANUAL_COMMANDS
            && !self.manual_command_counts.contains_key(&fingerprint)
        {
            return false;
        }
        let count = self.manual_command_counts.entry(fingerprint).or_default();
        *count = count.saturating_add(1);
        *count == 3
    }
}

impl App {
    /// Show a behavioral tip when both the quiet session cap and the persisted
    /// lifetime cap allow it. Persistence is best-effort: a read-only home
    /// must not make a useful in-session hint fail closed.
    pub fn maybe_show_behavioral_tip(&mut self, tip: BehavioralTip) -> bool {
        // Clear-input hooks are hot paths. Once the in-memory session gate is
        // closed, avoid touching the settings file for every later keypress.
        if !self.behavioral_tips.eligible_in_session(tip) {
            return false;
        }
        let mut settings = if cfg!(test) {
            Settings::default()
        } else {
            Settings::load_persisted().unwrap_or_default()
        };
        let lifetime_impressions = settings
            .behavioral_tip_impressions
            .get(tip.key())
            .copied()
            .unwrap_or(0);
        if !self.behavioral_tips.eligible(tip, lifetime_impressions) {
            return false;
        }

        self.behavioral_tips.record_impression(tip);
        settings.behavioral_tip_impressions.insert(
            tip.key().to_string(),
            lifetime_impressions.saturating_add(1),
        );
        if !cfg!(test)
            && let Err(err) = settings.save()
        {
            tracing::warn!(tip = tip.key(), error = %err, "behavioral tip impression was not persisted");
        }
        self.push_status_toast(
            tip.message(self.ui_locale),
            StatusToastLevel::Info,
            Some(8_000),
        );
        true
    }

    pub fn maybe_nudge_for_planning_prompt(&mut self, input: &str) -> bool {
        self.mode != AppMode::Plan
            && looks_like_planning_prompt(input)
            && self.maybe_show_behavioral_tip(BehavioralTip::PlanningMode)
    }

    pub fn note_manual_command_for_tip(&mut self, input: &str) -> bool {
        self.behavioral_tips.note_manual_command(input)
            && self.maybe_show_behavioral_tip(BehavioralTip::RepeatedCommandHotbar)
    }
}

fn manual_command_fingerprint(input: &str) -> Option<u64> {
    let parts = input.split_whitespace().collect::<Vec<_>>();
    let command = parts.first()?;
    if !command.starts_with('/') || command.eq_ignore_ascii_case("/hotbar") {
        return None;
    }
    let normalized = parts.join(" ");
    let mut hasher = DefaultHasher::new();
    normalized.hash(&mut hasher);
    Some(hasher.finish())
}

fn looks_like_planning_prompt(input: &str) -> bool {
    let normalized = input
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { ' ' })
        .collect::<String>();
    let words = normalized.split_whitespace().collect::<Vec<_>>();
    let has_word = |needle: &str| words.contains(&needle);

    ["plan", "planning", "roadmap", "strategy", "outline"]
        .into_iter()
        .any(has_word)
        || normalized.contains("how should we")
        || normalized.contains("before we start")
        || normalized.contains("step by step")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planning_detector_matches_intent_without_substring_false_positives() {
        assert!(looks_like_planning_prompt(
            "Please outline a migration strategy"
        ));
        assert!(looks_like_planning_prompt("How should we approach this?"));
        assert!(!looks_like_planning_prompt(
            "Explain the planetary boundary"
        ));
        assert!(!looks_like_planning_prompt("Fix the failing test"));
    }

    #[test]
    fn session_and_lifetime_caps_keep_tips_quiet() {
        let mut state = BehavioralTipState::default();
        assert!(state.eligible(BehavioralTip::PlanningMode, 0));
        state.record_impression(BehavioralTip::PlanningMode);
        assert!(!state.eligible(BehavioralTip::PlanningMode, 0));
        assert!(!state.eligible(BehavioralTip::McpValidation, 0));

        let fresh_session = BehavioralTipState::default();
        assert!(fresh_session.eligible(BehavioralTip::PlanningMode, 1));
        assert!(!fresh_session.eligible(BehavioralTip::PlanningMode, MAX_LIFETIME_IMPRESSIONS));
    }

    #[test]
    fn third_matching_manual_command_triggers_once() {
        let mut state = BehavioralTipState::default();
        assert!(!state.note_manual_command("/model one"));
        assert!(!state.note_manual_command("/model two"));
        assert!(!state.note_manual_command(" /model   one "));
        assert!(state.note_manual_command("/model one"));
        assert!(!state.note_manual_command("/model one"));
        assert!(!state.note_manual_command("/hotbar"));
        assert!(!state.note_manual_command("ordinary prompt"));
    }

    #[test]
    fn every_complete_locale_renders_tips_with_code_owned_controls() {
        let tips = [
            BehavioralTip::PlanningMode,
            BehavioralTip::BackgroundJobReceipt,
            BehavioralTip::ClearedInputRestore,
            BehavioralTip::McpValidation,
            BehavioralTip::RepeatedCommandHotbar,
        ];
        for locale in Locale::shipped_complete() {
            for tip in tips {
                let message = tip.message(*locale);
                assert!(!message.contains('{'), "unexpanded placeholder: {message}");
            }
        }

        assert_eq!(
            BehavioralTip::PlanningMode.message(Locale::En),
            "Planning? Tab cycles to Plan mode"
        );
        assert_eq!(
            BehavioralTip::BackgroundJobReceipt.message(Locale::En),
            "Receipts live in the Work panel — Enter opens the inspector"
        );
        assert_eq!(
            BehavioralTip::ClearedInputRestore.message(Locale::En),
            "Cleared · Ctrl+Z restores"
        );
        assert_eq!(
            BehavioralTip::McpValidation.message(Locale::En),
            "codewhale mcp validate starts servers and shows why"
        );
        assert_eq!(
            BehavioralTip::RepeatedCommandHotbar.message(Locale::En),
            "/hotbar can pin this"
        );
    }
}
