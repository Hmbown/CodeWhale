//! Documentation-only catalog of every user-facing keybinding.
//!
//! This module is the *single source of truth* for what shortcuts the help
//! overlay renders. The actual key handlers live in `tui/ui.rs` (and a few
//! sibling modules); they read keys directly off the crossterm event stream
//! and intentionally do **not** consult this catalog. The catalog exists so
//! that:
//!
//! 1. The help overlay (`tui/views/help.rs`) does not have to maintain a
//!    parallel list that silently rots when a handler is added or moved.
//! 2. New contributors have one place to look when answering "which keys are
//!    bound, and where do they go?"
//!
//! When you add or change a binding in `ui.rs`, **add or update the matching
//! entry here**. The compile-only side-effect of forgetting is a stale help
//! screen; there is no runtime crash, so the discipline lives in code review.
//!
//! Entries are grouped by `KeybindingSection`. The `chord` field is a
//! human-readable string formatted exactly the way it should appear in help —
//! we avoid storing `KeyBinding` values directly because many shortcuts are
//! pairs (`↑/↓`) or families (`1-8`) that don't map cleanly to a single
//! chord.

use std::borrow::Cow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeybindingSection {
    Navigation,
    Editing,
    Submission,
    Modes,
    Sessions,
    Clipboard,
    Help,
}

