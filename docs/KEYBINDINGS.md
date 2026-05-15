# Keybindings

This is the source-of-truth catalog of every keyboard shortcut the TUI recognizes. Bindings are grouped by **context** — the focus or modal state they fire in. A binding listed under "Composer" only takes effect when the composer is focused; one under "Transcript" only when the transcript has focus; and so on.

Bindings are not (yet) user-configurable — tracked for a future release (#436, #437). This document is the contract that future config-file overrides will name into.

## Global (any context)

| Chord                | Action                                                        |
|----------------------|---------------------------------------------------------------|
| `F1` or `Ctrl-/`     | Toggle the help overlay                                       |
| `Ctrl-K`             | Open the command palette (slash-command finder)                |
| `Ctrl-C`             | Cancel current turn / dismiss modal / arm-then-confirm quit    |
| `Ctrl-D`             | Quit (only when the composer is empty)                         |
| `Tab`                | Cycle TUI mode: Plan → Agent → YOLO → Plan                     |
| `Shift-Tab`          | Cycle reasoning effort: off → high → max → off                 |
| `Ctrl-R`             | Open the resume-session picker                                 |
| `Ctrl-L`             | Refresh / clear the screen                                     |
| `Ctrl-O`             | Open Activity Detail for selected/live/recent tool work, or the full reasoning timeline for thinking blocks when the composer is empty |
| `Ctrl-Shift-E` / `Cmd-Shift-E` | Toggle the file-tree sidebar                          |
| `Alt-!` / `Alt-@` / `Alt-#` / `Alt-$` / `Alt-0` | Focus Work / Tasks / Agents / Context / Auto sidebar |
| `Ctrl-Alt-0`         | Hide the right sidebar                                          |
| `Esc`                | Close topmost modal · cancel slash menu · dismiss toast        |

## Composer

Editing the message you're about to send.

| Chord                       | Action                                                  |
|-----------------------------|---------------------------------------------------------|
| `Enter`                     | Send the message (or run the slash command)             |
| `Alt-Enter` / `Ctrl-J`      | Insert a newline without sending                        |
| `Ctrl-U`                    | Delete to start of line                                 |
| `Ctrl-W`                    | Delete previous word                                    |
| `Ctrl-A` / `Home`           | Move to start of line                                   |
| `Ctrl-E` / `End`            | Move to end of line                                     |
| `Ctrl-←` / `Alt-←`          | Move backward one word                                  |
| `Ctrl-→` / `Alt-→`          | Move forward one word                                   |
| `Ctrl-V` / `Cmd-V`          | Paste from clipboard (also bracketed-paste auto-handled)|
| `Ctrl-Y`                    | Yank (paste) from kill buffer                           |
| `↑` / `↓`                   | Cycle composer history (also selects popup/attachment items) |
| `Ctrl-P` / `Ctrl-N`         | Cycle composer history (alternative)                     |
| `Ctrl-S`                    | Stash current draft (`/stash list`, `/stash pop` to recover) |
| `Alt-R`                    | Search prompt history (Alt-R to exit)                  |
| `Tab`                       | Slash-command / `@`-mention completion (popup-aware)    |
| `Ctrl-O`                    | Open external editor for the composer draft when it has focus |

### `@` mentions

Type `@<partial>` to open the file mention popup. `↑`/`↓` cycle the entries, `Tab` or `Enter` accepts. `Esc` hides the popup. As of v0.8.10 (#441), completions are re-ranked by mention frecency — files you mention often + recently float to the top.

### `#` quick-add (memory)

When `[memory] enabled = true`, typing `# foo` and pressing `Enter` appends `foo` as a timestamped bullet to your memory file *without* sending a turn. See `docs/MEMORY.md`.

## Transcript (when transcript has focus)

| Chord                | Action                                              |
|----------------------|-----------------------------------------------------|
| `↑` / `↓` / `j` / `k`| Scroll one line (v0.8.13+: bare arrows also scroll when composer empty) |
| `PgUp` / `PgDn`      | Scroll one page                                    |
| `Home` / `g`         | Jump to top                                         |
| `End` / `G`          | Jump to bottom                                     |
| `Esc`                | Return focus to composer                           |
| `y`                  | Yank selected region to clipboard                  |
| `v`                  | Begin / extend visual selection                    |
| `o`                  | Open URL under cursor (OSC 8 capable terminals)    |

## Sidebar (when sidebar has focus)

| Chord                | Action                                              |
|----------------------|-----------------------------------------------------|
| `↑` / `↓` / `j` / `k`| Move selection                                     |
| `Enter`              | Activate the selected item (open / focus / cancel) |
| `Tab`                | Cycle to next sidebar panel (Work → Tasks → Agents → Context) |
| `Esc`                | Return focus to composer                           |

## Slash-command palette (after `Ctrl-K` or typing `/`)

| Chord                | Action                                              |
|----------------------|-----------------------------------------------------|
| `↑` / `↓`            | Move selection                                     |
| `Enter` / `Tab`      | Run / complete the highlighted command             |
| `Esc`                | Dismiss palette                                     |

## Session Picker (`Ctrl-R` or `/sessions`)

| Chord                | Action                                              |
|----------------------|-----------------------------------------------------|
| `↑` / `↓` / `j` / `k`| Move selection in the session list                 |
| `1`-`9`              | Open the visible session history at that list slot |
| `PgUp` / `PgDn`      | Page the history pane                              |
| `Enter`              | Resume the selected session                        |
| `/`                  | Search sessions                                    |
| `s`                  | Cycle sort order                                   |
| `a`                  | Toggle current-workspace scope vs all workspaces   |
| `d`                  | Delete selected session after confirmation         |
| `Esc` / `q`          | Close the picker                                   |

## Approval modal (when a tool requests approval)

| Chord                | Action                                              |
|----------------------|-----------------------------------------------------|
| `y` / `Y`            | Approve once                                        |
| `a` / `A`            | Approve all (auto-approve subsequent calls)        |
| `n` / `N` / `Esc`    | Deny                                                |
| `e`                  | Edit the approved input before running              |

## Onboarding (first-run flow)

| Chord                | Action                                              |
|----------------------|-----------------------------------------------------|
| `Enter`              | Advance to next step (Welcome → Language → API → …) |
| `Esc`                | Step back one screen                                |
| `1`–`5`              | Pick a language (Language step)                    |
| `y` / `Y`            | Trust the workspace (Trust step)                   |
| `n` / `N`            | Skip the trust prompt                              |

## v0.8.29 audit notes

- **`Shift+Enter` / `Alt+Enter` newlines now work in VSCode on Windows (#1359).** crossterm's `PushKeyboardEnhancementFlags` command unconditionally returns `Unsupported` on Windows (`is_ansi_code_supported() == false`), so the Kitty keyboard protocol escape was never written to the terminal. Without it, VSCode's xterm.js stays in legacy mode where `Shift+Enter` is indistinguishable from plain `Enter`, causing the composer to send the message instead of inserting a newline. The fix writes the push/pop escapes (`\x1b[>1u` / `\x1b[<1u`) directly on Windows, bypassing crossterm's capability gate. VSCode integrated terminal and Windows Terminal ≥1.17 both honour the Kitty keyboard protocol; terminals that do not understand the sequences silently discard them.

## v0.8.13 audit notes

- **Ctrl-S is stash, not history search.** Fixed in this revision — `Alt-R` is history search.
- **Phantom `Alt+Up` removed.** The "Edit last queued message" binding was listed in README but never existed in the key dispatch code.
- **Bare Up/Down arrows scroll transcript when composer empty (v0.8.13).** Previously the `should_scroll_with_arrows` gate was hardcoded to false, meaning bare arrows always navigated composer history even when the composer was empty. Users in virtual terminals (Ghostty, Codex, Kitty-protocol) were especially affected because they couldn't use Cmd+Up / Alt+Up shortcuts.
- **Configurable keymap (#436) and `tui.toml` (#437) remain deferred.** The `TuiPrefs` struct and loader exist in `settings.rs` but are not wired at startup. The named-binding registry that would let `~/.deepseek/tui.toml` override individual entries is still pending.
- **No other broken bindings found.** Every other chord listed above resolves to a live handler in `crates/tui/src/tui/ui.rs` (key-event dispatch) or `crates/tui/src/tui/app.rs` (mode + state transitions).
