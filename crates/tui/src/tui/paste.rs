//! Paste-burst handling — turn rapid keystrokes (terminals without bracketed
//! paste) into a single committed buffer instead of N individual chars.
//!
//! Extracted from `tui/ui.rs` (P1.2). The owning state machine lives on
//! `App.paste_burst` (`tui::paste_burst`); these helpers wire it to the key
//! event loop and the composer's text buffer.

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{App, looks_like_slash_command_input};
use super::paste_burst::CharDecision;

/// Process a key in the context of paste-burst detection. Returns `true`
/// when the key was fully handled by the paste machinery (caller skips
/// further input handling); `false` when the key still needs the normal
/// composer path.
pub fn handle_paste_burst_key(app: &mut App, key: &KeyEvent, now: Instant) -> bool {
    if !app.use_paste_burst_detection {
        return false;
    }
    // Once we've observed a real `Event::Paste` in this session, bracketed
    // paste is verified working and the rapid-keystroke heuristic is
    // unnecessary. Skipping it eliminates false positives on fast typing /
    // IME commits / autocomplete on terminals with reliable bracketed
    // paste (the dominant case on iTerm2 / Ghostty / WezTerm / Windows
    // Terminal).
    if app.bracketed_paste_seen {
        return false;
    }

    let has_ctrl_alt_or_super = key.modifiers.contains(KeyModifiers::CONTROL)
        || key.modifiers.contains(KeyModifiers::ALT)
        || key.modifiers.contains(KeyModifiers::SUPER);

    match key.code {
        KeyCode::Enter => {
            if !in_command_context(app) && app.paste_burst.append_newline_if_active(now) {
                return true;
            }
            if !in_command_context(app)
                && app.paste_burst.newline_should_insert_instead_of_submit(now)
            {
                app.insert_char('\n');
                app.paste_burst.extend_window(now);
                return true;
            }
        }
        KeyCode::Char(c) if !has_ctrl_alt_or_super => {
            if !c.is_ascii() {
                if let Some(pending) = app.paste_burst.flush_before_modified_input() {
                    app.insert_str(&pending);
                }
                if app.paste_burst.try_append_char_if_active(c, now) {
                    return true;
                }
                if let Some(decision) = app.paste_burst.on_plain_char_no_hold(now) {
                    return handle_paste_burst_decision(app, decision, c, now);
                }
                app.insert_char(c);
                return true;
            }

            let decision = app.paste_burst.on_plain_char(c, now);
            return handle_paste_burst_decision(app, decision, c, now);
        }
        _ => {}
    }

    false
}

/// Apply a paste-burst decision to the composer buffer. Some decisions
/// retroactively grab the last few chars from the input back into the
/// pending paste buffer (when the heuristic decides the recent typing was
/// actually a paste).
pub fn handle_paste_burst_decision(
    app: &mut App,
    decision: CharDecision,
    c: char,
    now: Instant,
) -> bool {
    match decision {
        CharDecision::RetainFirstChar => true,
        CharDecision::BeginBufferFromPending | CharDecision::BufferAppend => {
            app.paste_burst.append_char_to_buffer(c, now);
            true
        }
        CharDecision::BeginBuffer { retro_chars } => {
            if apply_paste_burst_retro_capture(app, retro_chars as usize, c, now) {
                return true;
            }
            app.insert_char(c);
            true
        }
    }
}

fn apply_paste_burst_retro_capture(
    app: &mut App,
    retro_chars: usize,
    c: char,
    now: Instant,
) -> bool {
    let cursor_byte = app.cursor_byte_index();
    let before = &app.composer.input[..cursor_byte];
    let Some(grab) = app
        .composer
        .paste_burst
        .decide_begin_buffer(now, before, retro_chars)
    else {
        return false;
    };
    if !grab.grabbed.is_empty() {
        app.input.replace_range(grab.start_byte..cursor_byte, "");
        let removed = grab.grabbed.chars().count();
        app.cursor_position = app.cursor_position.saturating_sub(removed);
    }
    app.paste_burst.append_char_to_buffer(c, now);
    true
}

