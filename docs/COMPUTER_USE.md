# Computer use

The `computer-use` plugin lets a vision-capable route see a screen and drive
it: take screenshots, click, type, scroll, drag, press keys, launch apps, and
(on phones) read the accessibility tree. The target model is DeepSeek's
`deepseek-v4-flash-vision-exp` (alias `flash-vision`); any route whose offering
row states `image_input: Supported` works the same way.

Targets:

| Target | How pixels and input flow | Notes |
| --- | --- | --- |
| macOS | `screencapture` + CoreGraphics events | Needs Screen Recording and Accessibility for the terminal app |
| Windows | GDI capture + `SendInput` | Primary monitor; per-monitor DPI aware |
| Linux, HarmonyOS PC (glibc) | X11: `xdotool` + `scrot`/`maim`/`import`; Wayland: `grim` + `ydotool`/`wtype` | `computer_info` lists what is missing |
| Android | `adb` (`screencap`, `input`, `uiautomator dump`) | From a host, or on-device from Termux via wireless debugging |
| HarmonyOS / OpenHarmony | `hdc` + `uitest` (`screenCap`, `uiInput`, `dumpLayout`) | From a host with the DevEco/OpenHarmony toolchains |

Design: [`design/COMPUTER_USE_PLUGIN.md`](design/COMPUTER_USE_PLUGIN.md).

## Install and enable

The bundle is compiled into the `codewhale` binary (source of truth:
`crates/computer-use/bundle/`). It contributes an MCP server
(`codewhale computer-use serve`), the `computer-use:computer-use` Skill, the
`computer-operator` Agent profile, and the `/computer` command.

One step from the shell — writes the bundle to `~/.codewhale/plugins/computer-use`,
asks macOS for the Accessibility and Screen Recording prompts, takes a test
capture, and prints the in-session commands that finish activation:

```text
codewhale computer-use setup
```

Or from inside a session, from the built-in `official` catalog (equivalently
`/plugin install builtin:computer-use`):

```text
/plugin marketplace list                      # shows official → computer-use
/plugin marketplace install official computer-use
```

Either way the bundle lands disabled and untrusted; finish with the review:

```text
/plugin enable computer-use        # prints the review and the exact trust command
/plugin trust computer-use <content-hash>.<capability-hash>
/plugin enable computer-use
```

`/plugin update computer-use` re-stages the bundle embedded in the current
binary when it changed (after a Codewhale upgrade); `/plugin uninstall
computer-use` removes it. `codewhale computer-use setup --force` overwrites a
hand-placed copy; `--no-permissions` skips the OS prompts.

Trust is the hash-bound review described in [PLUGIN_BUNDLES.md](PLUGIN_BUNDLES.md).
The review shows the stdio command (`codewhale computer-use serve`) and the
usual warning that a stdio child runs with your host authority; there is no
OS sandbox around screen capture or input injection. MCP tool approval still
applies to every `computer_*` call.

The server binary is the `codewhale` executable itself, so nothing else needs
to be installed on desktop. The bundle declares `when.binaries = ["codewhale"]`
and its MCP server runs `codewhale computer-use serve`, so `codewhale` must be
on PATH (npm or release installs are); for a source build, put
`target/release/codewhale` on PATH or run `codewhale-computer-use` from
`cargo build -p codewhale-computer-use` and point a workspace copy of
`mcp.json` at it. Phones need `adb` (Android platform-tools) or
`hdc` (OpenHarmony SDK toolchains) reachable from PATH or the SDK locations
the server probes.

## Using it

Run the session on the vision route and ask:

```text
/model flash-vision
/computer open System Settings and turn on Night Shift
```

From a text-only route (for example `deepseek-v4-pro`) the `/computer` command
delegates to the `computer-operator` profile, which pins
`deepseek` / `deepseek-v4-flash-vision-exp`. Fleet refuses to reroute that
vision-bound member to a route without image input, so a misconfigured
provider fails loudly instead of silently running blind.

The model's loop is documented in the Skill: screenshot → one action →
verify from the screenshot the action returns → next. Coordinates are always
pixels of the most recent screenshot; the server maps them to device pixels,
points, or touch coordinates.

### Tools

