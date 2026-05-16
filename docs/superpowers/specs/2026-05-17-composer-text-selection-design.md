# Composer Text Selection

Add text selection support to the input box (composer) — keyboard and mouse.

## Problem

The composer (`ComposerState`) has no selection mechanism. Users cannot select, copy, or delete a range of text in the input box. Every edit is single-character or single-word, and mouse clicks in the composer area are ignored.

## Design

### Data Model

Add one field to `ComposerState`:

```rust
pub selection_anchor: Option<usize>,  // char-indexed, None = no selection
```

Semantics: `anchor` is the fixed end, `cursor_position` is the active end. The effective selection range is `min(anchor, cursor) .. max(anchor, cursor)`. When `anchor == cursor`, treat as no selection (set to `None`).

Editing method rules:
- **Text-modifying operations** (insert, delete): if selection exists, delete selected content first, then perform operation, then clear anchor.
- **Cursor movement with Shift held**: set/keep anchor, move cursor to extend selection.
- **Cursor movement without Shift**: clear anchor.
- **Unrelated operations** (history, slash menu): clear anchor.

### Keyboard Interactions

In `ui.rs` key dispatch block:

| Key | Behavior |
|---|---|
| `Shift+Left/Right` | Set anchor (if none), move cursor to extend selection |
| `Shift+Ctrl+Left/Right` | Set anchor (if none), move cursor by word |
| `Shift+Home/End` | Set anchor (if none), cursor to line start/end |
| `Ctrl+A` / `Cmd+A` | Select all (anchor=0, cursor=end) |
| `Backspace` / `Delete` | With selection: delete selected text; without: original behavior |
| Printable char input | With selection: replace selected text, clear anchor |
| `Ctrl+C` / `Cmd+C` | With selection: copy selected text to clipboard |
| `Ctrl+X` / `Cmd+X` | With selection: cut (copy + delete) |
| Non-Shift navigation keys | Clear anchor |

Implementation: add `key.modifiers.contains(SHIFT)` branches to existing `KeyCode::Left/Right/Home/End` handlers (~line 3238 in ui.rs).

### Mouse Interactions

In `mouse_ui.rs`, add composer-area mouse handling:

| Event | Behavior |
|---|---|
| `Down(Left)` | Position cursor at char boundary, clear anchor |
| `Drag(Left)` | Set anchor to click position (if none), continuously move cursor to extend selection |
| `Up(Left)` | End drag selection |
| `DoubleClick(Left)` | Select word under cursor (anchor=word-start, cursor=word-end) |
| `TripleClick(Left)` | Select entire line |

Coordinate mapping: mouse (col, row) -> char index, using `layout_input()` line-wrapping results for reverse lookup.

Area priority: check composer area first; if inside, handle composer mouse events; otherwise fall through to existing transcript logic.

### Rendering

In `ComposerWidget::render()`, replace uniform single-Span-per-line with selection-aware multi-Span rendering:

- **No selection**: unchanged, one Span per line.
- **With selection**: split each line into up to 3 Spans: `[before, normal] [selected, highlight] [after, normal]`.

Highlight style: `Style::default().fg(TEXT_PRIMARY).bg(Color::Rgb(70, 130, 220))` (blue background).

Multi-line selection: first line highlights from anchor to line end, middle lines fully highlighted, last line highlights from line start to cursor. Uses `layout_input()` wrapping results to compute per-line char ranges.

## Files Changed

| File | Change |
|---|---|
| `crates/tui/src/tui/app.rs` | Add `selection_anchor` field, modify editing methods to handle selection |
| `crates/tui/src/tui/ui.rs` | Add Shift+arrow/Ctrl+A/Ctrl+C/Ctrl+X handling in key dispatch |
| `crates/tui/src/tui/widgets/mod.rs` | Selection-aware rendering in `ComposerWidget` |
| `crates/tui/src/tui/mouse_ui.rs` | Add composer mouse click/drag/double-click handling |
