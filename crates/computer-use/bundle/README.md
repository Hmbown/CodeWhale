# Codewhale computer-use plugin

Screenshots and input for a vision model (target: `deepseek-v4-flash-vision-exp`)
on macOS, Windows, Linux/HarmonyOS PC, and attached Android (`adb`) or
HarmonyOS (`hdc`) devices. Full guide: `docs/COMPUTER_USE.md`.

```text
/plugin install ./crates/computer-use/bundle
/plugin enable computer-use      # shows the review + exact trust command
/plugin trust computer-use <token>
/plugin enable computer-use
/model flash-vision
/computer open the calculator and add 2 and 2
```

The bundle launches `codewhale computer-use serve` over stdio; configure a
phone target in `~/.codewhale/computer-use.toml`:

```toml
target = "android"   # or "harmony"; default "auto" = this desktop
[android]
serial = "emulator-5554"
```

## Tools

Whole screen, every platform: `computer_info`, `computer_screenshot`,
`computer_zoom`, `computer_click`, `computer_move`, `computer_drag`,
`computer_scroll`, `computer_type`, `computer_key`, `computer_wait`,
`computer_ui_tree` (phones), `computer_app`, `computer_devices`.

One app at a time (0.2.0): `computer_apps` lists running apps with their
consent state, `computer_app_state` captures one window as an image plus an
indexed element tree, `computer_element` acts on an element by index
(`press`, `set_value` with read-back, `menu`, `scroll` with movement
detection, `select_text`), and `computer_raise` is the visible fallback.
`computer_click/type/key/scroll/drag` also take an optional `app` argument.

On macOS these run **in the background**: the accessibility tree and
`CGEventPostToPid` reach the app without moving the cursor or raising the
window, and a covered window still captures. Windows contributes the app
directory and window capture (reads only); Android and HarmonyOS map the
element actions onto their UI trees, in the foreground. Actions need
Accessibility and captures need Screen Recording.

Every app-targeted call is gated by a per-app allowlist, which
`codewhale computer-use setup` writes into the config:

```toml
[apps]
allow = ["com.apple.Notes"]
deny = []
```

An unlisted app returns a `needs_app_approval` error naming the exact line to
add. The terminal hosting Codewhale, Codewhale itself, and
security/login/System-Settings surfaces can never be added.