| Tool | Purpose |
| --- | --- |
| `computer_info` | Target, display size, mode, permission diagnostics |
| `computer_screenshot` | Capture; `grid: true` overlays labeled 100 px lines |
| `computer_zoom` | Re-capture a region at full detail for small text/controls |
| `computer_click` / `computer_move` / `computer_drag` / `computer_scroll` | Pointer/touch actions; each returns a fresh screenshot |
| `computer_type` / `computer_key` | Text and shortcuts (`ctrl+s`, `cmd+shift+t`, `back`, `apphome`) |
| `computer_wait` | Sleep then screenshot |
| `computer_ui_tree` | Interactive elements with frame-pixel centers (Android, HarmonyOS) |
| `computer_app` | Launch / list / current app |
| `computer_devices` | Attached adb/hdc devices |
| `computer_apps` | Running apps with pid, bundle id, windows, and consent state |
| `computer_app_state` | One app window: image + indexed element tree |
| `computer_element` | Act on an element by index (press, set_value, menu, scroll, select_text) |
| `computer_raise` | Bring an app's window forward — the visible fallback |

### Why images are small

DeepSeek V4 Flash Vision caps every image at 384 input tokens and normalizes
larger images to roughly 800×800. The server therefore downscales captures to
a longest edge of 1024 px by default (`max_edge`), returns a screenshot after
every action (a whole step costs one round trip), and offers `computer_zoom`
so the model can read what the downscale blurred.

## Background / element control

Whole-screen control drives whatever is in front and moves the real cursor.
On macOS the server can also address **one app at a time** through its
accessibility tree, and deliver input to that app **in the background**: the
cursor does not move, the window is not raised, and a covered window still
captures and still responds. That is the difference between "the agent is
using my computer" and "the agent is using my computer while I use it".

### The loop

```text
computer_apps                                  # what is running, and may I touch it?
computer_app_state  {app: "Notes"}             # window image + indexed elements
computer_element    {app: "Notes", action: "press", index: 18}
computer_app_state  {app: "Notes"}             # confirm
```

`computer_app_state` returns the window as a PNG (scaled to `max_edge`) plus
one line per interactive element:

```text
#12 [AXTextField] "Search" at (240,64 320x28) press,set_value
#18 [AXButton] "New Note" at (36,96 120x32) press
```

Frames are pixels of the returned image. `mode` selects `full` (default),
`image`, or `ax` (tree only — cheapest, and the text is exact rather than
read off pixels); `filter` narrows a large window; `window_id` picks one
window of several; `max_nodes` caps the list.

`computer_element` actions:

| action | how it is performed | receipt |
| --- | --- | --- |
| `press` | `AXPress`, falling back to a posted click at the element's centre | which element |
| `set_value` | `AXValue` write, then read back; Electron/Chromium fall back to focus + select-all + type | `verified: true/false` |
| `menu` | `AXShowMenu`, falling back to a posted right-click | which element |
| `scroll` | a posted wheel event, escalating to the page keys when the scroll bars show nothing moved | `moved: true/false` (`false` = end of content) |
| `select_text` | `kAXSelectedTextRangeAttribute` | character range |

### What actually reaches a background app

This matters more than it looks, and the answer was measured rather than
assumed: **posted keyboard events reach a background app; posted mouse events
do not drive native AppKit controls.** A `CGEventPostToPid` click on a
Calculator key does nothing, while the same screen point through the phase-1
HID tap works. So the driver does not rely on posted clicks where it can
avoid them:

| what you ask for | what the driver does | background result |
| --- | --- | --- |
| `press` / `set_value` / `select_text` by index | accessibility action or attribute write | works |
| `click` at app pixels | `AXUIElementCopyElementAtPosition` at that point, then `AXPress` | works on anything the app exposes |
| `type` / `key` | posted key events | works |
| `scroll` | posted wheel, then the page keys if the scroll bars did not move | works |
| `drag` | posted mouse events | **does not work** outside Chromium/Electron content |
| `menu` | `AXShowMenu` | the action is accepted; a background app may decline to present the menu, and a menu is its own window so it never appears in the app's window capture |

The practical rule the Skill states: **act by index**. Pixel clicks are the
fallback, drag is not available in the background, and anything that truly
needs the pointer wants `computer_raise` and the foreground tools.

