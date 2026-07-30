//! Keyboard-shortcut predicates and platform-specific labels.
//!
//! These helpers normalise the cross-platform variations between
//! `Ctrl+…` (Linux/Windows) and `Cmd+…` (macOS), legacy `Ctrl+H`-as-
//! backspace handling, and the macOS Option-Latin-character escapes.
//! Centralising them
//! keeps the composer / transcript event loops in `ui.rs` short and
//! lets us add a new platform without touching the call sites.

use std::borrow::Cow;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(super) fn has_control_like_modifier(modifiers: KeyModifiers) -> bool {
    has_control_like_modifier_for_platform(modifiers, cfg!(target_os = "macos"))
}

pub(super) fn has_control_like_modifier_for_platform(
    modifiers: KeyModifiers,
    is_macos: bool,
) -> bool {
    modifiers.contains(KeyModifiers::CONTROL)
        || (is_macos && modifiers.contains(KeyModifiers::SUPER))
}

/// Compatibility path for enhanced terminal clients that forward `Cmd+C` or
/// `Ctrl+Shift+C` as key events. Most terminals consume these locally, so the
/// user-visible Codewhale binding remains `Ctrl+C` with an active selection.
pub(super) fn is_copy_shortcut(key: &KeyEvent) -> bool {
    let is_c = matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'));
    if !is_c {
        return false;
    }

    if key.modifiers.contains(KeyModifiers::SUPER) {
        return true;
    }

    key.modifiers.contains(KeyModifiers::CONTROL) && key.modifiers.contains(KeyModifiers::SHIFT)
}

/// Toggle the file-tree pane: `Ctrl+Shift+E` on Linux/Windows or
/// `Cmd+Shift+E` on macOS.
pub(super) fn is_file_tree_toggle_shortcut(key: &KeyEvent) -> bool {
    let is_shifted_e = matches!(key.code, KeyCode::Char('E'))
        || (matches!(key.code, KeyCode::Char('e')) && key.modifiers.contains(KeyModifiers::SHIFT));
    if !is_shifted_e {
        return false;
    }

    let has_forbidden_modifier =
        key.modifiers.contains(KeyModifiers::ALT) || key.modifiers.contains(KeyModifiers::SUPER);
    let ctrl_shift_e = key.modifiers.contains(KeyModifiers::CONTROL) && !has_forbidden_modifier;

    let cmd_shift_e = key.modifiers.contains(KeyModifiers::SUPER)
        && key.modifiers.contains(KeyModifiers::SHIFT)
        && !key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT);

    ctrl_shift_e || cmd_shift_e
}

pub(super) fn tool_details_shortcut_label() -> Cow<'static, str> {
    crate::tui::shell_key_routing::tool_details_chord()
}

pub(super) fn tool_details_shortcut_action_hint(noun: &str) -> String {
    format!("{} opens {noun}", tool_details_shortcut_label())
}

pub(super) fn reasoning_detail_shortcut_label() -> &'static str {
    "Ctrl+O"
}

/// Open the full reasoning detail pager for the selected or current turn.
/// Ctrl+O now shows the recorded reasoning timeline, not the whole-turn
/// inspector (#v092-reasoning-fix).
pub(super) fn is_reasoning_detail_shortcut(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('o') | KeyCode::Char('O'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && !key
            .modifiers
            .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::SUPER)
}

pub(super) fn turn_inspector_shortcut_label() -> &'static str {
    "Ctrl+Alt+O"
}