impl KeybindingSection {
    pub fn label(self, locale: crate::localization::Locale) -> Cow<'static, str> {
        use crate::localization::{MessageId, tr};
        let id = match self {
            Self::Navigation => MessageId::HelpSectionNavigation,
            Self::Editing => MessageId::HelpSectionEditing,
            Self::Submission => MessageId::HelpSectionActions,
            Self::Modes => MessageId::HelpSectionModes,
            Self::Sessions => MessageId::HelpSectionSessions,
            Self::Clipboard => MessageId::HelpSectionClipboard,
            Self::Help => MessageId::HelpSectionHelp,
        };
        tr(locale, id)
    }

    /// Stable ordering for help rendering — matches the variant declaration
    /// order; explicit so adding a section forces a deliberate placement.
    pub fn rank(self) -> u8 {
        match self {
            Self::Navigation => 0,
            Self::Editing => 1,
            Self::Submission => 2,
            Self::Modes => 3,
            Self::Sessions => 4,
            Self::Clipboard => 5,
            Self::Help => 6,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct KeybindingEntry {
    pub chord: &'static str,
    pub description_id: crate::localization::MessageId,
    pub section: KeybindingSection,
}

/// Canonical list of keybindings shown in the help overlay.
///
/// Strings are written in the same notation the existing help screen uses so
/// readers can cross-reference with documentation: `Ctrl+X`, `Alt+X`,
/// `Shift+X`, `↑/↓`, `PgUp/PgDn`, etc. Help renderers may apply per-platform
/// substitutions (e.g. `⌥` for Alt on macOS) at render time, but the catalog
/// itself stores the portable form.
pub const KEYBINDINGS: &[KeybindingEntry] = &[
    // --- Navigation ---
    KeybindingEntry {
        chord: "↑ / ↓",
        description_id: crate::localization::MessageId::KbScrollTranscript,
        section: KeybindingSection::Navigation,
    },
    KeybindingEntry {
        chord: "Alt+↑ / Alt+↓",
        description_id: crate::localization::MessageId::KbScrollTranscriptAlt,
        section: KeybindingSection::Navigation,
    },
    KeybindingEntry {
        chord: "Shift+↑ / Shift+↓",
        description_id: crate::localization::MessageId::KbBrowseHistory,
        section: KeybindingSection::Navigation,
    },
    KeybindingEntry {
        chord: "PgUp / PgDn",
        description_id: crate::localization::MessageId::KbScrollPage,
        section: KeybindingSection::Navigation,
    },
    KeybindingEntry {
        chord: "Ctrl+Home / Ctrl+End",
        description_id: crate::localization::MessageId::KbJumpTopBottom,
        section: KeybindingSection::Navigation,
    },
    KeybindingEntry {
        chord: "Alt+G / Alt+Shift+G",
        description_id: crate::localization::MessageId::KbJumpTopBottomEmpty,
        section: KeybindingSection::Navigation,
    },
    KeybindingEntry {
        chord: "Alt+[ / Alt+]",
        description_id: crate::localization::MessageId::KbJumpToolBlocks,
        section: KeybindingSection::Navigation,
    },
    // --- Editing ---
    KeybindingEntry {
        chord: "← / →",
        description_id: crate::localization::MessageId::KbMoveCursor,
        section: KeybindingSection::Editing,
    },
    KeybindingEntry {
        chord: "Home / End",
        description_id: crate::localization::MessageId::KbJumpLineStartEnd,
        section: KeybindingSection::Editing,
    },
    KeybindingEntry {
        chord: "Ctrl+A / Ctrl+E",
        description_id: crate::localization::MessageId::KbJumpLineStartEnd,
        section: KeybindingSection::Editing,
    },
    KeybindingEntry {
        chord: "Backspace / Delete",
        description_id: crate::localization::MessageId::KbDeleteChar,
        section: KeybindingSection::Editing,
    },
    KeybindingEntry {
        chord: "Ctrl+U",
        description_id: crate::localization::MessageId::KbClearDraft,
        section: KeybindingSection::Editing,
    },
    KeybindingEntry {
        chord: "Ctrl+S",
        description_id: crate::localization::MessageId::KbStashDraft,
        section: KeybindingSection::Editing,
    },
    KeybindingEntry {
        chord: "Alt+R",
        description_id: crate::localization::MessageId::KbSearchHistory,
        section: KeybindingSection::Editing,
    },
    KeybindingEntry {
        chord: "Ctrl+J / Alt+Enter / Shift+Enter",
        description_id: crate::localization::MessageId::KbInsertNewline,
        section: KeybindingSection::Editing,
    },
    // --- Submission / actions ---
    KeybindingEntry {
        chord: "Enter",
        description_id: crate::localization::MessageId::KbSendDraft,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        chord: "Esc",
        description_id: crate::localization::MessageId::KbCloseMenu,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        chord: "Ctrl+C",
        description_id: crate::localization::MessageId::KbCancelOrExit,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        chord: "Ctrl+B",
        description_id: crate::localization::MessageId::KbShellControls,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        chord: "Ctrl+D",
        description_id: crate::localization::MessageId::KbExitEmpty,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        chord: "Ctrl+K",
        description_id: crate::localization::MessageId::KbCommandPalette,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        chord: "Ctrl+X (Activity sidebar)",
        description_id: crate::localization::MessageId::KbCancelBackgroundShellJobs,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        chord: "Ctrl+P",
        description_id: crate::localization::MessageId::KbFuzzyFilePicker,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        // `/context` is the guaranteed path; Alt+C is an unadvertised
        // handler until proven in real terminals (TUI-DOG-003).
        chord: "/context",
        description_id: crate::localization::MessageId::KbCompactInspector,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        chord: "Alt+L",
        description_id: crate::localization::MessageId::KbLastMessagePager,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        // Bare `v` always types `v`; details is Alt+V only (⌥V on macOS).
        chord: "Alt+V",
        description_id: crate::localization::MessageId::KbSelectedDetails,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        chord: "Ctrl+O",
        description_id: crate::localization::MessageId::KbThinkingPager,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        // `/transcript` is the reliable fallback when a terminal cannot
        // distinguish Ctrl+Shift+T from Ctrl+T.
        chord: "/transcript / Ctrl+Shift+T",
        description_id: crate::localization::MessageId::KbLiveTranscript,
        section: KeybindingSection::Submission,
    },
    KeybindingEntry {
        chord: "Ctrl+T",
        description_id: crate::localization::MessageId::KbCycleThinking,
        section: KeybindingSection::Modes,
    },
    KeybindingEntry {
        chord: "Esc Esc",
        description_id: crate::localization::MessageId::KbBacktrackMessage,
        section: KeybindingSection::Submission,
    },
    // --- Modes ---
    KeybindingEntry {
        chord: "Tab",
        description_id: crate::localization::MessageId::KbCompleteCycleModes,
        section: KeybindingSection::Modes,
    },
    KeybindingEntry {
        chord: "Shift+Tab",
        description_id: crate::localization::MessageId::KbCyclePermissions,
        section: KeybindingSection::Modes,
    },
    KeybindingEntry {
        chord: "Alt+1-8",
        description_id: crate::localization::MessageId::KbJumpPlanAgentYolo,
        section: KeybindingSection::Modes,
    },
    KeybindingEntry {
        chord: "Alt+P / Alt+A / Alt+Y",
        description_id: crate::localization::MessageId::KbAltJumpPlanAgentYolo,
        section: KeybindingSection::Modes,
    },
    KeybindingEntry {
        chord: "Alt+! / Alt+@ / Alt+# / Alt+$ / Alt+0 / Ctrl+Alt+0",
        description_id: crate::localization::MessageId::KbFocusSidebar,
        section: KeybindingSection::Modes,
    },
    // --- Sessions ---
    KeybindingEntry {
        chord: "Ctrl+R",
        description_id: crate::localization::MessageId::KbSessionPicker,
        section: KeybindingSection::Sessions,
    },
    // --- Clipboard ---
    KeybindingEntry {
        chord: "Ctrl+V",
        description_id: crate::localization::MessageId::KbPasteAttach,
        section: KeybindingSection::Clipboard,
    },
    KeybindingEntry {
        chord: "Ctrl+Shift+C",
        description_id: crate::localization::MessageId::KbCopySelection,
        section: KeybindingSection::Clipboard,
    },
    KeybindingEntry {
        chord: "Right click",
        description_id: crate::localization::MessageId::KbContextMenu,
        section: KeybindingSection::Clipboard,
    },
    KeybindingEntry {
        chord: "@path",
        description_id: crate::localization::MessageId::KbAttachPath,
        section: KeybindingSection::Clipboard,
    },
    // --- Help ---
    KeybindingEntry {
        // F1 is primary (with /help); Ctrl+/ is the secondary fallback.
        // Alt+? stays an unadvertised handler (TUI-DOG-003).
        chord: "F1 / Ctrl+/",
        description_id: crate::localization::MessageId::KbHelpOverlay,
        section: KeybindingSection::Help,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_non_empty_and_sections_have_entries() {
        assert!(KEYBINDINGS.iter().any(|entry| !entry.chord.is_empty()));
        // Every declared section should appear in the catalog at least once,
        // otherwise the help overlay would render an empty heading.
        let sections = [
            KeybindingSection::Navigation,
            KeybindingSection::Editing,
            KeybindingSection::Submission,
            KeybindingSection::Modes,
            KeybindingSection::Sessions,
            KeybindingSection::Clipboard,
            KeybindingSection::Help,
        ];
        for section in sections {
            assert!(
                KEYBINDINGS.iter().any(|entry| entry.section == section),
                "no entries for section {section:?}"
            );
        }
    }

    #[test]
    fn help_advertises_f1_and_ctrl_slash_never_alt_question() {
        // TUI-DOG-003: Alt+? is not advertised anywhere; F1 (with /help) is
        // primary and Ctrl+/ is the secondary fallback.
        assert!(
            KEYBINDINGS.iter().any(|entry| {
                entry.section == KeybindingSection::Help
                    && entry.chord.contains("F1")
                    && entry.chord.contains("Ctrl+/")
            }),
            "help must document F1 with the Ctrl+/ fallback"
        );
        assert!(
            KEYBINDINGS
                .iter()
                .all(|entry| !entry.chord.contains("Alt+?")),
            "Alt+? must not be advertised in the help catalog"
        );
    }

    #[test]
    fn transcript_navigation_catalog_does_not_advertise_bare_typing_keys() {
        for stale in [
            "g / G",
            "[ / ]",
            "l",
            "?",
            "Ctrl+↑ / Ctrl+↓",
            "v",
            "v / Alt+V",
        ] {
            assert!(
                KEYBINDINGS.iter().all(|entry| entry.chord != stale),
                "stale handler-free chord remains documented: {stale}"
            );
        }
        for wired in ["Alt+G / Alt+Shift+G", "Alt+[ / Alt+]", "Alt+L", "Alt+V"] {
            assert!(
                KEYBINDINGS.iter().any(|entry| entry.chord == wired),
                "wired transcript shortcut missing from help: {wired}"
            );
        }
    }

    #[test]
    fn live_transcript_documents_command_before_shaky_chord() {
        let transcript = KEYBINDINGS
            .iter()
            .find(|entry| entry.description_id == crate::localization::MessageId::KbLiveTranscript)
            .expect("live transcript entry should be documented");

        assert_eq!(transcript.chord, "/transcript / Ctrl+Shift+T");
    }

    #[test]
    fn shell_binding_source_matches_help_catalog_chords() {
        use crate::tui::shell_key_routing::{ShellBindingId, binding};
        assert_eq!(binding(ShellBindingId::ToolDetails).catalog_chord, "Alt+V");
        assert_eq!(
            binding(ShellBindingId::ContextInspector).catalog_chord,
            "/context"
        );
        assert_eq!(binding(ShellBindingId::Help).catalog_chord, "F1 / Ctrl+/");
        for id in [
            ShellBindingId::ToolDetails,
            ShellBindingId::ContextInspector,
            ShellBindingId::Help,
        ] {
            let chord = binding(id).catalog_chord;
            assert!(
                KEYBINDINGS
                    .iter()
                    .any(|entry| entry.chord == chord || entry.chord.contains(chord)),
                "shell binding {id:?} chord missing from help catalog: {chord}"
            );
        }
    }

    #[test]
    fn ctrl_o_help_copy_matches_turn_inspector_behavior() {
        let ctrl_o = KEYBINDINGS
            .iter()
            .find(|entry| entry.chord == "Ctrl+O")
            .expect("Ctrl+O keybinding should be documented");

        // Ctrl+O now opens the whole-turn Turn Inspector (#4104), not the
        // single-cell Activity Detail. The message id is intentionally kept
        // (`KbThinkingPager`) to avoid an existing-symbol rename; only the
        // copy changes.
        assert_eq!(
            ctrl_o.description_id,
            crate::localization::MessageId::KbThinkingPager
        );
        assert_eq!(
            crate::localization::tr(crate::localization::Locale::En, ctrl_o.description_id,),
            "Open Turn Inspector"
        );
    }

    #[test]
    fn ctrl_x_activity_sidebar_cancel_all_is_documented() {
        let ctrl_x_activity = KEYBINDINGS
            .iter()
            .find(|entry| entry.chord == "Ctrl+X (Activity sidebar)")
            .expect("Ctrl+X Activity sidebar keybinding should be documented");

        assert_eq!(
            ctrl_x_activity.description_id,
            crate::localization::MessageId::KbCancelBackgroundShellJobs
        );
    }

    #[test]
    fn tool_details_documents_alt_v_only_never_bare_v() {
        let selected_details = KEYBINDINGS
            .iter()
            .filter(|entry| {
                entry.description_id == crate::localization::MessageId::KbSelectedDetails
            })
            .map(|entry| entry.chord)
            .collect::<Vec<_>>();

        // TUI-DOG-002: bare `v` always types `v`; details is Alt+V only.
        assert_eq!(selected_details, vec!["Alt+V"]);
        assert!(
            KEYBINDINGS
                .iter()
                .all(|entry| entry.chord != "v" && !entry.chord.starts_with("v /")),
            "bare `v` must not be advertised — composer typing owns it"
        );
    }

    #[test]
    fn section_rank_is_a_total_order() {
        let sections = [
            KeybindingSection::Navigation,
            KeybindingSection::Editing,
            KeybindingSection::Submission,
            KeybindingSection::Modes,
            KeybindingSection::Sessions,
            KeybindingSection::Clipboard,
            KeybindingSection::Help,
        ];
        let mut ranks: Vec<u8> = sections.iter().map(|s| s.rank()).collect();
        ranks.sort_unstable();
        ranks.dedup();
        assert_eq!(ranks.len(), sections.len(), "ranks must be unique");
    }
}