### Web content is not the same as a native control

Chromium and Electron render their UI inside an `AXWebArea`, and there an
accessibility write and a keystroke are **not** equivalent — measured on a
Chromium page with an `<input>` and a `contenteditable`:

- writing `AXValue` into a plain `<input>` updates it *and* fires the page's
  `input` event;
- writing `AXValue` into a `contenteditable` updates the content but fires
  **no** `input` event. The text is visibly there and reads back correctly, so
  the write reports `verified: true` — while a React-shaped composer (Slack,
  Discord, Notion) never learns of it and would send nothing;
- typing does fire real events in both, but Chromium drops key events while
  its window is inactive, so typing is only an option once the app is raised.

The driver cannot see whether a page's JavaScript reacted, and typing blindly
into a background browser risks putting text somewhere unintended. So it does
the targeted thing — the `AXValue` write — and, for any element inside a web
area, says plainly in the receipt that the app's own code may not have seen an
input event and that `computer_raise` plus typing is the fallback. `verified`
describes the *content*, never the app's state.

One caveat that is the app's choice rather than the driver's: whether an
inactive app acts on key events at all. TextEdit does — `cmd+shift+up` then
`z` edited a background document. Calculator does not: the same keystroke
does nothing until the app is activated, and then appears to arrive. So a
`type`/`key` receipt means "delivered", not "acted on"; confirm with a fresh
`computer_app_state` and use `computer_raise` for apps that defer input.

`computer_click`, `computer_type`, `computer_key`, `computer_scroll`, and
`computer_drag` each take an optional `app`. With it, x/y are pixels of that
app's last `computer_app_state` image and the input goes to that app in the
background; without it they keep their phase-1 whole-screen behaviour.
Element indexes and app-local coordinates are only valid for the snapshot
that produced them.

Apps that ignore input while covered are a real limitation, not a bug in the
server: `computer_raise` is the explicit, visible fallback, and the Skill
tells the model to try the background path first and to say when it had to
raise a window.

### Per-app consent

Every app-targeted call passes a server-side allowlist, on top of the plugin
trust review and the usual MCP tool approval:

```toml
[apps]
allow = ["com.apple.Notes", "Calculator"]
deny = ["Mail"]
```

Entries match a bundle id, an app name, or a process name, case-insensitively;
`deny` beats `allow`. An app on neither list returns

```text
needs_app_approval: `Notes` is not allowed for computer use yet.
Ask the user; if they agree, add this line to ~/.codewhale/computer-use.toml under [apps]:
  allow = ["com.apple.Notes"]
```

so the model has to come back to the user with the exact edit rather than
retrying or working around the gate. `codewhale computer-use setup` seeds the
empty block; nothing is ever granted automatically.

Four targets are excluded whatever the lists say, and cannot be configured:

- the terminal app hosting Codewhale (detected at startup by walking the
  parent-process chain, and matched by pid so renaming it changes nothing);
- Codewhale and the computer-use server themselves;
- `SecurityAgent`, `loginwindow`, and System Settings / System Preferences;
- secure text fields (`AXSecureTextField`) refuse `set_value` in any app.

### Permissions and platform support

The element lane needs both macOS permissions, for different reasons:

- **Accessibility** — reading the tree, `AXPress`/`AXValue` writes, and
  `CGEventPostToPid`. Without it every action fails with the grant
  instructions.
- **Screen Recording** — `CGWindowListCreateImage` for window capture *and*
  window titles in the app directory. Without it titles come back empty and
  captures fail.

Both are granted to the app that runs Codewhale (your terminal), not to
Codewhale itself. A signed helper app that owns the permissions is the right
long-term shape and is deliberately out of scope here; see
`docs/design/COMPUTER_USE_BACKGROUND.md` §3.4.

| Platform | Tree | Window image | Background actions |
| --- | --- | --- | --- |
| macOS | yes (AX) | yes, including covered windows | yes |
| Windows | no | yes (`PrintWindow`) | no — raise, then use the whole-screen tools |
| Android / HarmonyOS | yes (UI tree) | no (the display is the window; use `computer_screenshot`) | no — actions hit the foreground app |
| Linux | no | no | no |

