# Computer use, phase 2: background operation and element-level control

Status: proposed (2026-08-24). Builds on
[COMPUTER_USE_PLUGIN.md](COMPUTER_USE_PLUGIN.md) (phase 1: pixel-based,
foreground, five platforms).

## 1. What the two reference implementations do

Both were examined directly: Kimi Computer Use from the installed plugin
(`~/.kimi-code/plugins/managed/kimi-cu`, `KimiCU.app` 0.5.8, its MCP tool
schemas and SKILL.md) and OpenAI Codex from its published write-ups (April
16 2026 "Codex for (almost) everything", May 22 locked-Mac update, May 29
Windows update).

| Capability | Kimi CU (macOS) | Codex (macOS) | Codex (Windows) | Codewhale phase 1 |
| --- | --- | --- | --- | --- |
| Perception | window-scoped screenshot + **indexed AX tree** (`get_app_state`, `ax_filter`, `mode`) | AX hierarchy incl. **off-screen text** ("Appshots": window image + exposed text) | screenshots + windows/menus | full-screen screenshot, pixels only |
| Targeting | AX node **index** or screenshot pixel | AX element | pixel / UI | pixel |
| Background | **yes**: never moves the real cursor, never foregrounds; works on covered windows | **yes**: parallel **virtual cursors** per agent, apps stay in background | **no**: foreground, moves the real pointer | no |
| Locked screen | no | **yes**: authorization plug-in in the unlock flow, display covered, relocks on local input | no | no |
| Writes | `set_value` (AX write + read-back), `type_text` with clear/submit/verify, `select_text` | click/type/menus/clipboard | click/type/clipboard | type/key only |
| Menus | `perform_secondary_action` (AXShowMenu) | menu navigation via AX | menus | right-click pixel |
| Scroll | element or page scroll with **movement detection** (`ok:false` at end) | — | — | wheel notches |
| Consent model | plugin trust + TCC | **per-app allowlist**, one-time or "Always allow"; asks before sensitive/disruptive actions; pauses for passwords | same | plugin trust + MCP tool approval; skill-level stop rules |
| Excluded targets | — | Terminal apps, Codex itself, admin/security prompts | same | — |
| Permission holder | signed app + **launchd service** (agent process needs nothing) | signed app + authorization plug-in | app | the terminal app running Codewhale |
| Install | `/plugins` Official tab, managed runtime, `kimi-cu upgrade` | Codex app plug-in | Codex app | `codewhale computer-use setup`, `official` catalog |
| Phones / Linux | — | — | — | Android (adb), HarmonyOS (hdc), Linux |

The advantages worth taking are therefore: (a) element-level perception and
writes through the accessibility tree, (b) input delivered to a target
process/window without touching the user's cursor or foreground, (c)
window-scoped capture, (d) verification built into the tools, (e) a per-app
consent model with hard exclusions. Locked-screen operation (Codex) needs an
authorization plug-in installed with admin rights; it is out of scope here and
noted as a later, separately reviewed step.

## 2. How background input works on macOS (the part that matters)

Neither product uses a second login session or virtual display. Both rely on
the same two OS facilities, which is what makes "no cursor, no foreground"
possible for a process holding Accessibility:

1. **Accessibility actions and attribute writes** — `AXUIElementPerformAction`
   (`AXPress`, `AXShowMenu`, `AXScrollToVisible`, …) and
   `AXUIElementSetAttributeValue` (`AXValue`, `AXFocused`, `AXSelectedTextRange`).
   These act on the element directly; nothing on screen moves. Read-back via
   `AXUIElementCopyAttributeValue` gives verification for free.
2. **Process-targeted events** — `CGEventPostToPid(pid, event)` delivers
   mouse/keyboard events to one process at window-local coordinates without
   posting to the HID system tap, so the system cursor does not move and the
   window is not raised. (Kimi's binary confirms this path:
   `AXUIElementCopyElementAtPosition`, `AXUIElementPerformAction`,
   "chromium-no-raise-focus"; Electron/Chromium ignores synthetic AX presses
   for focus, so both products fall back to a background click to establish
   render-layer focus before typing.)

Window-scoped capture: `CGWindowListCreateImage(rect, kCGWindowListOptionIncludingWindow, windowID, …)`
captures one window's contents even when it is covered (Screen Recording
permission), which is also what makes covered-window operation observable.

Limits that both products document and we inherit: apps that drop input
when occluded (Kimi exposes `activate: true` as a fallback that briefly
raises the window), Chromium drag unreliability, and Terminal/agent-self
exclusions.

## 3. Design

### 3.1 New driver capability: `ElementDriver` (macOS first)

