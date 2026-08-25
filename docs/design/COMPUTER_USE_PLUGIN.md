# Computer-use plugin design

Status: design accepted for implementation (2026-08-24). Target model:
`deepseek-v4-flash-vision-exp`. Targets: macOS, Windows, Linux (incl.
HarmonyOS PC), Android, HarmonyOS/OpenHarmony devices.

## 1. Goal

Give a Codewhale session a screenshot → reason → act loop over a real screen,
driven primarily by DeepSeek V4 Flash Vision (experimental), through the
existing plugin-bundle boundary. Nothing in the core turn loop, prompt prefix,
or tool registry changes; the plugin contributes an MCP server, a Skill, an
Agent profile, and a command through the engines that already exist
(`docs/PLUGIN_BUNDLES.md`).

### Non-goals

- No background/automatic screen watching. Every capture is an explicit tool
  call inside a turn (consistent with `docs/READ_MEDIA.md`).
- No accessibility-tree automation on desktop in v1 (Android/HarmonyOS get a
  UI tree because their tooling exposes one cheaply).
- No new provider work: the vision route already exists
  (`crates/config/src/route/offering.rs`, `image_input: Supported`).
- No marketplace/auto-trust changes; the bundle installs and activates through
  the normal review.

## 2. Model constraints that shape the design

`deepseek-v4-flash-vision-exp` (live 2026-08-21, DeepSeek API news) accepts
PNG/JPEG/GIF/WebP as base64 data URLs on the Chat Completions route, works
with tool calling, and **caps every image at 384 input tokens, normalizing
larger images to roughly 800×800**. Consequences:

1. Screenshots are cheap (≤384 tokens each), so every action tool returns a
   fresh screenshot by default: one round trip per step instead of two.
2. Fine detail is lost at that resolution, so the server offers `computer_zoom`
   (crop a region and send it at full budget) and an optional labeled
   coordinate grid on screenshots to help the model read positions.
3. Coordinates are always expressed in the pixel space of the image the model
   last saw (the "frame"); the server maps them to device coordinates. The
   model never needs to know about Retina scale factors or device resolution.

Codewhale-side constraints (`crates/tui/src/image_attach.rs`,
`crates/tui/src/tools/registry.rs`): one image per MCP tool result, PNG/JPEG/
GIF/WebP, ≤5 MiB, base64 stripped from the text projection. The MCP client is
hand-rolled and pins protocol `2024-11-05` (`crates/mcp/src/stdio_client.rs`,
`crates/tui/src/mcp.rs`).

## 3. Architecture

```
crates/computer-use/bundle/            ← plugin bundle (plugin.json, mcp.json, skills/, agents/, commands/)
        │  stdio MCP (2024-11-05)
        ▼
codewhale computer-use serve     ← thin subcommand in crates/cli, delegates to
        │                          crates/computer-use (lib + bin `codewhale-computer-use`)
        ▼
  ┌─ McpServer (JSON-RPC over stdio, no async runtime)
  │    tools/list, tools/call, initialize, ping
  ├─ Session state: last frame, last zoom, target driver
  ├─ Frame pipeline: decode → downscale (max_edge) → optional grid → PNG → base64
  └─ Driver trait  ──┬─ desktop::macos   (screencapture + CoreGraphics CGEvent FFI)
                     ├─ desktop::windows (GDI capture + SendInput via windows-sys)
                     ├─ desktop::linux   (grim/scrot/maim/import + xdotool/ydotool)
                     ├─ android          (adb: screencap, input, uiautomator, monkey)
                     └─ harmony          (hdc: uitest screenCap/uiInput/dumpLayout, aa/bm)
```

### 3.1 Crate `crates/computer-use` (package `codewhale-computer-use`)

- Library + binary. No tokio, no rmcp: a ~300-line line-delimited JSON-RPC
  loop matching the TUI client's expectations. Dependencies: `serde`,
  `serde_json`, `toml`, `base64`, `image` (png+jpeg only), `anyhow`; `windows-sys`
  on Windows only; raw `extern "C"` CoreGraphics on macOS (no crate). Nothing
  that breaks the Android/OpenHarmony cross-builds guarded by
  `scripts/release/check-ohos-deps.sh`.
- `Driver` trait (sync):
  `info() -> TargetInfo`, `screenshot() -> RawFrame`, `click`, `move_to`,
  `drag`, `scroll`, `type_text`, `key_combo`, `ui_tree() -> Option<UiTree>`,
  `apps(action)`. Every method returns `Result<_, DriverError>` with a
  user-actionable message (missing binary, missing permission, no device).