`computer_info` reports the lane each target actually has, and the app tools
return an error naming the alternative when a platform lacks one.

## Configuration

`~/.codewhale/computer-use.toml` (or `$CODEWHALE_HOME/computer-use.toml`):

```toml
target = "auto"                 # auto | desktop | android | harmony
mode = "act"                    # act | observe (screenshots only)
max_edge = 1024                 # 256..2048
grid = false                    # overlay the coordinate grid by default
screenshot_after_action = true
settle_ms_desktop = 300
settle_ms_device = 700

[android]
serial = ""                     # adb -s; empty = the only connected device
adb = ""                        # explicit path when not on PATH

[harmony]
target = ""                     # hdc -t key; empty = the only connected target
hdc = ""

[linux]
display = ":0"                  # X11 display when the environment has none

[apps]                          # per-app consent for the element/background tools
allow = []                      # bundle id, app name, or process name
deny = []                       # deny beats allow
```

`auto` picks Android when the build runs on Android or a serial is set,
HarmonyOS when the build targets OpenHarmony or an hdc target is set, and the
local desktop otherwise. Plugin children receive a scrubbed environment
(PATH and HOME survive; `DISPLAY`, `ANDROID_SERIAL`, SDK variables do not), so
the file is the reliable place to select a device.

`codewhale computer-use doctor` prints the same diagnostics the model gets from
`computer_info`; `codewhale computer-use screenshot --grid --out shot.png` and
`codewhale computer-use call computer_key '{"keys":"ctrl+s"}'` exercise a
single tool without a model.

## Safety

- Activation requires the hash-bound trust review; MCP tool approval remains
  in front of every action. App-targeted calls pass a third, durable gate:
  the per-app allowlist above, with hard exclusions the user cannot override.
- `mode = "observe"` disables all input tools while keeping screenshots.
- Server-side bounds: coordinates must fall inside the current frame,
  `computer_type` ≤ 4000 characters, `computer_wait` ≤ 15 s, drags ≤ 5 s,
  scroll ≤ 50 notches, one image per tool result.
- Screenshots leave the machine: they are sent to the model provider as part
  of the turn. The Skill instructs the model to stop before credentials,
  payments, permission prompts, and destructive confirmations.
- Nothing is persisted by the server; temporary capture files are deleted
  after use.

## Platform notes

**macOS.** Grant Screen Recording and Accessibility to the terminal app that
runs Codewhale (System Settings → Privacy & Security). `computer_info` reports
both states. Whole-screen input is posted in display points; the driver
converts from the capture's backing pixels. App-targeted input goes to the
process with `CGEventPostToPid`, which never moves the cursor or raises the
window. Those events carry **global** display coordinates even though they are
aimed at one process — the driver maps window-image pixels → window-local
points → global points before posting. Posting window-local coordinates
silently misses, which is the kind of failure that looks like "the app ignored
us".

**Windows.** Input to an elevated window is blocked unless Codewhale runs
elevated. Only the primary monitor is captured. `computer_apps`,
`computer_app_state`, and `computer_raise` work (window directory and
`PrintWindow` capture); `computer_element` and the `app` argument do not —
UI Automation would give the tree, but cursor-free input is not reliable
there, so Windows stays foreground-only for actions.

**Linux / HarmonyOS PC.** X11 sessions need `xdotool` plus a screenshot tool;
Wayland sessions need `grim` (or `spectacle`/`gnome-screenshot`) and `ydotool`
with `ydotoold` running (`wtype` improves text entry). When the plugin child
has no `DISPLAY`, the driver uses `[linux].display` or `:0` and looks for a
`wayland-*` socket under the runtime directory.

**Android.** Enable USB or wireless debugging. Several devices: set
`[android].serial`. `input text` is ASCII-only; double-tap is two taps;
right-click is a long press. On the phone itself (Termux) run
`adb connect localhost:<port>` after pairing, then set `target = "android"`.

**HarmonyOS / OpenHarmony.** Requires `hdc` and a device with `uitest`
(HarmonyOS 4+/OpenHarmony 5+). Text entry tries `uiInput text` and falls back
to `uiInput inputText` at the last tapped point. Bundles launch with
`aa start -b <bundle> -a EntryAbility`; pass `bundle/Ability` to override.

