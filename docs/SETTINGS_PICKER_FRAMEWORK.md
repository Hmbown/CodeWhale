# Settings picker framework

Shared settings-picker infrastructure for the underwater TUI lives in
`crates/tui/src/tui/settings_picker/`.

## What it owns

- Option catalog with **current / default / effective**, availability + disabled
  reason, help/detail, optional per-item actions, and narrow-layout preference
- Tab + search filtering with **stable visible indices**
- Keyboard nav (↑/↓/Home/End/digits/Tab), Esc cancel, Enter commit
- Nav-level **preview → commit → cancel** lifecycle via `PickerNavResult`
- Responsive list/detail via `SettingsPickerLayout` (side-by-side when wide;
  stacked or list-only when narrow)

Ocean chrome (swatches, underwater surface paint, locale copy) stays in each
concrete picker so shared contracts do not flatten visual character.

## Migration status

| Picker | Status |
|--------|--------|
| `/theme` | Migrated — uses `SettingsPickerController` + `handle_nav_key`; keeps swatches and live preview |
| `/model` | Hook ready — leave full migration to TUI-DOG-009 sibling (truthful availability / performance) |
| `/provider` | Hook ready — same sibling; do not rewrite while availability work is open |
| Fleet setup | Framework only — billing/Fleet UX sibling owns flow rewrites |

## How to plug in

```rust
use crate::tui::settings_picker::{
    SettingOption, SettingsPickerController, SettingsPickerLayout,
    handle_nav_key, PickerNavResult,
};

let mut controller = SettingsPickerController::new(options, original_id);
let result = handle_nav_key(&mut controller, key, /* allow_search_typing */ true);
match result {
    PickerNavResult::Preview => { /* emit persist:false preview */ }
    PickerNavResult::Commit => { /* emit persist:true and close */ }
    PickerNavResult::Cancel => { /* rollback + close */ }
    _ => {}
}
let layout = SettingsPickerLayout::resolve(area, 34, controller.selected_option());
```

Matrix coverage lives in `settings_picker` unit tests: normal, narrow, disabled,
filtered, previewed, and reverted.

## Typed Settings editor

The complete application Settings editor is `ConfigView` in
`crates/tui/src/tui/views/mod.rs`. It is reachable through `F2`, bare
`/settings`, bare `/config`, and command discovery. `/settings text` preserves
the legacy plain-text diagnostic for headless and compatibility use.

Its `SettingsRegistry` classifies every row as Boolean, choice, integer, text,
action, or read-only while retaining the row's category and session/saved
scope. Boolean rows toggle with Space or Enter; bounded choices use the chooser;
provider/model actions open their full pickers; integer/text rows use the inline
editor. Typing filters the full list, and mouse selection/activation mirrors the
keyboard paths. Setting labels come from the locale packs; raw config keys stay
visible in edit/detail surfaces for diagnostics and compatibility.