/// Open the whole-turn inspector on a dedicated, collision-free chord.
/// Ctrl+Alt+O was free in the keybinding registry; it is distinct from
/// Ctrl+O (reasoning detail) and Ctrl+Shift+O (external editor).
pub(super) fn is_turn_inspector_shortcut(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('o') | KeyCode::Char('O'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && key.modifiers.contains(KeyModifiers::ALT)
        && !key
            .modifiers
            .intersects(KeyModifiers::SHIFT | KeyModifiers::SUPER)
}

/// Open the composer draft in `$VISUAL` / `$EDITOR` without colliding with
/// the reasoning detail or Turn Inspector shortcuts. Enhanced protocols can
/// report either character case, but SHIFT must be explicit so Windows Caps
/// Lock cannot misroute Ctrl+O. F4 is the fallback for legacy protocols that
/// cannot encode Ctrl+Shift+O.
pub(super) fn is_external_editor_shortcut(key: &KeyEvent) -> bool {
    let ctrl_shift_o = matches!(key.code, KeyCode::Char('o') | KeyCode::Char('O'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && key.modifiers.contains(KeyModifiers::SHIFT)
        && !key
            .modifiers
            .intersects(KeyModifiers::ALT | KeyModifiers::SUPER);
    let f4 = matches!(key.code, KeyCode::F(4)) && key.modifiers.is_empty();
    ctrl_shift_o || f4
}

/// Select the whole composer draft. `Ctrl+A` is intentionally NOT select-all:
/// it keeps its readline meaning (jump to start of input), matching every
/// other emacs-style binding in the composer. Select-all is therefore:
///
/// - `Ctrl+Shift+A` on every platform (mirrors `Ctrl+Shift+O` / `Ctrl+Shift+E`
///   precedent for shifted-Ctrl chords; requires an enhanced-keyboard
///   terminal, like those precedents).
/// - `Cmd+A` on macOS terminals that forward Cmd to the app (kitty, WezTerm,
///   iTerm2 with "Left/Right Command" remapping). The event-loop macOS
///   normalization deliberately skips this chord so `Cmd+A` is not collapsed
///   into readline `Ctrl+A`. `Cmd+Shift+A` also lands here after
///   normalization.
pub(super) fn is_select_all_shortcut(key: &KeyEvent) -> bool {
    let is_a = matches!(key.code, KeyCode::Char('a') | KeyCode::Char('A'));
    if !is_a {
        return false;
    }
    let cmd_a = key.modifiers.contains(KeyModifiers::SUPER)
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    let ctrl_shift_a = key.modifiers.contains(KeyModifiers::CONTROL)
        && key.modifiers.contains(KeyModifiers::SHIFT)
        && !key
            .modifiers
            .intersects(KeyModifiers::ALT | KeyModifiers::SUPER);
    cmd_a || ctrl_shift_a
}

/// Modifier predicate for the v0.8.30 family of `Alt+<key>` transcript-
/// nav shortcuts (`Alt+G` / `Alt+[` / `Alt+]` / `Alt+?` / `Alt+L`). Requires
/// `Alt` and disallows `Ctrl` / `Super` so the
/// bindings don't collide with platform clipboard / window-management
/// shortcuts. `Shift` is permitted so the capital-letter forms work on
/// any keyboard layout that produces them as `Alt+Shift+key`.
///
/// Plain `Char` events (no modifier, or modifier=`Shift` alone for the
/// uppercase form) fall through to text insertion, which is the whole
/// point — typing "good morning" no longer eats the first `g`.
pub(super) fn alt_nav_modifiers(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::ALT)
        && !modifiers.contains(KeyModifiers::CONTROL)
        && !modifiers.contains(KeyModifiers::SUPER)
}

pub(super) fn is_macos_option_v_legacy_key(key: &KeyEvent) -> bool {
    is_macos_option_v_legacy_key_for_platform(key, cfg!(target_os = "macos"))
}

pub(super) fn is_macos_option_v_legacy_key_for_platform(key: &KeyEvent, is_macos: bool) -> bool {
    is_macos && key.modifiers.is_empty() && matches!(key.code, KeyCode::Char('\u{221A}'))
}

/// Paste-from-clipboard: accept `Cmd+V`, `Ctrl+V`, or the legacy raw `\u{16}`
/// byte some terminals emit. A remote terminal normally consumes its local
/// paste chord and sends an `Event::Paste`; accepting both modifier families
/// still keeps enhanced-keyboard clients independent of the remote host OS.
pub(super) fn is_paste_shortcut(key: &KeyEvent) -> bool {
    let is_v = matches!(key.code, KeyCode::Char('v') | KeyCode::Char('V'));
    let is_legacy_ctrl_v = matches!(key.code, KeyCode::Char('\u{16}'));
    if !is_v && !is_legacy_ctrl_v {
        return false;
    }

    if is_legacy_ctrl_v {
        return true;
    }

    // Cmd+V on macOS
    if key.modifiers.contains(KeyModifiers::SUPER) {
        return true;
    }

    // Ctrl+V on Linux/Windows
    key.modifiers.contains(KeyModifiers::CONTROL)
}

/// Whether the key event represents a user typing a printable
/// character into the composer (no modifier that would turn it into
/// a shortcut).
pub(super) fn is_text_input_key(key: &KeyEvent) -> bool {
    if matches!(key.code, KeyCode::Char(c) if c.is_control()) {
        return false;
    }

    !key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT)
        && !key.modifiers.contains(KeyModifiers::SUPER)
}

/// `Ctrl+H` is the legacy ASCII backspace many terminals still emit
/// when the user presses Backspace. Disallows Alt/Super so it doesn't
/// shadow window-management combos.
pub(super) fn is_ctrl_h_backspace(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('h'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT)
        && !key.modifiers.contains(KeyModifiers::SUPER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enhanced_keyboard_clipboard_events_are_accepted_cross_platform() {
        let mac_copy = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::SUPER);
        let mac_paste = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::SUPER);
        let linux_copy = KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        let linux_paste = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL);

        assert!(is_copy_shortcut(&mac_copy));
        assert!(is_paste_shortcut(&mac_paste));
        assert!(is_copy_shortcut(&linux_copy));
        assert!(is_paste_shortcut(&linux_paste));
    }

    #[test]
    fn ctrl_o_and_ctrl_shift_o_have_stable_distinct_routes() {
        let reasoning = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);
        // Crossterm's native Windows decoder applies Caps Lock to the
        // character but does not expose Caps Lock as a modifier.
        let reasoning_caps_lock = KeyEvent::new(KeyCode::Char('O'), KeyModifiers::CONTROL);
        let editor_lower = KeyEvent::new(
            KeyCode::Char('o'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        let editor_upper = KeyEvent::new(
            KeyCode::Char('O'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );

        for reasoning in [&reasoning, &reasoning_caps_lock] {
            assert!(is_reasoning_detail_shortcut(reasoning));
            assert!(!is_turn_inspector_shortcut(reasoning));
            assert!(!is_external_editor_shortcut(reasoning));
        }
        for editor in [&editor_lower, &editor_upper] {
            assert!(!is_reasoning_detail_shortcut(editor));
            assert!(!is_turn_inspector_shortcut(editor));
            assert!(is_external_editor_shortcut(editor));
        }

        let editor_legacy_fallback = KeyEvent::new(KeyCode::F(4), KeyModifiers::NONE);
        assert!(is_external_editor_shortcut(&editor_legacy_fallback));
    }

    #[test]
    fn turn_inspector_uses_collision_free_ctrl_alt_o() {
        let turn_inspector = KeyEvent::new(
            KeyCode::Char('o'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        );
        let turn_inspector_caps = KeyEvent::new(
            KeyCode::Char('O'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        );
        for key in [&turn_inspector, &turn_inspector_caps] {
            assert!(is_turn_inspector_shortcut(key));
            assert!(!is_reasoning_detail_shortcut(key));
            assert!(!is_external_editor_shortcut(key));
        }

        // Must not fire for bare Alt+O (would shadow typing) or Ctrl+Shift+O.
        let alt_o = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::ALT);
        let ctrl_shift_o = KeyEvent::new(
            KeyCode::Char('o'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert!(!is_turn_inspector_shortcut(&alt_o));
        assert!(!is_turn_inspector_shortcut(&ctrl_shift_o));
    }

    #[test]
    fn select_all_accepts_ctrl_shift_a_and_cmd_a_but_not_readline_ctrl_a() {
        let ctrl_shift_lower = KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        let ctrl_shift_upper = KeyEvent::new(
            KeyCode::Char('A'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        let cmd_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::SUPER);
        assert!(is_select_all_shortcut(&ctrl_shift_lower));
        assert!(is_select_all_shortcut(&ctrl_shift_upper));
        assert!(is_select_all_shortcut(&cmd_a));

        // Readline home stays readline home.
        let readline_ctrl_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert!(!is_select_all_shortcut(&readline_ctrl_a));
        // Alt combinations and plain typing never select-all.
        let alt_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::ALT);
        let plain_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert!(!is_select_all_shortcut(&alt_a));
        assert!(!is_select_all_shortcut(&plain_a));
    }

    #[test]
    fn tool_details_hint_uses_the_routed_chord_not_plain_typing() {
        let label = tool_details_shortcut_label();

        assert_eq!(label, crate::tui::shell_key_routing::tool_details_chord());
        assert_ne!(label, "v");
        assert_eq!(
            tool_details_shortcut_action_hint("full output"),
            format!("{label} opens full output")
        );
    }

    /// #3256: every surface that advertises tool details must name the chord
    /// that `is_tool_details_shortcut` actually handles — the help catalog,
    /// shell binding catalog, and in-transcript hint share one source of
    /// truth so "v opens details" cannot regress while bare `v` types `v`.
    #[test]
    fn tool_details_hint_tracks_keybinding_catalog_and_handler() {
        use crate::localization::MessageId;
        use crate::tui::keybindings::KEYBINDINGS;
        use crate::tui::shell_key_routing::{
            ShellBindingId, binding, is_tool_details_shortcut, tool_details_chord,
        };
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let catalog_chords: Vec<&str> = KEYBINDINGS
            .iter()
            .filter(|entry| entry.description_id == MessageId::KbSelectedDetails)
            .map(|entry| entry.chord)
            .collect();
        assert_eq!(catalog_chords, vec!["Alt+V"]);
        assert_eq!(binding(ShellBindingId::ToolDetails).catalog_chord, "Alt+V");
        assert_eq!(binding(ShellBindingId::ToolDetails).footer_chord, "Alt+V");

        let label = tool_details_shortcut_label();
        assert_eq!(label, tool_details_chord());
        assert!(
            label == "Alt+V" || label == "⌥V",
            "details hint must advertise Alt+V / ⌥V, got {label}"
        );
        assert!(!label.eq_ignore_ascii_case("v"));
        assert!(!tool_details_shortcut_action_hint("details").starts_with("v "));

        let plain_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE);
        let alt_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT);
        assert!(!is_tool_details_shortcut(&plain_v));
        assert!(is_tool_details_shortcut(&alt_v));
    }
}