- Coordinate model. `Frame { shot_w, shot_h, dev_w, dev_h, logical_w, logical_h }`.
  Model coordinates `(x, y)` in shot space → device pixels
  `x * dev_w / shot_w` → driver-native units (macOS points = pixels /
  backing scale; Windows physical pixels with DPI awareness; adb/hdc physical
  pixels of the rotated display). A `Zoom { region_in_shot_space, zoom_w, zoom_h }`
  is kept beside the frame; actions accept `frame: "screen" | "zoom"`.
- Target selection: `--target auto|desktop|android|harmony` (CLI) or
  `~/.codewhale/computer-use.toml`; `auto` = android when compiled for Android
  or `ANDROID_SERIAL` is set, harmony when compiled for OpenHarmony or an hdc
  target is configured, otherwise desktop. Plugin children run with a
  scrubbed env (`crates/tui/src/child_env.rs`), so the server never depends on
  `DISPLAY`/`WAYLAND_DISPLAY`/SDK env vars being present: it probes
  well-known locations for `adb`/`hdc` and falls back to `DISPLAY=:0` /
  `$XDG_RUNTIME_DIR/wayland-*`. `${SOURCE_ENV}` mappings are not used in the
  bundle because an unset source is a hard spawn error.
- Modes: `act` (default) or `observe` (only screenshot/zoom/info/ui_tree/
  devices; actions return an error naming the mode). Operator-set only.

### 3.2 Tool surface (MCP)

| tool | args | returns |
|---|---|---|
| `computer_info` | — | target kind, driver, display size, scale, capabilities, permission diagnostics (text) |
| `computer_screenshot` | `grid?: bool`, `max_edge?: u32` | image + `Frame W×H` text; resets zoom |
| `computer_zoom` | `x,y,width,height` (frame space) | image of the region scaled to the budget + mapping text |
| `computer_click` | `x,y`, `button?: left\|right\|middle`, `clicks?: 1..3`, `frame?`, `hold_ms?` (long-press) | text + screenshot |
| `computer_move` | `x,y`, `frame?` | text + screenshot |
| `computer_drag` | `from_x,from_y,to_x,to_y`, `duration_ms?`, `frame?` | text + screenshot |
| `computer_scroll` | `x,y`, `direction: up\|down\|left\|right`, `amount?: 1..50`, `frame?` | text + screenshot |
| `computer_type` | `text` (≤4000 chars) | text + screenshot |
| `computer_key` | `keys` e.g. `"ctrl+shift+t"`, `"enter"`, `"back"` | text + screenshot |
| `computer_wait` | `ms` (≤15000) | text + screenshot |
| `computer_ui_tree` | `max_nodes?` | compact list of interactive elements with frame-space centers (Android/HarmonyOS); desktop returns not-supported |
| `computer_app` | `action: launch\|list\|current`, `name?` | text |
| `computer_devices` | — | connected adb/hdc devices and the selected one |

Action tools take a screenshot after a settle delay (`screenshot_after_action`,
default true, 300 ms desktop / 700 ms devices). Every result's text starts
with the frame line (`frame: 1024x640 (device 2560x1600, scale 2.5)`) so the
model always has the current coordinate space.

Key names are platform-neutral: `cmd`/`super`/`win`/`meta` → platform meta
key, `ctrl`, `alt`/`option`, `shift`, `enter`, `tab`, `esc`, `backspace`,
`delete`, arrows, `home`, `end`, `pageup`, `pagedown`, `space`, `f1..f12`,
plus device keys `back`, `home`, `recents`, `power`, `volume_up`,
`volume_down`. Unknown names fail with the accepted list.

### 3.3 Per-platform drivers

**macOS** — capture: `screencapture -x -C -t png <tmp>` (physical pixels;
needs Screen Recording for the terminal app). Display bounds/scale:
`CGDisplayBounds`, `CGDisplayPixelsWide`. Input: `CGEventCreateMouseEvent`,
`CGEventSetIntegerValueField(kCGMouseEventClickState)`,
`CGEventCreateScrollWheelEvent`, `CGEventCreateKeyboardEvent` +
`CGEventKeyboardSetUnicodeString`, `CGEventPost(kCGHIDEventTap)` (needs
Accessibility). Diagnostics: `CGPreflightScreenCaptureAccess`,
`AXIsProcessTrusted`. Apps: `open -a`, `osascript` for frontmost.

**Windows** — process is marked per-monitor DPI aware; capture primary monitor
via GDI `BitBlt` → `GetDIBits` → PNG; input via `SendInput` (absolute mouse,
`KEYEVENTF_UNICODE` typing, VK codes for combos); apps via `cmd /c start`.

