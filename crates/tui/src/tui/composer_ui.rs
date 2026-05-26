use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::app::App;

const COMPOSER_ARROW_SCROLL_LINES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EscapeAction {
    CloseSlashMenu,
    CancelRequest,
    DiscardQueuedDraft,
    ClearInput,
    Noop,
}

pub(crate) fn next_escape_action(app: &App, slash_menu_open: bool) -> EscapeAction {
    if slash_menu_open {
        EscapeAction::CloseSlashMenu
    } else if app.is_loading {
        EscapeAction::CancelRequest
    } else if app.queued_draft.is_some() && app.input.is_empty() {
        EscapeAction::DiscardQueuedDraft
    } else if !app.input.is_empty() {
        EscapeAction::ClearInput
    } else {
        EscapeAction::Noop
    }
}

pub(crate) fn select_previous_slash_menu_entry(app: &mut App, entry_count: usize) {
    if entry_count == 0 {
        return;
    }
    let selected = app.slash_menu_selected.min(entry_count.saturating_sub(1));
    app.slash_menu_selected = (selected + entry_count - 1) % entry_count;
}

pub(crate) fn select_next_slash_menu_entry(app: &mut App, entry_count: usize) {
    if entry_count == 0 {
        return;
    }
    let selected = app.slash_menu_selected.min(entry_count.saturating_sub(1));
    app.slash_menu_selected = (selected + 1) % entry_count;
}

pub(crate) fn handle_composer_history_arrow(
    app: &mut App,
    key: KeyEvent,
    slash_menu_open: bool,
    mention_menu_open: bool,
) -> bool {
    if slash_menu_open || mention_menu_open {
        return false;
    }
    if key.modifiers.contains(KeyModifiers::ALT) || key.modifiers.contains(KeyModifiers::SUPER) {
        return false;
    }

    // When `composer_arrows_scroll` is enabled, plain Up/Down scroll the
    // transcript for single-line drafts. Multiline composers keep editor-like
    // line navigation, with history fallback at the first/last line.
    let scroll_transcript = app.composer_arrows_scroll && !app.input.contains('\n');

    match key.code {
        KeyCode::Up => {
            if scroll_transcript {
                app.scroll_up(COMPOSER_ARROW_SCROLL_LINES);
            } else {
                app.vim_move_up();
            }
            true
        }
        KeyCode::Down => {
            if scroll_transcript {
                app.scroll_down(COMPOSER_ARROW_SCROLL_LINES);
            } else {
                app.vim_move_down();
            }
            true
        }
        _ => false,
    }
}

pub(crate) fn is_word_cursor_modifier(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL) || modifiers.contains(KeyModifiers::ALT)
}

pub(crate) fn is_composer_newline_key(key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('j') => key.modifiers.contains(KeyModifiers::CONTROL),
        KeyCode::Enter => {
            key.modifiers.contains(KeyModifiers::ALT)
                || (key.modifiers.contains(KeyModifiers::SHIFT)
                    && !key.modifiers.contains(KeyModifiers::CONTROL))
        }
        _ => false,
    }
}

pub(crate) fn handle_history_search_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => {
            let _ = app.accept_history_search();
        }
        KeyCode::Esc => {
            app.cancel_history_search();
        }
        KeyCode::Char('c') | KeyCode::Char('C')
            if key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            app.cancel_history_search();
        }
        KeyCode::Backspace => {
            app.history_search_backspace();
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            while app
                .history_search_query()
                .is_some_and(|query| !query.is_empty())
            {
                app.history_search_backspace();
            }
        }
        KeyCode::Up => {
            app.history_search_select_previous();
        }
        KeyCode::Down => {
            app.history_search_select_next();
        }
        KeyCode::Char(ch)
            if key.modifiers.is_empty()
                || key.modifiers == KeyModifiers::SHIFT
                || key.modifiers == KeyModifiers::NONE =>
        {
            app.history_search_insert_char(ch);
        }
        _ => {}
    }
}