## Verification status

- Unit and MCP-protocol tests run on every host (`cargo test -p codewhale-computer-use`,
  70 tests) and cover the consent verdicts, the element-tree rendering and
  index cache, the BGRA→RGBA window-image conversion, bundle-id resolution,
  the device UI-tree mapping, and the `[apps]` block `setup` seeds.
- The macOS whole-screen path was exercised live: real capture at 5760×3240,
  pointer events, zoom, and a full stdio MCP session.
- The macOS element/background path was exercised live on 2026-08-24 with
  Accessibility and Screen Recording granted, over a real stdio MCP session:
  - `computer_apps` listed running apps with bundle ids resolved from their
    `.app` bundles, and reported the host terminal as `excluded`;
  - an unlisted app returned the `needs_app_approval` error naming the exact
    `allow = [...]` line, and the same call succeeded once that line was added;
  - two `computer_element press` calls on Calculator's `#13 [AXButton] "7"`
    left `77` on its display **while Finder stayed frontmost** — no cursor
    movement and no window raise;
  - `set_value` into TextEdit's `AXTextArea` returned `verified: true` and the
    window capture showed the text, followed by `select_text 0..10` (also
    `verified: true`) with the selection visible — all with Terminal frontmost;
  - window capture returned correct colour and geometry (a 1200×800-point
    window as a 1024×683 PNG, element frames scaled to those pixels).
- Chromium/Electron apps expose only their window chrome until
  `AXManualAccessibility` is set; the driver sets it and waits once per app,
  which took one such app from 13 nodes to 104 including its web content.
- Background scrolling was exercised live in TextEdit: three pages down moved
  the view from line 000 to line 078 with `moved: true`, scrolling back up
  reported `moved: true` and then `moved: false` on hitting the top — the
  end-of-content signal is real, not assumed.
- Three defects were found and fixed by this live pass, each of which had
  looked correct in code review:
  - posted events were carrying window-local coordinates, so every pixel
    click and wheel silently missed; they carry global display coordinates;
  - posted mouse events do not drive native AppKit controls at all, so a
    pixel click now hit-tests with `AXUIElementCopyElementAtPosition` and
    presses the element it finds (verified against Calculator's keypad);
  - an `image`-mode `app_state` was dropping the accessibility binding, which
    silently disabled hit-testing and movement detection for the *next*
    action; image captures now keep the window bound.
- Measured and documented rather than worked around: background **drag** does
  nothing on native AppKit views, and `AXShowMenu` is accepted but a menu was
  never observed to appear for a background app.
- Keyboard delivery is app-dependent: virtual key codes and modifiers reach a
  background TextEdit (`cmd+shift+up`, then `z`, edited the document), while
  Calculator ignores the same keystroke until it is activated. The driver
  behaves identically in both cases, so a delivery receipt is not proof the
  app acted — re-state to confirm.
- Covered-window operation was exercised live: with a TextEdit window sized
  to fully contain Calculator and brought to the front, `computer_app_state`
  reported `occluded: true` and returned **the Calculator's own pixels**, not
  the window on top of it; `computer_element press` on a keypad button then
  changed the covered window's display. Capture and action both work through
  full occlusion.
- Chromium/Electron `set_value` was exercised live against a local page in an
  isolated Chrome profile: the `<input>` write verified and fired the page's
  `input` event; the `contenteditable` write verified but fired none, and
  typing into it worked only with the window raised. That measurement is why
  web-content writes now carry an explicit caveat in the receipt.
- A further false-negative was found and fixed here: `set_value` verification
  read back through the snapshot's element reference with no delay, so a
  Chromium write that had in fact landed reported `verified: false`.
  Verification now re-resolves the element by hit-test and allows a short
  settling window, and returns "unknown" rather than "failed" when it cannot
  read at all.
- Windows, Linux, Android, and HarmonyOS drivers are compile-checked for
  their targets (`x86_64-pc-windows-msvc`, `aarch64-unknown-linux-musl`,
  `aarch64-linux-android`, `aarch64-unknown-linux-ohos`) but have not been
  run against real devices in this repository; treat them as preview until a
  device QA pass is recorded.
