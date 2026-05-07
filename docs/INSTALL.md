# Installing DeepSeek TUI

This page covers every supported install path and the most common
"it didn't install" failures, including **Linux ARM64** and other less
common platforms.

If you just want the short version, see the
[main README](../README.md#quickstart) or
[简体中文 README](../README.zh-CN.md#快速开始).

---

## 1. Supported platforms

`deepseek-tui` ships prebuilt binaries for these
platform/architecture combinations from v0.8.8 onward:

| Platform     | Architecture | npm install | `cargo install` | GitHub release asset                                  |
| ------------ | ------------ | :---------: | :-------------: | ----------------------------------------------------- |
| Linux        | x64 (x86_64) |     ✅      |       ✅        | `deepseek-linux-x64`, `deepseek-tui-linux-x64`        |
| Linux        | arm64        |     ✅      |       ✅        | `deepseek-linux-arm64`, `deepseek-tui-linux-arm64`    |
| macOS        | x64          |     ✅      |       ✅        | `deepseek-macos-x64`, `deepseek-tui-macos-x64`        |
| macOS        | arm64 (M-series) | ✅      |       ✅        | `deepseek-macos-arm64`, `deepseek-tui-macos-arm64`    |
| Windows      | x64          |     ✅      |       ✅        | `deepseek-windows-x64.exe`, `deepseek-tui-windows-x64.exe` |
| Other Linux (musl, riscv64, …) | — |   ❌¹    |       ✅²       | build from source                                     |
| FreeBSD / OpenBSD              | — |   ❌      |       ✅²       | build from source                                     |

¹ The npm package will exit with a clear error and point you here.
² Provided your toolchain can compile a recent Rust workspace; see
  [Build from source](#5-build-from-source) below.

> **Linux ARM64 note (v0.8.7 and earlier).** v0.8.7 and earlier do **not**
> publish a Linux ARM64 prebuilt; users on HarmonyOS thin-and-light, Asahi
> Linux, Raspberry Pi, AWS Graviton, etc. saw `Unsupported architecture: arm64`
> from `npm i -g deepseek-tui`. v0.8.8 publishes both `deepseek-linux-arm64`
> and `deepseek-tui-linux-arm64`, so a plain `npm i -g deepseek-tui` works
> on any glibc-based ARM64 Linux. If you're stuck on v0.8.7, jump to
> [Build from source](#5-build-from-source) — `cargo install` works fine.

---

## 2. Install via npm (recommended)

```bash
npm install -g deepseek-tui
deepseek
```

`postinstall` downloads the right pair of binaries from the matching GitHub
release, verifies a SHA-256 manifest, and exposes both `deepseek` and
`deepseek-tui` on your `PATH`.

Useful environment variables:

| Variable                            | Purpose                                                                                |
| ----------------------------------- | -------------------------------------------------------------------------------------- |
| `DEEPSEEK_TUI_VERSION`              | Pin which release the wrapper downloads (defaults to `deepseekBinaryVersion`)          |
| `DEEPSEEK_TUI_GITHUB_REPO`          | Point the downloader at a fork (`owner/repo`)                                          |
| `DEEPSEEK_TUI_RELEASE_BASE_URL`     | Override the download root (e.g. an internal mirror or release-asset proxy)            |
| `DEEPSEEK_TUI_FORCE_DOWNLOAD=1`     | Re-download even if a cached binary marker matches                                     |
| `DEEPSEEK_TUI_DISABLE_INSTALL=1`    | Skip the `postinstall` download entirely (CI smoke, vendored binaries)                 |
| `DEEPSEEK_TUI_OPTIONAL_INSTALL=1`   | Don't fail `npm install` on download/extract errors — useful in CI matrices            |

> **Slow npm download from mainland China?** If `npm install` itself is slow
> (not just the postinstall binary download), use an npm registry mirror:
> ```bash
> npm config set registry https://registry.npmmirror.com
> npm install -g deepseek-tui
> ```
> See also [Section 3](#3-install-via-cargo-any-tier-1-rust-target) if you
> prefer Cargo over npm.

---

## 3. Install via Cargo (any Tier-1 Rust target)

If GitHub releases are slow, blocked, or you're on an unsupported architecture,
install from crates.io directly. Both crates are required — the dispatcher
delegates to the TUI runtime at runtime.

```bash
# Requires Rust 1.88+ (https://rustup.rs)
cargo install deepseek-tui-cli --locked   # provides `deepseek`
cargo install deepseek-tui     --locked   # provides `deepseek-tui`
deepseek --version
```

### China / mirror-friendly install

When installing from mainland China, configure mirrors for both **rustup**
(the Rust toolchain installer) and **Cargo** (the package registry) to avoid
TLS timeouts and download failures.

**Step 1: Install Rust via a rustup mirror**

```bash
# PowerShell
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
(New-Object Net.WebClient).DownloadFile('https://win.rustup.rs/x86_64', 'rustup-init.exe')

# git-bash / msys2
export RUSTUP_DIST_SERVER=https://mirrors.tuna.tsinghua.edu.cn/rustup
export RUSTUP_UPDATE_ROOT=https://mirrors.tuna.tsinghua.edu.cn/rustup/rustup
./rustup-init.exe -y --default-toolchain stable

# Linux / macOS
export RUSTUP_DIST_SERVER=https://mirrors.tuna.tsinghua.edu.cn/rustup
export RUSTUP_UPDATE_ROOT=https://mirrors.tuna.tsinghua.edu.cn/rustup/rustup
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
```

The `RUSTUP_DIST_SERVER` and `RUSTUP_UPDATE_ROOT` environment variables must
be set **before** running rustup-init; the toolchain download otherwise hits
the same TLS handshake problem as the installer.

**Step 2: Configure Cargo registry mirror**

```toml
# ~/.cargo/config.toml
[source.crates-io]
replace-with = "tuna"

[source.tuna]
registry = "sparse+https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/"
```

`rsproxy`, Tencent COS, and Aliyun OSS mirrors work the same way; pick whichever
is fastest from your network.

---

## 4. Manual download from GitHub Releases

Grab the matching pair of binaries for your platform from the
[Releases page](https://github.com/Hmbown/DeepSeek-TUI/releases) and drop them
side by side into a directory on your `PATH` (e.g. `~/.local/bin`):

```bash
# Linux ARM64 example
mkdir -p ~/.local/bin
curl -L -o ~/.local/bin/deepseek      \
    https://github.com/Hmbown/DeepSeek-TUI/releases/latest/download/deepseek-linux-arm64
curl -L -o ~/.local/bin/deepseek-tui  \
    https://github.com/Hmbown/DeepSeek-TUI/releases/latest/download/deepseek-tui-linux-arm64
chmod +x ~/.local/bin/deepseek ~/.local/bin/deepseek-tui
deepseek --version
```

Verify integrity against the per-release SHA-256 manifest:

```bash
curl -L -o /tmp/deepseek-artifacts-sha256.txt \
    https://github.com/Hmbown/DeepSeek-TUI/releases/latest/download/deepseek-artifacts-sha256.txt
( cd ~/.local/bin && sha256sum -c /tmp/deepseek-artifacts-sha256.txt --ignore-missing )
```

(Use `shasum -a 256 -c` instead of `sha256sum` on macOS.)

### Windows Scoop

DeepSeek TUI is listed in Scoop's main bucket:

```powershell
scoop update
scoop install deepseek-tui
deepseek --version
```

Scoop manifests are maintained outside this repository's release workflow and
can lag GitHub/npm/Cargo releases. Use npm or manual GitHub release downloads
when you need the newest version immediately.

---

## 5. Build from source

This is the catch-all for any platform we don't ship — including musl, riscv64,
LoongArch, FreeBSD, and pre-2024 ARM64 distros.

### Prerequisites

- **Rust** 1.88 or later — install with [rustup](https://rustup.rs).
- **Linux build-time deps** (Debian/Ubuntu/openEuler/Kylin):
  ```bash
  sudo apt-get install -y build-essential pkg-config libdbus-1-dev
  # openEuler / RHEL family:
  # sudo dnf install -y gcc make pkgconf-pkg-config dbus-devel
  ```
- A working `cmake` is **not** required.

### Build and install

```bash
git clone https://github.com/Hmbown/DeepSeek-TUI.git
cd DeepSeek-TUI

cargo install --path crates/cli --locked   # provides `deepseek`
cargo install --path crates/tui --locked   # provides `deepseek-tui`

deepseek --version
```

Both binaries land in `~/.cargo/bin/` by default; make sure that directory is
on your `PATH`.

### Cross-compiling from x64 to ARM64 Linux

If you want to build an ARM64 Linux binary on an x64 Linux host (e.g. for a
HarmonyOS / openEuler ARM64 thin-and-light), use
[`cross`](https://github.com/cross-rs/cross), which wraps the official Rust
cross-targets in a Docker container:

```bash
# Once
rustup target add aarch64-unknown-linux-gnu
cargo install cross --locked

# Per build
cross build --release --target aarch64-unknown-linux-gnu -p deepseek-tui-cli
cross build --release --target aarch64-unknown-linux-gnu -p deepseek-tui
```

The resulting binaries land in
`target/aarch64-unknown-linux-gnu/release/deepseek` and
`target/aarch64-unknown-linux-gnu/release/deepseek-tui`. Copy the matched pair
to the ARM64 host (e.g. via `scp`) and `chmod +x` them.

If you don't have Docker available, install the cross-linker directly and let
Cargo do the work:

```bash
sudo apt-get install -y gcc-aarch64-linux-gnu
rustup target add aarch64-unknown-linux-gnu

cat >> ~/.cargo/config.toml <<'EOF'
[target.aarch64-unknown-linux-gnu]
linker = "aarch64-linux-gnu-gcc"
EOF

cargo build --release --target aarch64-unknown-linux-gnu -p deepseek-tui-cli
cargo build --release --target aarch64-unknown-linux-gnu -p deepseek-tui
```

The same recipe works for `aarch64-unknown-linux-musl` if your distro is
musl-based.

### Windows build from source

Building on Windows requires the **MSVC C toolchain** from
[Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/#build-tools-for-visual-studio-2022)
(the free workload-selectable installer, not the full IDE).

**Prerequisites (Windows)**

1. Install Visual Studio 2022 Build Tools — select the **"Desktop development
   with C++"** workload.
2. Install [Rust](https://rustup.rs) 1.88+ (see the
   [China mirror instructions](#china--mirror-friendly-install) above if
   downloading from mainland China).
3. Install [Git for Windows](https://git-scm.com/download/win) (provides `git`
   and the `git-bash` terminal).

**Recommended terminals**: Windows Terminal, `git-bash`, or PowerShell.
`cmd.exe` works but has a small buffer and limited PATH behavior.

**Setting up the MSVC environment**

Visual Studio Build Tools install `cl.exe` to a versioned directory but do
**not** add it to `PATH` globally. You must set the environment manually or
use a Developer Command Prompt. The required variables are:

```powershell
# Adjust version numbers to match your installation
$msvc = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207"
$sdk   = "C:\Program Files (x86)\Windows Kits\10"
$sdkv  = "10.0.26100.0"

$env:INCLUDE  = "$msvc\include;$msvc\atlmfc\include;$sdk\Include\$sdkv\ucrt;$sdk\Include\$sdkv\um;$sdk\Include\$sdkv\shared"
$env:LIB      = "$msvc\lib\x64;$msvc\atlmfc\lib\x64;$sdk\Lib\$sdkv\ucrt\x64;$sdk\Lib\$sdkv\um\x64"
$env:LIBPATH  = "$msvc\lib\x64;$msvc\atlmfc\lib\x64"
$env:CC       = "$msvc\bin\Hostx64\x64\cl.exe"
$env:CXX      = "$msvc\bin\Hostx64\x64\cl.exe"
$env:PATH     = "$msvc\bin\Hostx64\x64;$env:PATH"
```

Alternatively, open a **"Developer Command Prompt for VS 2022"** (available
from the Start Menu after installing Build Tools), which runs `vcvars64.bat`
to configure all of the above automatically. Then add `cargo` to `PATH` inside
that session and run `cargo build` from the project root.

**Cargo registry mirror** — on Windows the mirror config goes to
`%USERPROFILE%\.cargo\config.toml`. See [Step 2 above](#china--mirror-friendly-install).

**Build**

```bash
git clone https://github.com/Hmbown/DeepSeek-TUI.git
cd DeepSeek-TUI
set CARGO_HTTP_CHECK_REVOKE=false   # may be needed behind some Chinese ISPs
cargo build --release
```

Both binaries appear in `target\release\deepseek.exe` and
`target\release\deepseek-tui.exe`.

> **Prefer `npm install -g` on Windows unless you need to modify source.**
> The npm package pulls prebuilt binaries and avoids the C toolchain
> dependency entirely — see [Section 2](#2-install-via-npm-recommended).

---

## 6. Troubleshooting

### `Unsupported architecture: arm64 on platform linux`

You're on a release earlier than v0.8.8 that doesn't publish Linux ARM64
binaries. Either upgrade (`npm i -g deepseek-tui@latest`) or use
`cargo install` per [Section 3](#3-install-via-cargo-any-tier-1-rust-target).

### `MISSING_COMPANION_BINARY` at runtime

The dispatcher (`deepseek`) requires the TUI runtime (`deepseek-tui`) to be on
the same `PATH`. If you installed only one crate via `cargo install`, install
both:

```bash
cargo install deepseek-tui-cli --locked
cargo install deepseek-tui     --locked
```

### `deepseek update` reports `no asset found for platform deepseek-linux-aarch64`

This is [#503](https://github.com/Hmbown/DeepSeek-TUI/issues/503) in v0.8.7 —
the self-updater used Rust's `aarch64`/`x86_64` arch names instead of the
release artifact's `arm64`/`x64`. Workaround until v0.8.8:

```bash
npm i -g deepseek-tui@latest
# or
cargo install deepseek-tui-cli --locked
```

### npm download is slow or times out from mainland China

Set `DEEPSEEK_TUI_RELEASE_BASE_URL` to a mirrored release-asset directory
(rsproxy, TUNA, Tencent COS, Aliyun OSS), or skip npm entirely and use the
Cargo mirror setup in [Section 3](#3-install-via-cargo-any-tier-1-rust-target).

### Debian/Ubuntu: `error: linker 'cc' not found` while building

Install the C toolchain:

```bash
sudo apt-get install -y build-essential pkg-config libdbus-1-dev
```

### Wrapper installs but `deepseek` isn't found

`npm i -g` installs into `$(npm prefix -g)/bin`; make sure that directory is on
your shell's `PATH`. With nvm: `nvm use --lts && hash -r`.

### Windows: `TLS handshake eof` or `CRYPT_E_REVOCATION_OFFLINE` from `rustup-init`

The TLS handshake to `static.rust-lang.org` fails from behind the GFW or
certain Chinese ISPs. Set the rustup mirror environment variables **before**
running the installer:

```bash
# git-bash / msys2
export RUSTUP_DIST_SERVER=https://mirrors.tuna.tsinghua.edu.cn/rustup
export RUSTUP_UPDATE_ROOT=https://mirrors.tuna.tsinghua.edu.cn/rustup/rustup
./rustup-init.exe -y --default-toolchain stable
```

If you see `CRYPT_E_REVOCATION_OFFLINE` from Cargo after Rust is installed,
also set `CARGO_HTTP_CHECK_REVOKE=false` during `cargo build`.

### Windows: MSVC compiler (`cl.exe`) not found during `cargo build`

Visual Studio Build Tools do not add `cl.exe` to the global `PATH`. Either:

1. Open **"Developer Command Prompt for VS 2022"** from the Start Menu, add
   `%USERPROFILE%\.cargo\bin` to `PATH` in that window, and run `cargo build`
   from there; or
2. Set the MSVC environment variables manually — see the
   [Windows build from source](#windows-build-from-source) section for the
   PowerShell snippet.

Verify the compiler is reachable: `cl.exe /?` should print help text.

### Windows: `拒绝访问 (os error 5)` when Cargo executes build scripts

Third-party antivirus software (Huorong, 360, Kaspersky, etc.) may block
Cargo from executing freshly-compiled build-script binaries
(e.g. `libsqlite3-sys`, `aws-lc-sys`, `instability`). The error is
path-agnostic — moving `target-dir` does not help.

**Symptoms**: `could not execute process ... build-script-build (never executed)`

**Workarounds** (pick one):

1. **Add the project's `target/` directory to your AV exclusions list.**
2. **Close the antivirus software temporarily** during `cargo build`.
3. **Use `npm install -g deepseek-tui` instead** — the npm package ships
   prebuilt binaries and skips the Cargo build entirely
   ([Section 2](#2-install-via-npm-recommended)).
4. **Use `cargo install deepseek-tui-cli --locked`** from crates.io — this
   changes the binary path, which some AV tools treat differently.

To verify that the build-script binary itself is valid (not corrupted), locate
it under `target/debug/build/<crate>/build-script-build` and run it manually:

```bash
target/debug/build/libsqlite3-sys-*/build-script-build
# If this runs but panics with "NotPresent" (no C compiler), the binary is
# fine — the AV is blocking Cargo's process-spawning path specifically.
```

---

## 7. Verifying your install

```bash
deepseek --version
deepseek doctor       # checks API key, provider, runtime, and PATH integrity
deepseek doctor --json
```

`doctor` exits non-zero if it finds a problem and prints structured remediation
hints. Paste the JSON output into a GitHub issue if you need help.