fn in_command_context(app: &App) -> bool {
    looks_like_slash_command_input(&app.input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::TuiOptions;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn test_app() -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: PathBuf::from("."),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: PathBuf::from("."),
            memory_path: PathBuf::from("memory.md"),
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        let mut app = App::new(options, &Config::default());
        app.use_paste_burst_detection = true;
        app
    }

    fn plain(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)
    }

    #[test]
    fn raw_short_cjk_multiline_paste_buffers_enter_instead_of_submitting() {
        // #1302: pasting short CJK content like "请联网搜索：\nSTM32 …" used
        // to silently submit the first line because the heuristic decided
        // it wasn't paste-like (no whitespace + under 16 chars). The
        // non-ASCII bypass now classifies it as a paste so the Enter is
        // absorbed into the burst buffer.
        let mut app = test_app();
        let t0 = Instant::now();

        let pasted = "请联网搜索：\nSTM32 商业应用案例";
        for (i, ch) in pasted.chars().enumerate() {
            let key = if ch == '\n' {
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
            } else {
                plain(ch)
            };
            let handled =
                handle_paste_burst_key(&mut app, &key, t0 + Duration::from_millis(i as u64));
            assert!(
                handled,
                "raw paste character {ch:?} must be handled by paste-burst detection"
            );
        }

        assert!(app.flush_paste_burst_if_due(
            t0 + Duration::from_millis(pasted.chars().count() as u64)
                + crate::tui::paste_burst::PasteBurst::recommended_active_flush_delay()
        ));
        assert_eq!(app.input, pasted);
    }

    #[test]
    fn raw_multiline_paste_buffers_enter_instead_of_submitting() {
        let mut app = test_app();
        let t0 = Instant::now();

        assert!(handle_paste_burst_key(&mut app, &plain('a'), t0));
        assert!(handle_paste_burst_key(
            &mut app,
            &plain('b'),
            t0 + Duration::from_millis(1)
        ));
        assert!(handle_paste_burst_key(
            &mut app,
            &plain('c'),
            t0 + Duration::from_millis(2)
        ));
        assert!(handle_paste_burst_key(
            &mut app,
            &KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            t0 + Duration::from_millis(3)
        ));

        assert!(app.input.is_empty(), "paste remains buffered until idle");
        assert!(app.flush_paste_burst_if_due(
            t0 + Duration::from_millis(3)
                + crate::tui::paste_burst::PasteBurst::recommended_active_flush_delay()
        ));
        assert_eq!(app.input, "abc\n");
    }

    #[test]
    fn paste_buffered_question_mark_does_not_fall_through_to_help_shortcut() {
        let mut app = test_app();
        let t0 = Instant::now();

        assert!(handle_paste_burst_key(&mut app, &plain('?'), t0));

        assert!(app.input.is_empty(), "shortcut char stays buffered first");
        assert!(app.view_stack.is_empty(), "help modal must not open");
        assert!(app.flush_paste_burst_if_due(
            t0 + crate::tui::paste_burst::PasteBurst::recommended_flush_delay()
        ));
        assert_eq!(app.input, "?");
    }

    /// Pin the IME-input contract: macOS/Windows input methods commit
    /// each Chinese character as a single `KeyCode::Char(c)` event
    /// after the candidate popup closes. Each codepoint fits in a
    /// `char` (no surrogate pair concerns for BMP chars), so a
    /// straightforward sequence of plain-char events must land in
    /// `app.input` verbatim — no ASCII filter, no byte-vs-char index
    /// drift, no paste-burst false-positive that buffers the chars
    /// indefinitely.
    #[test]
    fn ime_chinese_chars_route_through_to_composer() {
        let mut app = test_app();
        let t0 = Instant::now();

        // Type the four Chinese codepoints "你好世界" one event at a
        // time, with realistic ~50ms gaps so the paste-burst heuristic
        // doesn't classify them as a paste burst.
        for (i, ch) in "你好世界".chars().enumerate() {
            let now = t0 + Duration::from_millis(50 * i as u64);
            let _ = handle_paste_burst_key(&mut app, &plain(ch), now);
        }

        // Past the active-flush delay so any buffered burst commits.
        let after = t0
            + Duration::from_millis(50 * 4)
            + crate::tui::paste_burst::PasteBurst::recommended_active_flush_delay();
        let _ = app.flush_paste_burst_if_due(after);

        assert_eq!(
            app.input, "你好世界",
            "IME-typed Chinese characters must land in composer verbatim"
        );
        assert_eq!(
            app.cursor_position, 4,
            "cursor advances by one per codepoint, not per UTF-8 byte"
        );
    }

    /// Pin the bracketed-paste contract for CJK content: pasted
    /// Chinese text (e.g. when a user copies a question from a
    /// Chinese website and pastes into the composer) must preserve
    /// every codepoint and not double-count multi-byte chars in the
    /// cursor position.
    #[test]
    fn bracketed_paste_preserves_chinese_and_mixed_text() {
        let mut app = test_app();
        app.insert_paste_text("你好世界 hello 世界 café");
        assert_eq!(app.input, "你好世界 hello 世界 café");
        // 4 + 1 + 5 + 1 + 2 + 1 + 4 = 18 codepoints (counting é as one).
        assert_eq!(app.cursor_position, 18);
    }

    #[test]
    fn paste_burst_detection_can_be_disabled_without_disabling_bracketed_paste() {
        let mut app = test_app();
        app.use_paste_burst_detection = false;

        assert!(!handle_paste_burst_key(
            &mut app,
            &plain('a'),
            Instant::now()
        ));
        assert!(app.input.is_empty());

        app.insert_paste_text("line 1\r\nline 2");
        assert_eq!(app.input, "line 1\nline 2");
        assert!(app.use_bracketed_paste);
    }

    /// Once the session has observed a real `Event::Paste`, the
    /// rapid-keystroke heuristic must short-circuit. This pins the new
    /// "auto-disable paste-burst on verified bracketed paste" behavior so
    /// fast typing / IME commits / autocomplete on capable terminals can't
    /// be mis-classified as a paste burst.
    #[test]
    fn paste_burst_short_circuits_after_bracketed_paste_observed() {
        let mut app = test_app();
        app.use_paste_burst_detection = true;
        app.bracketed_paste_seen = true;

        let t0 = Instant::now();
        for (i, ch) in "abcdefgh".chars().enumerate() {
            // Type fast enough that paste-burst would normally fire.
            let now = t0 + Duration::from_millis(i as u64);
            assert!(
                !handle_paste_burst_key(&mut app, &plain(ch), now),
                "paste-burst must NOT consume keys once bracketed paste verified"
            );
        }
        // No buffering — every char fell through to the normal composer
        // path (the test harness doesn't insert chars when the burst
        // handler returns false; we only assert the short-circuit
        // contract here).
        assert!(app.input.is_empty());
    }
}
