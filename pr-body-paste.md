## Problem

Pasting large text into the CodeWhale TUI composer (e.g. pasting a 20KB log file or code block) immediately converts it to an `@.deepseek/pastes/paste-xxx.md` mention. The user's text vanishes from the composer and is replaced by a cryptic `@file` reference they did not ask for. This is confusing — the user expected to see their pasted text and be able to review/edit it before sending.

## Root Cause

`insert_paste_text()` calls `consolidate_large_input_if_oversized()` at paste time, which checks if `char_count(input) > MAX_SUBMITTED_INPUT_CHARS` (16,000 chars). When exceeded, it immediately writes the text to a paste file and replaces the composer input with `@.deepseek/pastes/paste-xxx.md`.

The consolidation is useful at submit time (safety net), but at paste time it is surprising and confusing.

## Fix

1. **Remove the immediate consolidation from `insert_paste_text()`** — the text now stays in the composer as-is after pasting.
2. **Keep the consolidation as a safety net at submit time** — the same logic already runs when the user presses Enter, so the model still gets the `@file` reference if needed.
3. **Improve the toast message** — explain what happened when the submit-time consolidation fires.

The user now sees their full pasted text in the composer and can review/edit before sending. The consolidation only triggers when they press Enter if the text is still over the 16K char limit.

## Files

- `crates/tui/src/tui/app.rs`: commented out `self.consolidate_large_input_if_oversized()` in `insert_paste_text`, updated toast message
