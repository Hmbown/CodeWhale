//! Streaming-thinking lifecycle for the active cell.
//!
//! DeepSeek V4 emits `reasoning_content` chunks before final answers.
//! These get rendered as a "Thinking" entry inside the per-turn active
//! cell. This module is the single source of truth for:
//!
//! - creating a streaming thinking entry on first chunk
//! - appending chunks to the live entry
//! - showing a localized placeholder while a translation is in-flight
//!   (and animating its elapsed/spinner suffix)
//! - replacing the placeholder when the translation arrives
//! - finalizing the entry (stopping the spinner, stamping duration)
//!   when a thinking block ends
//! - stashing the reasoning buffer onto `app.last_reasoning` so the
//!   summary survives compaction

use std::time::Instant;

use crate::tui::active_cell::ActiveCell;
use crate::tui::app::App;
use crate::tui::history::HistoryCell;

/// Ensure an in-flight Thinking entry exists in `active_cell` and return its
/// entry index. If no thinking entry is currently streaming, push a fresh one.
/// P2.3: thinking shares the active cell with subsequent tool calls so the
/// pair render as one logical "Working…" block.
pub(super) fn ensure_active_entry(app: &mut App) -> usize {
    if let Some(idx) = app.streaming_thinking_active_entry {
        return idx;
    }
    if app.active_cell.is_none() {
        app.active_cell = Some(ActiveCell::new());
    }
    let active = app.active_cell.as_mut().expect("active_cell just ensured");
    let entry_idx = active.push_thinking(HistoryCell::Thinking {
        content: String::new(),
        streaming: true,
        duration_secs: None,
    });
    app.streaming_thinking_active_entry = Some(entry_idx);
    app.bump_active_cell_revision();
    entry_idx
}

/// Append text to a streaming Thinking entry inside `active_cell`. Bumps the
/// active-cell revision so the renderer re-draws the live tail.
pub(super) fn append(app: &mut App, entry_idx: usize, text: &str) {
    if text.is_empty() {
        return;
    }
    let mutated = if let Some(active) = app.active_cell.as_mut()
        && let Some(HistoryCell::Thinking { content, .. }) = active.entry_mut(entry_idx)
    {
        content.push_str(text);
        true
    } else {
        false
    };
    if mutated {
        app.bump_active_cell_revision();
    }
}

/// Build the spinner-decorated placeholder shown in the thinking entry
/// while a translation is in flight (`Thinking… (1.2s |)`).
pub(super) fn translation_placeholder_frame(app: &App) -> String {
    let base = crate::localization::thinking_translation_placeholder(app.ui_locale);
    let elapsed = app
        .thinking_started_at
        .or(app.turn_started_at)
        .map(|started| started.elapsed().as_secs_f32())
        .unwrap_or_default();
    let frame = match (elapsed.mul_add(2.0, 0.0) as usize) % 4 {
        0 => "|",
        1 => "/",
        2 => "-",
        _ => "\\",
    };
    format!("{base} ({elapsed:.1}s {frame})")
}

/// If the given entry is empty or still showing the translation
/// placeholder prefix, replace it with the latest animated frame.
pub(super) fn set_placeholder(app: &mut App, entry_idx: usize) {
    let base = crate::localization::thinking_translation_placeholder(app.ui_locale);
    let next = translation_placeholder_frame(app);
    let mutated = if let Some(active) = app.active_cell.as_mut()
        && let Some(HistoryCell::Thinking { content, .. }) = active.entry_mut(entry_idx)
        && (content.is_empty() || content.starts_with(base))
    {
        if *content != next {
            *content = next;
            true
        } else {
            false
        }
    } else {
        false
    };
    if mutated {
        app.bump_active_cell_revision();
    }
}

