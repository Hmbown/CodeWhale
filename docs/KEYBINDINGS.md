# Keybindings

This is the source-of-truth catalog of every keyboard shortcut the TUI recognizes. Bindings are grouped by **context** — the focus or modal state they fire in. A binding listed under "Composer" only takes effect when the composer is focused; one under "Transcript" only when the transcript has focus; and so on.

Global key chords are not yet user-configurable — tracked for a future release (#436, #437). Hotbar slot actions are configurable with `[[hotbar]]` and `/hotbar`; the Hotbar activation chord remains `Alt-1` through `Alt-8`.

## Global (any context)

| Chord                | Action                                                        |
|----------------------|---------------------------------------------------------------|
| `F1` or `Ctrl-/`     | Toggle the help overlay                                       |
| `Ctrl-K`             | Open the command palette (slash-command finder)                |
| `Ctrl-C`             | Cancel current turn / dismiss modal / arm-then-confirm quit    |
| `Ctrl-B`             | Move a supported foreground shell wait into `/jobs` so the turn can continue; use `/jobs` or `exec_shell_wait` to inspect it |
| `Ctrl-D`             | Quit (only when the composer is empty)                         |
| `Tab`                | Cycle TUI mode: Plan → Act → Operate → Plan                    |
| `Shift-Tab`          | Cycle permission posture: Ask → Auto-Review → Full Access                    |
| `Ctrl-T`             | Cycle reasoning effort for the active provider. DeepSeek-style providers cycle off → high → max → off; OpenAI Codex cycles low → medium → high → xhigh → low. |
| `Ctrl-Shift-T`       | Toggle live transcript overlay (sticky-tail auto-scroll)                       |
| `Ctrl-R`             | Open the resume-session picker                                 |
| `Ctrl-L`             | Refresh / clear the screen                                     |
| `Ctrl-O`             | Open the whole-turn Turn Inspector, regardless of composer contents |
| `Alt-V` / `Option-V` (macOS) | Open the details pager for the selected, visible, or most recent tool/sub-agent card; terminals that emit the legacy Option-V glyph are also handled |
| `Ctrl-Shift-E` / `Cmd-Shift-E` | Toggle the file-tree sidebar                          |
| `Alt-G`              | Scroll transcript to top when the composer is empty             |
| `Alt-1`-`Alt-8`      | Dispatch Hotbar slots 1-8 when no modal or inline picker is open |
| `Alt-!` / `Alt-@` / `Alt-#` / `Alt-$` / `Alt-0` | Focus Pinned / Tasks / Agents / Context / Auto sidebar |
| `Ctrl-Alt-0`         | Hide/show the pinned sidebar                                    |
| `Esc`                | Close topmost modal · cancel slash menu · dismiss toast        |

## Composer

Editing the message you're about to send.

| Chord                       | Action                                                  |
|-----------------------------|---------------------------------------------------------|
| `Enter`                     | Send the message (or run the slash command)             |
| `Alt-Enter` / `Ctrl-J`      | Insert a newline without sending (`Ctrl-J` force-steers while a turn is running) |
| `Ctrl-Enter` / `Cmd-Enter`  | Force a live steer into the current turn when supported by the terminal |
| `Ctrl-U`                    | Clear the whole draft (recoverable — see `Ctrl-Z`)      |
| `Ctrl-Z`                    | Restore the cleared draft (only while the composer is empty) |
| `Ctrl-W`                    | Delete previous word                                    |
| `Ctrl-A` / `Home`           | Move to start of input / start of line (readline convention) |
| `Ctrl-E` / `End`            | Move to end of input / end of line                      |
| `Ctrl-←` / `Alt-←`          | Move backward one word                                  |
| `Ctrl-→` / `Alt-→`          | Move forward one word                                   |
| `Shift-←` / `Shift-→`       | Extend the selection one grapheme at a time             |
| `Ctrl-Shift-←/→` / `Alt-Shift-←/→` | Extend the selection one word at a time          |
| `Shift-Home` / `Shift-End`  | Extend the selection to the start / end of the line     |
| `Ctrl-Shift-Home` / `Ctrl-Shift-End` | Extend the selection to the start / end of the draft |
| `Ctrl-Shift-A` / `Cmd-A`    | Select the whole draft (see note below)                 |
| Mouse drag                  | Select composer text; click moves the cursor            |
| `Cmd-V` / `Ctrl-Shift-V`    | Terminal-local paste (arrives as bracketed paste when supported) |
| `Ctrl-V`                    | Direct clipboard paste in a local or forwarded graphical session |
| `Ctrl-Y`                    | Yank (paste) from kill buffer                           |
| `↑` / `↓`                   | Cycle composer history (also selects popup/attachment items) |
| `Ctrl-P` / `Ctrl-N`         | Cycle composer history (alternative)                     |
| `Ctrl-S`                    | Stash current draft; with queued follow-ups during a running turn, send the next queued item now |
| `Alt-R`                    | Search prompt history (Alt-R to exit)                  |
| `Tab`                       | Slash-command / `@`-mention completion (popup-aware)    |
| `Ctrl-Shift-O` / `F4`       | Open the composer draft in `$VISUAL` / `$EDITOR`; F4 works when the terminal cannot distinguish Ctrl-Shift-O from Ctrl-O |
| `! command`                 | Run a shell command through normal approval, sandbox, and output surfaces |

### Selection semantics

Typing, pasting, `Backspace`, or `Delete` with an active selection replaces or
removes the selected text, like any GUI editor. Plain movement keys (arrows,
`Home`/`End`, word motions) collapse the selection. When a selection covers the
whole draft, deleting or typing over it stashes the outgoing text the same way
`Ctrl-U` does, so `Ctrl-Z` (on an empty composer) or `Alt-R` draft recovery can
bring it back.

Cursor movement and deletion are grapheme-aware: one `←`/`→` step or one
`Backspace` covers a full emoji ZWJ sequence, flag pair, or combining-mark
cluster — never half of one. CJK text moves and deletes per character as
expected.

**Why select-all is not `Ctrl-A`:** the composer follows the readline
convention, where `Ctrl-A` jumps to the start of the input (paired with
`Ctrl-E`). Select-all is `Ctrl-Shift-A` on every platform (like
`Ctrl-Shift-O` / `Ctrl-Shift-E`, it needs a terminal with an enhanced keyboard
protocol). On macOS terminals that forward the Command key (kitty, WezTerm,
iTerm2 with Command remapping), native `Cmd-A` also selects all; `Cmd-Shift-A`
works everywhere on macOS because Cmd normalizes to Ctrl.

### Hotbar

Hotbar trigger semantics are intentionally `Alt-1` through `Alt-8` only. On macOS keyboards this is the Option/Alt key plus the number row. Bare `1`-`8` is normal text input in the composer and remains owned by pickers, onboarding, approval prompts, and modal views.

Function keys and `Cmd-1` through `Cmd-8` are not the primary Hotbar chords. Many terminals reserve those keys for tabs, windows, or OS shortcuts, and some never forward them to terminal apps. If a terminal is configured to send `Alt-1` for a custom shortcut, the Hotbar receives the same reliable chord.

Fresh configs resolve to this default bar unless `[[hotbar]]` overrides it or `hotbar = []` disables it:

| Slot | Chord   | Default action     | Label     |
|------|---------|--------------------|-----------|
| 1    | `Alt-1` | `voice.toggle`     | `voice`   |
| 2    | `Alt-2` | `session.compact`  | `compact` |
| 3    | `Alt-3` | `mode.plan`        | `plan`    |
| 4    | `Alt-4` | `mode.agent`       | `agent`   |
| 5    | `Alt-5` | `mode.operate`     | `operate` |
| 6    | `Alt-6` | `palette.open`     | `palette` |
| 7    | `Alt-7` | `sidebar.toggle`   | `side`    |
| 8    | `Alt-8` | `trust.toggle`     | `trust`   |

| Focus state | Hotbar behavior |
|-------------|-----------------|
| Composer empty, text, or whitespace | `Alt-1`-`Alt-8` dispatches a configured slot |
| Sidebar focused, hidden, or auto | `Alt-1`-`Alt-8` still dispatches a configured slot |
| Slash menu or history search open | Blocked; the inline selector owns the key event |
| Command palette, help, approval, file picker, session picker, Fleet setup, or any modal stack | Blocked; the modal owns the key event |
| Onboarding | Blocked; onboarding owns numeric choices |

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
| Mouse drag           | Select transcript text in Codewhale                |
| `Ctrl-C`             | Copy an active Codewhale selection                 |
| `Cmd-click` (macOS) / `Ctrl-click` (Linux/Windows) | Open an OSC 8 link in a supporting terminal (terminal-owned) |

For terminal-native selection, hold `Shift` while dragging (terminal support
varies), then use the terminal's own copy command: usually `Cmd-C` on macOS or
`Ctrl-Shift-C` on Linux/Windows. Those commands are handled by the local
terminal and are intentionally separate from Codewhale's `Ctrl-C` selection
binding. Over SSH, Codewhale sends copy requests back through OSC 52, or via
tmux's `load-buffer -w` path when running inside tmux.

## Sidebar (when sidebar has focus)

| Chord                | Action                                              |
|----------------------|-----------------------------------------------------|
| `↑` / `↓` / `j` / `k`| Move selection                                     |
| `Enter`              | Activate the selected item (open / focus / cancel) |
| `Tab`                | Cycle to next sidebar panel (Work → Tasks → Agents → Context) |
| `Ctrl-X`             | Cancel all running background shell jobs when the Tasks panel is focused |
| `Esc`                | Return focus to composer                           |

## Slash-command palette (after `Ctrl-K` or typing `/`)

| Chord                          | Action                                              |
|--------------------------------|-----------------------------------------------------|
| `↑` / `↓` / `Ctrl+P` / `Ctrl+N`| Move selection                                     |
| `Enter` / `Tab`                | Run / complete the highlighted command             |
| `Esc`                          | Dismiss palette                                     |

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
| `Enter`              | Advance to next step (Welcome → Language → API/trust gates → setup checkpoint) |
| `Esc`                | Step back one screen                                |
| `1`–`9`              | Pick a language (Language step)                    |
| `0`–`9`              | Pick a provider (Provider step; SGLang, vLLM, and Ollama are keyless by default) |
| `y` / `Y`            | Trust the workspace (Trust step)                   |
| `n` / `N`            | Skip the trust prompt                              |

## v0.8.29 audit notes

- **`Shift+Enter` / `Alt+Enter` newlines now work in VSCode on Windows (#1359).** crossterm's `PushKeyboardEnhancementFlags` command unconditionally returns `Unsupported` on Windows (`is_ansi_code_supported() == false`), so the Kitty keyboard protocol escape was never written to the terminal. Without it, VSCode's xterm.js stays in legacy mode where `Shift+Enter` is indistinguishable from plain `Enter`, causing the composer to send the message instead of inserting a newline. The fix writes the push/pop escapes (`\x1b[>1u` / `\x1b[<1u`) directly on Windows, bypassing crossterm's capability gate. VSCode integrated terminal and Windows Terminal ≥1.17 both honour the Kitty keyboard protocol; terminals that do not understand the sequences silently discard them.

## v0.8.13 audit notes

- **Ctrl-S is stash, not history search.** Fixed in this revision — `Alt-R` is history search.
- **Phantom `Alt+Up` removed.** The "Edit last queued message" binding was listed in README but never existed in the key dispatch code.
- **Bare Up/Down arrows scroll transcript when composer empty (v0.8.13).** Previously the `should_scroll_with_arrows` gate was hardcoded to false, meaning bare arrows always navigated composer history even when the composer was empty. Users in virtual terminals (Ghostty, Codex, Kitty-protocol) were especially affected because they couldn't use Cmd+Up / Alt+Up shortcuts.
- **Configurable keymap (#436) and `tui.toml` (#437) remain deferred.** The `TuiPrefs` struct and loader exist in `settings.rs` but are not wired at startup. The named-binding registry that would let `~/.codewhale/tui.toml` override individual entries is still pending.
- **No other broken bindings found.** Every other chord listed above resolves to a live handler in `crates/tui/src/tui/ui.rs` (key-event dispatch) or `crates/tui/src/tui/app.rs` (mode + state transitions).
