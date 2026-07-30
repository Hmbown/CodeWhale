//! Shell keyboard bindings for details / context / help.
//!
//! Footer hints, help catalog chords, and live handlers must agree on one
//! source. Printable characters always belong to the composer: bare `v`
//! types `v` in every focus state — work surface, transcript selection,
//! panel, or modal (TUI-DOG-002). Details/output fires only on
//! Option+V / Alt+V, and macOS renders the label as `⌥V`, never `Alt`/`Cmd`.
//! Help is `F1` (with `/help`); `Ctrl+/` stays as a secondary fallback.
//! `Alt+?` and `Alt+C` are still accepted where terminals deliver them but
//! are never advertised until proven in real terminals (TUI-DOG-003);
//! `/context` is the guaranteed context path.
//! Ambiguous macOS Option glyphs (`ç` / `¿`) remain text: terminals do not
//! identify whether they came from Option or from a user's keyboard layout.

use std::borrow::Cow;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::key_shortcuts;

/// Stable binding ids shared by handlers, footer hints, and help catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellBindingId {
    ToolDetails,
    ContextInspector,
    Help,
}

/// One advertised binding with the portable catalog chord and focus rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellBinding {
    pub id: ShellBindingId,
    /// Chord shown in help / documentation (portable Alt form; macOS
    /// substitutes `⌥` at render time via [`display_chord`]).
    pub catalog_chord: &'static str,
    /// Compact footer chord when this binding is advertised.
    pub footer_chord: &'static str,
}

/// Canonical shell bindings. Handlers and chrome read from here.
pub const SHELL_BINDINGS: &[ShellBinding] = &[
    ShellBinding {
        id: ShellBindingId::ToolDetails,
        catalog_chord: "Alt+V",
        footer_chord: "Alt+V",
    },
    ShellBinding {
        id: ShellBindingId::ContextInspector,
        // `/context` is the guaranteed path; Alt+C stays an unadvertised
        // handler until proven in Cursor/Terminal.app/iTerm2/tmux/PTY.
        catalog_chord: "/context",
        footer_chord: "/context",
    },
    ShellBinding {
        id: ShellBindingId::Help,
        // `/help` also opens this; Ctrl+/ is the secondary fallback.
        catalog_chord: "F1 / Ctrl+/",
        footer_chord: "F1",
    },
];

#[must_use]
pub fn binding(id: ShellBindingId) -> &'static ShellBinding {
    SHELL_BINDINGS
        .iter()
        .find(|binding| binding.id == id)
        .expect("shell binding catalog is exhaustive")
}

/// Platform-aware chord for opening complete tool or approval details.
#[must_use]
pub fn tool_details_chord() -> Cow<'static, str> {
    display_chord(binding(ShellBindingId::ToolDetails).footer_chord)
}

