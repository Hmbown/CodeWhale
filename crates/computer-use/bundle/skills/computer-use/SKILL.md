---
name: computer-use
description: Operate a desktop or phone screen with the computer_* tools: screenshot, reason, act, verify. Whole-screen and per-app/background control, coordinate rules, zoom, element trees, per-app consent, and safety stops.
---

# Computer use

You control a real screen through MCP tools whose names start with
`computer_` (they may appear prefixed with the plugin server name). The
model that reads screenshots is DeepSeek V4 Flash Vision or another
vision-capable route; every screenshot costs only a few hundred tokens, so
look often and act in small steps.

## The loop

1. `computer_info` once: learn the target (desktop / android / harmony), the
   display size, and any permission problem.
2. `computer_screenshot`. The result says `frame: WxH`. **All x/y you send
   are pixels of that image**, origin top-left. Never use device pixels or
   percentages.
3. Decide **one** action. Prefer keyboard shortcuts and typing over precise
   clicking when both work (`computer_key "cmd+l"` then `computer_type`).
4. Act. Every action tool returns a new screenshot; read it before the next
   step and confirm the intended change happened. If it did not, do not
   repeat blindly: zoom, re-read, or choose another route.
5. Stop when the task is done or blocked, and report what is on screen.

## Two ways to work

**Whole screen (phase 1).** `computer_screenshot` + pixel coordinates. It
works everywhere, drives whatever is in front, and moves the real cursor.
Use it for the desktop itself, for apps with no accessibility tree (games,
remote desktops, canvases), and on Linux.

**One app (phase 2, macOS).** `computer_apps` → `computer_app_state` →
`computer_element`. This reads the app's **element tree** — exact text and
roles rather than guessed pixels — and acts on the app *in the background*:
the user's cursor never moves, the window is never raised, and a covered
window still works. Prefer it whenever the target is a specific app.

## The app loop

1. `computer_apps` — running apps with their pid, bundle id, window titles,
   and their **consent state** (`allowed` / `needs approval` / `denied` /
   `excluded`).
2. `computer_app_state {app: "Notes"}` — one window as an image plus an
   indexed element list:

   ```text
   #12 [AXTextField] "Search" at (240,64 320x28) press,set_value
   #18 [AXButton] "New Note" at (36,96 120x32) press
   ```

   `mode: ax` skips the image (cheapest, and the text is exact);
   `mode: image` skips the tree; `filter` narrows a large window;
   `window_id` picks one window of many.
3. Act by index — this is the reliable path, and measurably so: element
   actions go through the accessibility API, while a pixel click has to be
   hit-tested back to an element first and a drag cannot be delivered at all.
   Reach for an index before a coordinate.

   - `computer_element {app, action: "press", index: 18}`
   - `computer_element {app, action: "set_value", index: 12, value: "…"}`
     (written through the accessibility API and **read back**; the result
     says `verified: true/false`). In a browser or an Electron app the
     receipt adds a web-content note: the text is in place, but the app's own
     code may not have seen an input event. Check that the app reacted — a
     send button enabling, a character count moving. If it did not,
     `computer_raise` it and type instead.
   - `computer_element {app, action: "menu", index: n}` opens its menu
   - `computer_element {app, action: "scroll", index: n, direction, pages}`
     reports `moved: false` when you are already at the end of the content
   - `computer_element {app, action: "select_text", index: n, start, end}`
4. Or act by pixel *inside that app*: `computer_click`, `computer_type`,
   `computer_key`, `computer_scroll`, `computer_drag` all take an optional
   `app` argument. With `app` set, x/y are pixels of the **last
   `computer_app_state` image** for that app and the input is delivered to
   that app in the background. Without `app` they behave exactly as before.

   Not everything survives the trip to a background app. Presses, value
   writes, text selection, typing, keys, and scrolling do. **Drag does not**
   — use `select_text` to select, and for a genuine drag call
   `computer_raise` first and then the foreground `computer_drag`. A `menu`
   opens in its own window, so look for it with `computer_screenshot`, not in
   the app's window image.