/// Advance the spinner suffix on every existing translation placeholder
/// in `active_cell`. Returns true when at least one cell was updated so
/// the dispatch loop can schedule another tick.
pub(super) fn animate_pending_translation(app: &mut App, translation_pending: bool) -> bool {
    if !app.translation_enabled {
        return false;
    }
    let thinking_streaming = app.streaming_thinking_active_entry.is_some();
    if !translation_pending && !thinking_streaming {
        return false;
    }
    let base = crate::localization::thinking_translation_placeholder(app.ui_locale);
    let next = translation_placeholder_frame(app);

    if let Some(active) = app.active_cell.as_mut() {
        for idx in (0..active.entry_count()).rev() {
            if let Some(HistoryCell::Thinking { content, .. }) = active.entry_mut(idx)
                && content.starts_with(base)
                && *content != next
            {
                *content = next.clone();
                app.bump_active_cell_revision();
                return true;
            }
        }
    }
    false
}

/// Replace a translation placeholder with the finished translated text.
/// Searches the active cell first, then the finalized history (covers
/// the case where the translation lands after the thinking block was
/// already moved into history).
pub(super) fn replace_pending_translation(
    app: &mut App,
    placeholder: &str,
    translated_text: String,
) {
    if let Some(active) = app.active_cell.as_mut() {
        for idx in (0..active.entry_count()).rev() {
            if let Some(HistoryCell::Thinking { content, .. }) = active.entry_mut(idx)
                && content.starts_with(placeholder)
            {
                *content = translated_text;
                app.bump_active_cell_revision();
                return;
            }
        }
    }

    for idx in (0..app.history.len()).rev() {
        if let Some(HistoryCell::Thinking { content, .. }) = app.history.get_mut(idx)
            && content.starts_with(placeholder)
        {
            *content = translated_text;
            app.bump_history_cell(idx);
            return;
        }
    }
}

/// Start a new streaming thinking block. If another thinking block is still
/// active, first drain its pending UI tail so a late block boundary cannot
/// discard content buffered inside `StreamingState`.
pub(super) fn start_block(app: &mut App) -> bool {
    let finalized_previous = if app.streaming_thinking_active_entry.is_some() {
        let finalized = finalize_current(app);
        stash_reasoning_buffer_into_last_reasoning(app);
        finalized
    } else {
        false
    };

    app.reasoning_buffer.clear();
    app.reasoning_header = None;
    app.thinking_started_at = Some(Instant::now());
    app.streaming_state.reset();
    app.streaming_state.start_thinking(0, None);
    let _ = ensure_active_entry(app);
    finalized_previous
}

/// Finalize the currently-streaming thinking entry: drain the pending
/// state buffer, compute elapsed duration, stop the spinner.
pub(super) fn finalize_current(app: &mut App) -> bool {
    let duration = app
        .thinking_started_at
        .take()
        .map(|t| t.elapsed().as_secs_f32());
    let remaining = app.streaming_state.finalize_block_text(0);
    finalize_active_entry(app, duration, &remaining)
}

/// Move the in-flight reasoning buffer onto `app.last_reasoning` so the
/// summary survives compaction or transcript trimming.
pub(super) fn stash_reasoning_buffer_into_last_reasoning(app: &mut App) {
    if app.reasoning_buffer.is_empty() {
        return;
    }

    if let Some(existing) = app.last_reasoning.as_mut()
        && !existing.is_empty()
    {
        if !existing.ends_with('\n') {
            existing.push('\n');
        }
        existing.push_str(&app.reasoning_buffer);
    } else {
        app.last_reasoning = Some(app.reasoning_buffer.clone());
    }
    app.reasoning_buffer.clear();
}

/// Finalize the in-flight thinking entry in `active_cell`: append the
/// collector's remaining buffered text, stop the spinner, and stamp the
/// duration. Returns `true` when a thinking entry was finalized (so the
/// dispatch loop knows the transcript was touched). No-op if no thinking
/// entry is currently streaming.
pub(super) fn finalize_active_entry(app: &mut App, duration: Option<f32>, remaining: &str) -> bool {
    let Some(entry_idx) = app.streaming_thinking_active_entry.take() else {
        return false;
    };
    if !remaining.is_empty() {
        append(app, entry_idx, remaining);
    }
    if let Some(active) = app.active_cell.as_mut()
        && let Some(HistoryCell::Thinking {
            streaming,
            duration_secs,
            ..
        }) = active.entry_mut(entry_idx)
    {
        *streaming = false;
        *duration_secs = duration;
    }
    app.bump_active_cell_revision();
    true
}
