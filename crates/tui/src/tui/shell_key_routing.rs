//! Shell keyboard bindings for details / context / help.
//!
//! Footer hints, help catalog chords, and live handlers must agree on one
//! source. Printable characters always belong to the composer: bare `v`
//! types `v` in every focus state — work surface, transcript selection,
//! panel, or modal (TUI-DOG-002). Details/output fires only on
//! Option+V / Alt+V, and macOS renders the label as `⌥V`, never `Alt`/`Cmd`.
//! Help is `F1` (with `/help`); `Ctrl+/` stays as a secondary fallback.
//! Provider/route is `F3` (with `/provider`); it is non-printable so it can
//! remain available while the composer owns ordinary text input.
//! `Alt+?` and `Alt+C` are still accepted where terminals deliver them but
//! are never advertised until proven in real terminals (TUI-DOG-003);
//! `/context` is the guaranteed context path.
//! Ambiguous macOS Option glyphs (`ç` / `¿`) remain text: terminals do not
//! identify whether they came from Option or from a user's keyboard layout.

use std::borrow::Cow;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::key_shortcuts;
use crate::tui::views::ModalKind;

/// Who owns the keyboard right now.
///
/// One value, derived in one place ([`crate::tui::app::App::focus`]), in
/// place of the `app.input.is_empty()` guesses that used composer *content*
/// as a stand-in for composer *focus*. Composer editing keys still ask about
/// the text itself; every shell binding asks this instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// The onboarding rail owns every key until it finishes.
    Onboarding,
    /// A modal view is on top of the stack and handles its own keys.
    Modal(ModalKind),
    /// The pre-session launch screen — its menu or its composer.
    Launch,
    /// A focused rail or workflow panel inside a live session.
    Panel,
    /// The session composer: the default owner.
    Composer,
}

/// Which focus states a binding is live in — the `ShellBinding` focus rule
/// that used to be re-invented at every call site as
/// `&& app.view_stack.is_empty()`. The variants nest: each admits everything
/// the one above it does, plus one more surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusScope {
    /// A live session: the composer, or a rail/workflow panel that has taken
    /// the keys from it.
    SessionShell,
    /// [`FocusScope::SessionShell`], plus the pre-session launch stage.
    AnyShell,
    /// [`FocusScope::AnyShell`], plus the Config modal, which displays the
    /// very setting the binding changes.
    AnyShellOrConfig,
    /// Every focus state, onboarding and modals included.
    Everywhere,
}

impl FocusScope {
    #[must_use]
    pub fn admits(self, focus: Focus) -> bool {
        match self {
            Self::SessionShell => matches!(focus, Focus::Composer | Focus::Panel),
            Self::AnyShell => matches!(focus, Focus::Composer | Focus::Panel | Focus::Launch),
            Self::AnyShellOrConfig => matches!(
                focus,
                Focus::Composer | Focus::Panel | Focus::Launch | Focus::Modal(ModalKind::Config)
            ),
            Self::Everywhere => true,
        }
    }
}

/// Stable binding ids shared by handlers, footer hints, and help catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellBindingId {
    ToolDetails,
    ContextInspector,
    ProviderRoute,
    Help,
    Settings,
    /// Tab: cycle the session mode.
    ModeCycle,
    /// Shift+Tab: cycle the permission posture.
    PermissionCycle,
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
    /// The focus states this binding is live in. Never composer content:
    /// a shell binding does the same thing whether or not you have typed.
    pub focus: FocusScope,
}

impl ShellBinding {
    /// Does this key press this binding, ignoring focus?
    #[must_use]
    pub fn matches(&self, key: &KeyEvent) -> bool {
        match self.id {
            ShellBindingId::ToolDetails => is_tool_details_shortcut(key),
            ShellBindingId::ContextInspector => is_context_inspector_shortcut(key),
            ShellBindingId::ProviderRoute => is_provider_route_shortcut(key),
            ShellBindingId::Help => is_help_shortcut(key),
            ShellBindingId::Settings => is_settings_shortcut(key),
            ShellBindingId::ModeCycle => is_mode_cycle_shortcut(key),
            ShellBindingId::PermissionCycle => is_permission_cycle_shortcut(key),
        }
    }
}

/// The shell's single key-admission authority: which binding, if any, this
/// key presses for this focus owner.
///
/// Callers keep their position in the event loop — that ordering is a real
/// statement about which surface sees a key first — but none of them decides
/// admission any more, and none of them may ask about composer content.
#[must_use]
pub fn route(focus: Focus, key: &KeyEvent) -> Option<ShellBindingId> {
    SHELL_BINDINGS
        .iter()
        .find(|binding| binding.focus.admits(focus) && binding.matches(key))
        .map(|binding| binding.id)
}