5. `computer_app_state` again to confirm. Element indexes only refer to the
   snapshot that produced them — re-state after anything that changes the
   window.

A receipt means the action was **delivered**, not that the app acted on it.
Some apps ignore keys while they are not active (TextEdit accepts them,
Calculator does not). Always confirm with a fresh `computer_app_state`.

`computer_raise {app}` is the visible fallback: some apps drop input while
they are covered or inactive. Try the background path first, raise only when
the state shows nothing changed, and tell the user you had to.

## Per-app consent

Every app-targeted call is gated by an allowlist in
`~/.codewhale/computer-use.toml`, on top of the usual tool approval:

```toml
[apps]
allow = ["com.apple.Notes", "Calculator"]
deny  = ["Mail"]
```

An unlisted app returns a `needs_app_approval:` error naming the app and the
exact line to add. When you see it: **stop, show the user the app name and
that line, and ask.** Only they can edit the file; never work around the
gate by falling back to whole-screen clicking on that app.

Some targets can never be allowed and say so: the terminal running
Codewhale, Codewhale itself, and security/login/System-Settings surfaces.
Secure text fields refuse `set_value` — ask the user to type the password.

## Precision

- Small targets or small text: `computer_zoom` on a region (frame pixels),
  then either act with `frame: "zoom"` using the zoom image's pixels, or
  convert with the formula printed in the zoom result.
- Unsure where things are: `computer_screenshot` with `grid: true` draws
  labeled lines every 100 px (they are an overlay, not the app).
- Phones (Android/HarmonyOS): call `computer_ui_tree` first. It lists
  buttons/fields with their centers in frame pixels and their text, which
  beats guessing from pixels. `computer_key "back"` / `"apphome"` and
  `computer_app launch <package>` are available there. The app tools work
  too — `computer_app_state` returns the same tree indexed for
  `computer_element` — but phone actions always go to the **foreground**
  app; there is no background lane there.
- After any action the previous zoom is stale; take a new one if needed.
- Double-click: `clicks: 2`. Long press / context menu on phones:
  `hold_ms: 800` or `button: "right"`. Drag/swipe: `computer_drag`.
- Typing goes to the focused control: click the field first, then
  `computer_type`. Newlines in text press Enter. Use `computer_key` for
  shortcuts (`ctrl+a`, `cmd+shift+t`, `alt+f4`, `enter`, `esc`).
- Slow UI (page loads, app launches): `computer_wait` returns a screenshot
  after the delay instead of acting on a stale frame.

## Safety stops

Pause and ask the user before you:

- enter or read passwords, 2FA codes, card numbers, or recovery codes;
- confirm purchases, transfers, deletions, sends, or account changes;
- accept permission prompts, install software, or change system settings;
- act on anything the task did not explicitly cover.

Screenshots are sent to the model provider; do not open private material
unrelated to the task. If a step keeps failing three times, stop and
describe the screen instead of continuing.

## Errors you may see

- `no frame yet` → call `computer_screenshot` first.
- `outside the current frame` → you used coordinates from an older or
  differently sized image; take a new screenshot.
- `observe mode` → the operator disabled input; you can only look.
- `needs_app_approval:` → the app is not in `[apps] allow`. Ask the user and
  show them the line the error names; do not retry until they say yes.
- `no app-state snapshot of X` → call `computer_app_state` on that app
  before acting on it (indexes and app-local x/y come from that snapshot).
- `index N is out of range` → the window changed; re-run
  `computer_app_state` and use the new indexes.
- `app/element tools are not available on this target` → this platform has
  no element lane (Linux, and actions on Windows); use the whole-screen
  tools instead.
- permission/unavailable messages name the fix (grant Screen Recording /
  Accessibility on macOS, install xdotool/grim on Linux, connect a device
  with `adb`/`hdc`). Report them to the user; do not work around them.
