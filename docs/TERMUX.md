# Termux / Android arm64 Support

CodeWhale runs natively on Android arm64 via [Termux](https://termux.dev).
This document covers the install path and the platform-specific behavior
differences you should know about.

## Installation

See [`INSTALL.md`](./INSTALL.md) → "Android / Termux arm64" for the current
install steps. The short version:

```sh
# Inside Termux (pkg install rust git ...)
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
```

Or, when a release includes `codewhale-android-arm64.tar.gz`, extract it
into `$PREFIX/bin`.

> **Do not** install the GNU libc `codewhale-linux-arm64` archive in Termux.
> Android uses Bionic libc, not glibc — the Linux binary will not run.

## Platform behavior on Android

CodeWhale's security model has two independent layers:

1. **OS filesystem sandbox** — Seatbelt (macOS), Landlock (Linux), or
   nothing. This layer restricts what *shell commands* can access at the
   kernel level.
2. **CodeWhale's own gates** — workspace trust, approval prompts,
   `allow_shell`/`disallowed-tools`, and the file-tool permission system.
   These are application-level and work identically on every platform.

### Sandbox: unavailable (type = none)

Android does not expose Landlock, Seatbelt, or any equivalent mandatory
access control API that CodeWhale can use. On Android,
`codewhale doctor` reports **sandbox type: none**.

- `get_platform_sandbox()` returns `None` on Android.
- No Linux-only sandbox modules (Landlock, bwrap) are compiled into the
  Android build — they are `#[cfg(target_os = "linux")]`-gated and Rust
  treats `android` as a distinct target from `linux`.
- Shell commands run without OS-level filesystem containment. Rely on
  CodeWhale's approval gates and workspace trust for safety.

### Approvals: still apply

CodeWhale's approval system (interactive prompts for risky actions,
`allow_shell`, `--disallowed-tools`) is entirely application-level. It works
identically on Android — the absence of an OS sandbox does not weaken it.

### Secret storage: file-backed

Android has no OS keyring (no Secret Service / dbus). CodeWhale falls back
to **file-backed secret storage**: encrypted-at-rest JSON files under
`~/.codewhale/secrets/` (Termux home directory), with `0600` permissions.

- API keys set via `codewhale setup` or `/provider` are stored in these
  files, never in plaintext beyond the file the user chose.
- `codewhale doctor` reports which secret backend is active.

### Self-update

`codewhale update` on Android requests `codewhale-android-arm64` and
`codewhale-tui-android-arm64` release assets — never the Linux arm64
assets. The GNU libc (glibc) compatibility preflight is Linux-only and is
skipped entirely on Android (Bionic libc).

## Known limitations (first Termux release)

| Feature | Status | Notes |
|---------|--------|-------|
| OS sandbox | ❌ unavailable | No Landlock/bwrap/Seatbelt on Android |
| OS keyring | ❌ unavailable | Falls back to file-backed secrets |
| Approvals / gates | ✅ full | Application-level, platform-independent |
| File tools | ✅ full | Governed by workspace trust |
| Self-update | ✅ full | Selects Android assets |
| Shell execution | ⚠️ no containment | Runs without OS sandbox; rely on approvals |

## Related issues

- #4236 — Epic: official Termux / Android arm64 support
- #4238 — Make Android sandbox and secret-store behavior explicit
- #4240 — Build and bundle Android arm64 release assets
- #4241 — Teach updater to select Android assets on Termux
- #4242 — Run Termux runtime QA