```rust
pub trait ElementDriver {
    fn apps(&mut self) -> Result<Vec<AppInfo>>;                       // name, bundle id, pid, windows
    fn app_state(&mut self, target: &AppTarget, opts: StateOpts)     // window-scoped capture + AX tree
        -> Result<AppState>;                                          // nodes carry index, role, title/value,
                                                                      // frame (window px), actions, focused,
                                                                      // enabled, off-screen text when exposed
    fn act(&mut self, target: &AppTarget, action: ElementAction) -> Result<ActionReceipt>;
}
pub enum ElementAction {
    Press { index },                       // AXPress, fallback: background click at node center
    Click { at: WindowPoint, button, count, hold_ms },   // CGEventPostToPid, never raises
    SetValue { index, value },             // AXValue write + read-back; Electron path: focus + select-all + type
    Type { text, index: Option, clear, submit }, // read-back verification when the element exposes text
    Key { combo },                         // CGEventPostToPid keyboard events
    ShowMenu { index },                    // AXShowMenu
    Scroll { index_or_point, pages },      // AXScrollToVisible / wheel events; reports moved: bool
    Drag { from, to },                     // background drag; documented as unreliable on Chromium
    Raise { }                              // explicit, visible fallback (Kimi's `activate`)
}
```

`AppTarget` is a bundle id, app name, or pid; every tool requires one. The
phase-1 `Driver` stays for whole-screen/foreground use and for Windows,
Linux, Android, HarmonyOS. The macOS driver implements both traits.

### 3.2 Tool surface additions

| tool | purpose |
| --- | --- |
| `computer_apps` | running apps (name, bundle id, pid, window titles); the allowlist state per app |
| `computer_app_state` | `{app, mode: full\|image\|ax, filter?, window_id?}` → window image (scaled to the model budget) + indexed AX tree text; caches the snapshot for index-based actions |
| `computer_element` | `{app, index, action: press\|set_value\|menu\|scroll\|select_text, value?, pages?}` — AX-level actions with read-back |
| existing `computer_click/type/key/scroll/drag` | gain an `app` argument; with `app` set they run in the background against that app's window (window-local pixels of the last `computer_app_state` image); without it they keep phase-1 foreground behavior |
| `computer_raise` | explicit fallback that brings the window forward when an app drops occluded input |

Result texts carry `verified: true/false` (read-back), `moved: true/false`
for scrolls, and `occluded: true` when the window is fully covered.

### 3.3 Consent model (Codex-style, enforced server-side)

- Per-app allowlist in `~/.codewhale/computer-use.toml`:
  `[apps] allow = ["com.apple.Notes", "Calculator"]`, `deny = [...]`.
  A tool call on an app not in `allow` returns a structured
  `needs_app_approval` error naming the app and the exact line to add; the
  `/computer` command surfaces it as a question to the user. (Codewhale's
  MCP tool approval remains in front of every call; the allowlist is an
  additional, durable, per-target gate.)
- Hard exclusions, not configurable: the terminal app that hosts Codewhale,
  Codewhale/`codew` itself, `SecurityAgent`/`loginwindow`/System Settings
  privacy panes, and password fields (`AXSecureTextField` never receives
  `set_value`/`type`).
- Sensitive-action pause stays in the Skill: the model must ask before
  passwords, payments, deletions, sends, permission dialogs.

### 3.4 Permissions

Phase 1 already requests Accessibility + Screen Recording for the terminal
app. A signed helper app that owns the permissions (Kimi/Codex style) is the
right long-term shape but needs a code-signing identity and notarization in
the release pipeline; it is a separate, later decision. Until then
`computer_info`/`setup` keep reporting the terminal-app state honestly.

### 3.5 Windows and Linux

- Windows: UI Automation (COM) provides the same tree/actions
  (`IUIAutomation`, `InvokePattern`, `ValuePattern`); background input to a
  specific window is possible with `PostMessage`/UIA patterns but, like
  Codex on Windows, true cursor-free operation is not generally reliable —
  document as foreground-only for actions, background for reads.
- Linux: AT-SPI2 over D-Bus for the tree (GTK/Qt/Electron with a11y enabled);
  input remains xdotool/ydotool (foreground). Out of the first cut.
- Android/HarmonyOS already have UI trees; `computer_element` can map
  `press` to a tap at the node center and `set_value` to `input text`.

### 3.6 Model fit

DeepSeek V4 Flash Vision reads at most ~800 px per image, so the AX text
(free, exact) is the primary channel and the window image is confirmation.
`computer_app_state` defaults to `mode: full` with the tree capped to N
interactive nodes and `filter` for large windows; the Skill instructs: state →
act by index → state again, and reserves pixel clicks for canvases without
AX.

## 4. Delivery

1. macOS `ElementDriver`: AX FFI (`AXUIElementCreateApplication`,
   `AXUIElementCopyAttributeValue(s)`, `AXUIElementPerformAction`,
   `AXUIElementSetAttributeValue`, `CGEventPostToPid`,
   `CGWindowListCreateImage`, `NSRunningApplication` via `osascript`/`lsappinfo`
   or `CGWindowListCopyWindowInfo` for the app/window directory) — the
   largest piece, ~1.5–2k lines including tests against a mock element tree.
2. Tools, allowlist, exclusions, Skill update, `setup` writing an initial
   `[apps]` section, docs.
3. Windows UIA reads; Android/HarmonyOS element actions.
4. Later, separately reviewed: signed helper app; locked-screen operation.

Evidence plan: unit tests on the AX tree rendering/index cache and the
allowlist gate; live macOS checks (state of a covered Notes window,
`set_value` into a text field with read-back, background click on a button,
menu open) recorded in `docs/COMPUTER_USE.md`.
