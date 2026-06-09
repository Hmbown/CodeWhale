# Multi-Tab & Internationalization (i18n) Feature Guide

> Branch: `feat/multi-agent-v0850` · PR: #2753 · 22 commits · +3,872/-326 · 29 files

## 1. Multi-Tab System

### Concepts

| Concept | Description |
|---|---|
| **Tab** | A concurrent conversation in the same window. Up to 9 tabs. |
| **Session** | A saved conversation on disk (`~/.codewhale/sessions/`). Invoke with `Ctrl+R`. |
| **Session ≠ Tab** | Sessions are persistent conversation files. Tabs are ephemeral concurrent slots. |

### Keyboard Shortcuts

| Shortcut | Action | Notes |
|---|---|---|
| `Ctrl+T` | New tab | Standard browser/VSCode convention |
| `Ctrl+W` | Close active tab | Only when tabs exist (doesn't fire while composing) |
| `` Ctrl+` `` | Open tab switcher | Overlay with search/filter |
| `Ctrl+1` ~ `Ctrl+9` | Jump to tab N | Direct access |
| `Ctrl+PageDown` | Next tab | **Windows Terminal-safe alternative to Ctrl+Tab** |
| `Ctrl+PageUp` | Previous tab | **Windows Terminal-safe alternative to Ctrl+Shift+Tab** |
| `Ctrl+Tab` | Next tab | Works on Linux/macOS; intercepted by Windows Terminal |
| `Ctrl+Shift+Tab` | Previous tab | Works on Linux/macOS; intercepted by Windows Terminal |

> **Windows Note**: `Ctrl+Tab`, `Ctrl+Shift+Tab`, `Ctrl+Shift+N`, `Ctrl+Shift+W` are intercepted by Windows Terminal before reaching the TUI. Use `Ctrl+PageDown/Up`, `Ctrl+T`, and `Ctrl+W` instead.

### Tab Bar

- **Empty state**: Yellow hint bar: `[Ctrl+T] New tab | Ctrl+`: Switcher | Ctrl+PageDown/Up: next/prev tab`
- **With tabs**: DarkGray bar showing `[💬 1:title*]` for active tab, `[💬 2:title]` for others
- Each tab shows an icon by type: 💬 Chat · 📤 Delegation · 🔍 Review · 👥 Meeting

### Tab Switcher (`Ctrl+` `` ` ``)

- Title: "Open Tabs (Ctrl+PageDown/Up to switch)"
- Lists all tabs with type indicators and unread counts
- Number keys 1-9 for direct selection
- Text filter: start typing to filter by title

---

## 2. Internationalization (i18n)

### Supported Languages

| Locale | Language | Coverage |
|---|---|---|
| `en` | English | 100% (source language) |
| `zh-Hans` | Simplified Chinese (简体中文) | 100% (full translation) |
| `zh-Hant` | Traditional Chinese (繁體中文) | Core + status messages |
| `ja` | Japanese (日本語) | Full (via English fallback) |
| `pt-BR` | Portuguese (Brazil) | Full (via English fallback) |
| `es-419` | Spanish (Latin America) | Full (via English fallback) |
| `vi` | Vietnamese (Tiếng Việt) | Full (via English fallback) |

### Switching Language

| Method | How |
|---|---|
| **Slash command** | `/locale [code]` (e.g., `/locale zh-Hans`) |
| **Hotkey** | `Ctrl+Shift+L` cycles: en → zh-Hans → ja → pt-BR → es-419 → vi → zh-Hant → en |
| **Settings UI** | `/config` → navigate to `locale` → Enter to edit |
| **Config file** | `settings.toml`: `locale = "zh-Hans"` |
| **Environment** | `LC_ALL=zh_CN.UTF-8` (auto-detected when `locale = "auto"`) |

### What's Localized

All **status bar messages**, **context menus**, **command palette**, **help text**, and **config UI** respond to the active locale. The footer displays a `🌐 <code>` chip when a non-English locale is active.

---

## 3. All Keyboard Shortcuts (Complete Reference)

### Multi-Tab (New)

| Shortcut | Action |
|---|---|
| `Ctrl+T` | New tab |
| `Ctrl+W` | Close tab |
| `` Ctrl+` `` | Tab switcher |
| `Ctrl+1` ~ `Ctrl+9` | Jump to tab N |
| `Ctrl+PageDown` | Next tab |
| `Ctrl+PageUp` | Previous tab |

### i18n (New)

| Shortcut | Action |
|---|---|
| `Ctrl+Shift+L` | Cycle locale |
| `/locale [code]` | Switch locale by code |
| `/locale` (no arg) | Show current + available list |

### Original CodeWhale (Unchanged)

| Shortcut | Action |
|---|---|
| `Ctrl+C` | Cancel / interrupt |
| `Ctrl+/` | Slash command panel |
| `Ctrl+K` | Clear screen |
| `Ctrl+L` | Compact context |
| `Ctrl+R` | Session picker |
| `Ctrl+O` | Open composer in $EDITOR |
| `Ctrl+E` | Toggle Vim mode |
| `Ctrl+U` | Clear composer |
| `Ctrl+Y` | Redo |
| `Ctrl+X` | Review / plan mode |
| `Alt+1/2/3/4` | Sidebar focus |
| `Ctrl+Enter` | Submit with force |

---

## 4. Implementation Notes

- **MessageId enum**: ~430 variants in `crates/tui/src/localization.rs`
- **tr() function**: `tr(locale: Locale, id: MessageId) -> &'static str`
- **Fallback chain**: Non-English → English (via `_ => return None` catch-all)
- **Test coverage**: 4312/4315 tests pass. 3 Windows-specific shell tests fail (pre-existing).

## 5. PR Status

- **#2864 (narrow tab-core)**: MERGED into stewardship (2026-06-06)
- **#2753 (full multi-tab + i18n)**: OPEN / MERGEABLE. Base: `codex/v0.9.0-stewardship@f88528a5`. Head: `feat/multi-agent-v0850` (fom `ljm3790865/CodeWhale-multi`).