/// Canonical shell bindings. Handlers and chrome read from here.
pub const SHELL_BINDINGS: &[ShellBinding] = &[
    ShellBinding {
        id: ShellBindingId::ToolDetails,
        catalog_chord: "Alt+V",
        footer_chord: "Alt+V",
        // The rail claims the details chord for a selected row before the
        // transcript pager sees it; both are session surfaces.
        focus: FocusScope::SessionShell,
    },
    ShellBinding {
        id: ShellBindingId::ContextInspector,
        // `/context` is the guaranteed path; Alt+C stays an unadvertised
        // handler until proven in Cursor/Terminal.app/iTerm2/tmux/PTY.
        catalog_chord: "/context",
        footer_chord: "/context",
        focus: FocusScope::SessionShell,
    },
    ShellBinding {
        id: ShellBindingId::ProviderRoute,
        // `/provider` remains the portable, explicit command path.
        catalog_chord: "F3 / /provider",
        footer_chord: "F3",
        // The route is also pickable before a session exists.
        focus: FocusScope::AnyShell,
    },
    ShellBinding {
        id: ShellBindingId::Help,
        // `/help` also opens this; Ctrl+/ is the secondary fallback.
        catalog_chord: "F1 / Ctrl+/",
        footer_chord: "F1",
        focus: FocusScope::Everywhere,
    },
    ShellBinding {
        id: ShellBindingId::Settings,
        catalog_chord: "F2",
        footer_chord: "F2",
        // Shell-global for the same reason as Help: a settings route that
        // disappears inside onboarding or a modal is not a route.
        focus: FocusScope::Everywhere,
    },
    ShellBinding {
        id: ShellBindingId::ModeCycle,
        catalog_chord: "Tab",
        footer_chord: "Tab",
        // Tab is the session's mode cycle. The composer's own completions
        // get the key first, but *having typed* never disables it. The
        // launch screen is excluded: there Tab moves focus between the
        // startup menu and the pre-session composer.
        focus: FocusScope::SessionShell,
    },
    ShellBinding {
        id: ShellBindingId::PermissionCycle,
        catalog_chord: "Shift+Tab",
        footer_chord: "Shift+Tab",
        // A shell-level permission control, live wherever the shell is —
        // including the launch screen, where it used to be dead — plus the
        // Config modal that displays the posture it changes.
        focus: FocusScope::AnyShellOrConfig,
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

/// Route entry stays on a non-printable function key so it never steals a
/// model/provider name from the composer. `/provider` remains available in
/// terminals that do not forward function keys.
#[must_use]
pub fn is_provider_route_shortcut(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::F(3)) && key.modifiers.is_empty()
}

#[must_use]
pub fn is_help_shortcut(key: &KeyEvent) -> bool {
    if matches!(key.code, KeyCode::F(1)) {
        return true;
    }
    // Windows delivers AltGr as Ctrl+Alt, so a layout-emitted glyph (e.g.
    // AltGr+Q typing '/' on ABNT2) would satisfy a bare CONTROL check.
    // AltGr chords are text, never shortcuts (#4723).
    let altgr = crate::tui::widgets::key_hint::is_altgr(key.modifiers);
    if matches!(key.code, KeyCode::Char('/'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && !altgr
    {
        return true;
    }
    // Some legacy terminal stacks encode Ctrl+/ as the ASCII unit separator,
    // which crossterm reports as Ctrl+7 or Ctrl+_. Accept both portable
    // decodings so the documented fallback remains real.
    if matches!(key.code, KeyCode::Char('7') | KeyCode::Char('_'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && !altgr
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

/// Tab cycles the session mode. Terminal chords that mean something else to
/// the host (Ctrl/Alt/Cmd+Tab) are not ours, and Shift+Tab is the permission
/// cycle below.
#[must_use]
pub fn is_mode_cycle_shortcut(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Tab)
        && !key.modifiers.intersects(
            KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER | KeyModifiers::SHIFT,
        )
}

/// Shift+Tab cycles the permission posture. Terminals encode the same chord
/// either as `BackTab` or as `Tab` + SHIFT; accept both.
#[must_use]
pub fn is_permission_cycle_shortcut(key: &KeyEvent) -> bool {
    let forbidden = KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER;
    if key.modifiers.intersects(forbidden) {
        return false;
    }
    matches!(key.code, KeyCode::BackTab)
        || (matches!(key.code, KeyCode::Tab) && key.modifiers.contains(KeyModifiers::SHIFT))
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

    #[test]
    fn altgr_slash_types_text_instead_of_opening_help() {
        // Windows encodes AltGr as Ctrl+Alt: AltGr+Q on ABNT2 delivers '/'
        // with CONTROL|ALT and must reach the composer as text (#4723).
        let altgr_slash = KeyEvent::new(
            KeyCode::Char('/'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        );
        let altgr_seven = KeyEvent::new(
            KeyCode::Char('7'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        );
        if cfg!(windows) {
            assert!(!is_help_shortcut(&altgr_slash));
            assert!(!is_help_shortcut(&altgr_seven));
        } else {
            // Elsewhere Ctrl+Alt is a deliberate chord and keeps working.
            assert!(is_help_shortcut(&altgr_slash));
            assert!(is_help_shortcut(&altgr_seven));
        }
        // Plain Ctrl+/ still opens help everywhere.
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
    fn topbar_route_f3_requires_a_plain_function_key() {
        assert!(is_provider_route_shortcut(&KeyEvent::new(
            KeyCode::F(3),
            KeyModifiers::NONE
        )));
        assert!(!is_provider_route_shortcut(&KeyEvent::new(
            KeyCode::F(3),
            KeyModifiers::ALT
        )));
        assert!(!is_provider_route_shortcut(&KeyEvent::new(
            KeyCode::Char('3'),
            KeyModifiers::NONE
        )));
    }

    #[test]
    fn footer_only_advertises_chords_that_are_live_where_it_is_shown() {
        // The footer hint row is built from this table, so it must not be
        // able to name a chord the same table refuses at the focus the
        // footer is rendered in.
        let hints = footer_action_hints_for_platform(true, false);
        for id in [
            ShellBindingId::ToolDetails,
            ShellBindingId::ContextInspector,
            ShellBindingId::Help,
        ] {
            let binding = binding(id);
            assert!(hints.contains(binding.footer_chord), "{hints}");
            assert!(
                binding.focus.admits(Focus::Composer),
                "footer advertises {id:?}, which the table does not admit at the composer"
            );
        }
    }

    #[test]
    fn focus_scopes_nest_from_the_session_outwards() {
        let ladder = [
            FocusScope::SessionShell,
            FocusScope::AnyShell,
            FocusScope::AnyShellOrConfig,
            FocusScope::Everywhere,
        ];
        let states = [
            Focus::Composer,
            Focus::Panel,
            Focus::Launch,
            Focus::Modal(ModalKind::Config),
            Focus::Modal(ModalKind::Approval),
            Focus::Onboarding,
        ];
        for pair in ladder.windows(2) {
            for focus in states {
                assert!(
                    !pair[0].admits(focus) || pair[1].admits(focus),
                    "{:?} admits {focus:?} but the wider {:?} does not",
                    pair[0],
                    pair[1]
                );
            }
        }
        // Nothing but Help/Settings may reach a focused workflow.
        for binding in SHELL_BINDINGS {
            assert_eq!(
                binding.focus.admits(Focus::Modal(ModalKind::Approval)),
                binding.focus == FocusScope::Everywhere,
                "{:?} must not reach across an approval decision",
                binding.id
            );
        }
    }

    #[test]
    fn tab_and_shift_tab_are_distinct_bindings() {
        let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        let shift_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT);
        assert_eq!(
            route(Focus::Composer, &tab),
            Some(ShellBindingId::ModeCycle)
        );
        assert_eq!(
            route(Focus::Composer, &shift_tab),
            Some(ShellBindingId::PermissionCycle)
        );
        // Tab moves focus on the launch stage; the posture control still works.
        assert_eq!(route(Focus::Launch, &tab), None);
        assert_eq!(
            route(Focus::Launch, &shift_tab),
            Some(ShellBindingId::PermissionCycle)
        );
    }

    #[test]
    fn catalog_chords_match_final_contract() {
        assert_eq!(binding(ShellBindingId::Help).catalog_chord, "F1 / Ctrl+/");
        assert_eq!(
            binding(ShellBindingId::ContextInspector).catalog_chord,
            "/context"
        );
        assert_eq!(binding(ShellBindingId::ToolDetails).catalog_chord, "Alt+V");
        assert_eq!(
            binding(ShellBindingId::ProviderRoute).catalog_chord,
            "F3 / /provider"
        );
        for binding in SHELL_BINDINGS {
            assert!(!binding.catalog_chord.contains("Alt+?"));
            assert_ne!(binding.catalog_chord, "v");
            assert!(!binding.catalog_chord.starts_with("v /"));
            assert!(!binding.footer_chord.contains("Alt+?"));
            assert_ne!(binding.footer_chord, "v");
        }
    }
}