/// Render a portable `Alt+X` chord for the current platform. macOS normally
/// shows `⌥X`; ASCII-safe terminals retain the portable `Alt+X` spelling.
#[must_use]
pub fn display_chord(chord: &'static str) -> Cow<'static, str> {
    display_chord_for_platform_and_ascii(
        chord,
        cfg!(target_os = "macos"),
        crate::tui::color_compat::ascii_safe_enabled(),
    )
}

#[cfg(test)]
#[must_use]
pub fn display_chord_for_platform(chord: &'static str, is_macos: bool) -> Cow<'static, str> {
    display_chord_for_platform_and_ascii(chord, is_macos, false)
}

fn display_chord_for_platform_and_ascii(
    chord: &'static str,
    is_macos: bool,
    ascii_safe: bool,
) -> Cow<'static, str> {
    if ascii_safe {
        return Cow::Borrowed(chord);
    }
    if !is_macos {
        return Cow::Borrowed(chord);
    }
    let rendered = chord.replace("Alt+", "⌥").replace("F1", "fn+F1");
    if rendered == chord {
        Cow::Borrowed(chord)
    } else {
        Cow::Owned(rendered)
    }
}

/// Footer right-hand action hints. Placeholders (`{output}`, `{context}`,
/// `{keys}`) are localized by the caller.
#[must_use]
pub fn footer_action_hints(include_context: bool) -> String {
    footer_action_hints_for_platform_and_ascii(
        include_context,
        cfg!(target_os = "macos"),
        crate::tui::color_compat::ascii_safe_enabled(),
    )
}

#[cfg(test)]
#[must_use]
pub fn footer_action_hints_for_platform(include_context: bool, is_macos: bool) -> String {
    footer_action_hints_for_platform_and_ascii(include_context, is_macos, false)
}

fn footer_action_hints_for_platform_and_ascii(
    include_context: bool,
    is_macos: bool,
    ascii_safe: bool,
) -> String {
    let details = display_chord_for_platform_and_ascii(
        binding(ShellBindingId::ToolDetails).footer_chord,
        is_macos,
        ascii_safe,
    );
    let help = display_chord_for_platform_and_ascii(
        binding(ShellBindingId::Help).footer_chord,
        is_macos,
        ascii_safe,
    );
    if include_context {
        format!(
            "{details}:{{output}} · {}:{{context}} · {help}:{{keys}}",
            binding(ShellBindingId::ContextInspector).footer_chord
        )
    } else {
        format!("{details}:{{output}} · {help}:{{keys}}")
    }
}

/// Details/output opens only on Option+V (macOS legacy `√`) or Alt+V.
/// Bare `v` always types `v` — never a shortcut, in any focus state.
#[must_use]
pub fn is_tool_details_shortcut(key: &KeyEvent) -> bool {
    if key_shortcuts::is_macos_option_v_legacy_key(key) {
        return true;
    }
    matches!(key.code, KeyCode::Char('v') | KeyCode::Char('V'))
        && key_shortcuts::alt_nav_modifiers(key.modifiers)
}

#[must_use]
pub fn is_context_inspector_shortcut(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        && key_shortcuts::alt_nav_modifiers(key.modifiers)
}

#[must_use]
pub fn is_help_shortcut(key: &KeyEvent) -> bool {
    if matches!(key.code, KeyCode::F(1)) {
        return true;
    }
    // AltGr on European/Latin keyboards (e.g. Brazilian ABNT2, where `/` is
    // AltGr+Q, or AZERTY) is reported by Windows as Ctrl+Alt. Such chords are
    // AltGr-typed text, not the `Ctrl+/` help chord, so leave them to the
    // composer as a literal character. Reuse the same platform-gated primitive
    // the composer's `is_plain_char` guard uses so the two agree exactly on
    // what counts as text; Alt-only chords (e.g. the unadvertised Alt+? below)
    // stay live. Same philosophy as the sidebar-focus fix in #2863/#2867.
    let is_altgr = crate::tui::widgets::key_hint::is_altgr(key.modifiers);
    if !is_altgr
        && matches!(key.code, KeyCode::Char('/'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
    {
        return true;
    }
    // Some legacy terminal stacks encode Ctrl+/ as the ASCII unit separator,
    // which crossterm reports as Ctrl+7 or Ctrl+_. Accept both portable
    // decodings so the documented fallback remains real.
    if !is_altgr
        && matches!(key.code, KeyCode::Char('7') | KeyCode::Char('_'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
    {
        return true;
    }
    // Alt+? still opens help where the terminal delivers it, but it is not
    // advertised anywhere (TUI-DOG-003).
    matches!(key.code, KeyCode::Char('?')) && key_shortcuts::alt_nav_modifiers(key.modifiers)
}

#[must_use]
pub fn is_settings_shortcut(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::F(2)) && key.modifiers.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_v_is_never_a_shortcut_in_any_state() {
        // TUI-DOG-002: bare `v` always types `v`; there is no focus state in
        // which it opens details, so the matcher takes no focus argument.
        let plain_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE);
        assert!(!is_tool_details_shortcut(&plain_v));
        let plain_upper_v = KeyEvent::new(KeyCode::Char('V'), KeyModifiers::SHIFT);
        assert!(!is_tool_details_shortcut(&plain_upper_v));
    }

    #[test]
    fn alt_v_and_macos_option_v_open_details() {
        let alt_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT);
        assert!(is_tool_details_shortcut(&alt_v));
        let alt_upper_v = KeyEvent::new(KeyCode::Char('V'), KeyModifiers::ALT);
        assert!(is_tool_details_shortcut(&alt_upper_v));
    }

    #[test]
    fn details_label_is_option_glyph_on_macos_and_alt_elsewhere() {
        assert_eq!(display_chord_for_platform("Alt+V", true), "⌥V");
        assert_eq!(display_chord_for_platform("Alt+V", false), "Alt+V");
        let macos = footer_action_hints_for_platform(true, true);
        assert!(macos.starts_with("⌥V:"), "{macos}");
        assert!(!macos.contains("Alt"), "{macos}");
        assert!(!macos.contains("Cmd"), "{macos}");
        let other = footer_action_hints_for_platform(true, false);
        assert!(other.starts_with("Alt+V:"), "{other}");
    }

    #[test]
    fn ascii_safe_macos_hints_keep_portable_chords() {
        assert_eq!(
            display_chord_for_platform_and_ascii("Alt+V", true, true),
            "Alt+V"
        );
        let hints = footer_action_hints_for_platform_and_ascii(true, true, true);
        assert!(hints.starts_with("Alt+V:"), "{hints}");
        assert!(hints.contains("F1:"), "{hints}");
        assert!(!hints.contains('⌥'), "{hints}");
    }

    #[test]
    fn footer_hints_never_advertise_bare_v_alt_question_or_alt_c() {
        for is_macos in [true, false] {
            for include_context in [true, false] {
                let hints = footer_action_hints_for_platform(include_context, is_macos);
                assert!(!hints.starts_with("v:"), "{hints}");
                assert!(!hints.contains(" v:"), "{hints}");
                assert!(!hints.contains("Alt+?"), "{hints}");
                assert!(!hints.contains("Alt+C"), "{hints}");
                assert!(hints.contains("F1:"), "{hints}");
                if is_macos {
                    assert!(hints.contains("fn+F1:"), "{hints}");
                }
                if include_context {
                    assert!(hints.contains("/context:"), "{hints}");
                }
            }
        }
    }

    #[test]
    fn help_accepts_f1_ctrl_slash_and_unadvertised_fallbacks() {
        assert!(is_help_shortcut(&KeyEvent::new(
            KeyCode::F(1),
            KeyModifiers::NONE
        )));
        assert!(is_help_shortcut(&KeyEvent::new(
            KeyCode::Char('/'),
            KeyModifiers::CONTROL
        )));
        assert!(is_help_shortcut(&KeyEvent::new(
            KeyCode::Char('7'),
            KeyModifiers::CONTROL
        )));
        assert!(is_help_shortcut(&KeyEvent::new(
            KeyCode::Char('_'),
            KeyModifiers::CONTROL
        )));
        // Unadvertised but accepted where the terminal delivers them.
        assert!(is_help_shortcut(&KeyEvent::new(
            KeyCode::Char('?'),
            KeyModifiers::ALT
        )));
        let inverted_question = KeyEvent::new(KeyCode::Char('\u{00bf}'), KeyModifiers::NONE);
        assert!(!is_help_shortcut(&inverted_question));
    }

    // On Windows, AltGr is delivered as Ctrl+Alt (e.g. Brazilian ABNT2 types
    // `/` via AltGr+Q, AZERTY via other keys). Those chords must fall through
    // to the composer as literal characters instead of opening help. This is
    // platform-specific: `key_hint::is_altgr` only treats Ctrl+Alt as AltGr on
    // Windows, matching the composer's own `is_plain_char` guard.
    #[cfg(windows)]
    #[test]
    fn altgr_typed_slash_is_text_not_the_help_chord() {
        let altgr = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL | KeyModifiers::ALT);
        assert!(!is_help_shortcut(&altgr('/')));
        // The legacy ASCII-unit-separator fallbacks must stay guarded too, so
        // an AltGr chord that happens to land on `7`/`_` still types text.
        assert!(!is_help_shortcut(&altgr('7')));
        assert!(!is_help_shortcut(&altgr('_')));
        // Plain Ctrl+/ (no Alt) still opens help.
        assert!(is_help_shortcut(&KeyEvent::new(
            KeyCode::Char('/'),
            KeyModifiers::CONTROL
        )));
    }

    // Off Windows there is no AltGr remapping, so a genuine Ctrl+Alt+/ chord
    // is not layout text and help still fires — the fix must not regress it.
    #[cfg(not(windows))]
    #[test]
    fn ctrl_alt_slash_still_opens_help_off_windows() {
        let chord = KeyEvent::new(
            KeyCode::Char('/'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        );
        assert!(is_help_shortcut(&chord));
        assert!(is_help_shortcut(&KeyEvent::new(
            KeyCode::Char('/'),
            KeyModifiers::CONTROL
        )));
    }

    #[test]
    fn settings_accepts_only_plain_f2() {
        assert!(is_settings_shortcut(&KeyEvent::new(
            KeyCode::F(2),
            KeyModifiers::NONE
        )));
        assert!(!is_settings_shortcut(&KeyEvent::new(
            KeyCode::F(2),
            KeyModifiers::SHIFT
        )));
        assert!(!is_settings_shortcut(&KeyEvent::new(
            KeyCode::F(1),
            KeyModifiers::NONE
        )));
    }

    #[test]
    fn context_accepts_explicit_alt_c_without_stealing_layout_characters() {
        let alt_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::ALT);
        assert!(is_context_inspector_shortcut(&alt_c));
        let cedilla = KeyEvent::new(KeyCode::Char('\u{00e7}'), KeyModifiers::NONE);
        assert!(!is_context_inspector_shortcut(&cedilla));
    }

    #[test]
    fn catalog_chords_match_final_contract() {
        assert_eq!(binding(ShellBindingId::Help).catalog_chord, "F1 / Ctrl+/");
        assert_eq!(
            binding(ShellBindingId::ContextInspector).catalog_chord,
            "/context"
        );
        assert_eq!(binding(ShellBindingId::ToolDetails).catalog_chord, "Alt+V");
        for binding in SHELL_BINDINGS {
            assert!(!binding.catalog_chord.contains("Alt+?"));
            assert_ne!(binding.catalog_chord, "v");
            assert!(!binding.catalog_chord.starts_with("v /"));
            assert!(!binding.footer_chord.contains("Alt+?"));
            assert_ne!(binding.footer_chord, "v");
        }
    }
}