**Linux / HarmonyOS PC** — process-based. Wayland: `grim` (or `spectacle`,
`gnome-screenshot`) + `ydotool`/`wtype`; X11: `scrot`/`maim`/`import` +
`xdotool`. `computer_info` lists which helpers were found and what is
missing. HarmonyOS PC with a glibc userspace uses this driver
(`docs/HarmonyOS.md` tiering).

**Android** — `adb [-s serial] exec-out screencap -p`; `input tap|swipe|
draganddrop|text|keyevent|keycombination`; long-press via zero-distance swipe;
`uiautomator dump /dev/tty` parsed by a small built-in XML reader into a
compact node list; `monkey -p <pkg>` to launch, `pm list packages`,
`dumpsys window` for the current app. Works from a host or from Termux on the
device itself via wireless debugging (`adb connect localhost:<port>`).

**HarmonyOS / OpenHarmony** — `hdc [-t key] shell uitest screenCap -p
/data/local/tmp/…` + `hdc file recv`; `uitest uiInput click|doubleClick|
longClick|swipe|drag|inputText|keyEvent`; `uitest dumpLayout` JSON for the
UI tree; `aa start -b <bundle> -a <ability>` to launch, `bm dump -a` to list.

### 3.4 Plugin bundle `crates/computer-use/bundle/`

```
plugin.json          name "computer-use"; extensions.net.codewhale: display_name, agents, commands,
                     when.os = [macos, linux, windows, android]
mcp.json             mcpServers.computer: stdio, command "codewhale", args ["computer-use","serve"],
                     execute_timeout 120
skills/computer-use/SKILL.md    the operating loop for the model
agents/computer-operator.toml   provider "deepseek", model "deepseek-v4-flash-vision-exp"
commands/computer.md            /computer <task> → loads the skill and delegates or drives
```

`when.os` cannot name `ohos` (`crates/tui/src/plugins/manifest.rs`), and
OpenHarmony builds report `target_os = "linux"`, so `linux` covers HarmonyOS
PC and OpenHarmony hosts. The MCP server name is exposed as
`plugin-12-computer-use-computer`; tools appear under that prefix.

### 3.5 Model routing

- Direct: `/model flash-vision` (alias for `deepseek-v4-flash-vision-exp`) and
  use the tools; `read_media`-style vision checks admit the route because the
  offering row states `image_input: Supported`.
- Delegated: from a text route (e.g. `deepseek-v4-pro`), the `/computer`
  command asks the session to spawn the `computer-operator` profile via the
  `agent` tool. Fleet refuses to reroute a vision-bound member to a non-vision
  route, which is the correct fail-closed behavior.

## 4. Safety

- Activation only through `/plugin trust` + `/plugin enable`; the review shows
  the stdio command and warns it runs with the user's host authority.
- MCP tool approval prompts remain in front of every action tool.
- Server-side bounds: coordinates must lie inside the frame; text ≤4000 chars;
  wait ≤15 s; drag ≤5 s; scroll ≤50 notches; one screenshot per result.
- `observe` mode for read-only sessions.
- Screenshots leave the machine: the Skill tells the model to stop and ask
  before entering credentials, payments, or destructive confirmations, and the
  docs state plainly that captured pixels are sent to the provider.
- No credentials are read or written by the server; it stores nothing beyond
  temp files that are deleted after use.

## 5. Testing and evidence

- Unit tests (host-independent): coordinate mapping incl. Retina/zoom,
  key-combo parsing, uiautomator XML → node list, hdc dumpLayout JSON → node
  list, frame downscale + grid, MCP handshake/`tools/list`/`tools/call` over an
  in-memory mock driver, config parsing, and bundle manifest validation via
  `PluginManifest::validate_from_path` in the TUI crate's plugin tests.
- Compile evidence across targets: `cargo check -p codewhale-computer-use
  --target {aarch64-apple-darwin, x86_64-pc-windows-msvc, aarch64-linux-android,
  aarch64-unknown-linux-ohos, aarch64-unknown-linux-musl}` (all installed).
- Live smoke on this macOS host: `codewhale-computer-use doctor`,
  `screenshot`, `call computer_move`, and a real MCP handshake via stdin.
- Not verifiable here: Windows/Linux/Android/HarmonyOS device runs. Those are
  reported as untested in the docs rather than implied.

## 6. Files touched

- New: `crates/computer-use/**`, `crates/computer-use/bundle/**`,
  `docs/COMPUTER_USE.md`, this design doc.
- Modified: root `Cargo.toml` (workspace member), `crates/cli/Cargo.toml` +
  `crates/cli/src/lib.rs` (`computer-use` subcommand delegating to the lib,
  handled before runtime/store resolution), `docs/PLUGIN_BUNDLES.md` (one
  paragraph pointing at the bundle), `CHANGELOG.md`.
