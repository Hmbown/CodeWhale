# Installing CodeWhale

This page covers every supported install path and the most common
"it didn't install" failures, including **Linux ARM64** and other less
common platforms.

If you just want the short version, see the
[main README](../README.md#install) or
[简体中文 README](../README.zh-CN.md#安装).

On macOS and Linux, the website installer is the shortest install/update path:

```bash
curl -fsSL https://codewhale.net/install.sh | sh
```

It downloads the matching `codewhale`, `codew`, and `codewhale-tui` release binaries,
verifies them against `codewhale-artifacts-sha256.txt`, installs to
`~/.local/bin` by default, and exposes the `codew` convenience command.

---

## 1. Supported platforms

CodeWhale ships matched `codewhale`, `codew`, and `codewhale-tui` prebuilt binaries for
these platform/architecture combinations. Linux ARM64 is available from
v0.8.8 onward. Linux RISC-V prebuilts are temporarily paused because the locked
`rquickjs-sys` dependency does not ship `riscv64gc-unknown-linux-gnu` bindings.

| Platform     | Architecture | npm install | `cargo install` | GitHub release asset                                  |
| ------------ | ------------ | :---------: | :-------------: | ----------------------------------------------------- |
| Linux        | x64 (x86_64) |     ✅      |       ✅        | `codewhale-linux-x64`, `codew-linux-x64`, `codewhale-tui-linux-x64`        |
| Linux        | arm64        |     ✅      |       ✅        | `codewhale-linux-arm64`, `codew-linux-arm64`, `codewhale-tui-linux-arm64`    |
| Android / Termux | arm64 (aarch64) | ❌¹ | ✅² | `codewhale-android-arm64.tar.gz` Termux archive when published |
| Linux        | riscv64      |     ❌¹     |       ❌³       | temporarily unsupported until upstream bindings land |
| macOS        | x64          |     ✅      |       ✅        | `codewhale-macos-x64`, `codew-macos-x64`, `codewhale-tui-macos-x64`        |
| macOS        | arm64 (M-series) | ✅      |       ✅        | `codewhale-macos-arm64`, `codew-macos-arm64`, `codewhale-tui-macos-arm64`    |
| Windows      | x64          |     ✅      |       ✅        | `codewhale-windows-x64.exe`, `codew-windows-x64.exe`, `codewhale-tui-windows-x64.exe` |
| Linux x64 on musl (Alpine) | ✅ (static) |    ✅      |       ✅        | static `codewhale-tui-linux-x64` (musl) asset           |
| Other Linux (musl non-x64, other arches) | — | ❌¹ | ✅² | build from source                                     |
| FreeBSD / OpenBSD              | — |   ❌      |       ✅²       | build from source                                     |

¹ The npm package will exit with a clear error and point you here.
² Provided your toolchain can compile a recent Rust workspace; see
  [Build from source](#7-build-from-source) below.
³ RISC-V source builds currently need upstream `rquickjs-sys` RISC-V bindings or
  a bindgen-enabled dependency build.

Android / Termux is not the same target as Linux arm64. Do not install the
GNU libc `codewhale-linux-arm64` archive in Termux; use the Termux-specific
Android archive when a release or release candidate publishes one, or build
from source inside Termux.

The Linux **x64** release assets have been **static (musl) builds** since v0.8.65.
They have no glibc dependency and run on any x86_64 Linux, including Ubuntu
22.04, Debian stable, RHEL/CentOS, and Alpine/musl. SQLite is bundled into the
binary through `rusqlite`, so no separate `libsqlite3` runtime package is needed.

The Linux **arm64** release assets are still GNU libc (glibc) builds. They
dynamically link normal Linux runtime libraries such as `libdbus-1` and `libc`,
and are built on Ubuntu 24.04, so they can require `GLIBC_2.39`.

### Linux glibc floor (arm64)

This floor applies only to the **GNU libc** arm64 asset. The static x64 (musl)
asset has no `GLIBC_*` symbols, so it passes the install preflight and runs on
older systems without error. In the current v0.8.67 release lane, the GNU asset
is built on Ubuntu 24.04 and can require `GLIBC_2.39`. Ubuntu 22.04 ships glibc
2.35, so those arm64 binaries fail with errors such as:

```text
version `GLIBC_2.39' not found
```

The npm wrapper, `codewhale update`, and the Unix archive installer preflight
Linux GNU binaries before installing them and point older systems to Cargo/source
builds. If you are on Ubuntu 22.04 arm64, Debian stable, RHEL/CentOS, or another
older GNU base for a non-x64 asset, use:

```bash
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
```

Future release engineering may add static (musl) arm64 assets so the glibc floor
goes away entirely; until then, x64 is static and arm64 users on older distros
should build from source.

> **Linux ARM64 note (v0.8.7 and earlier).** v0.8.7 and earlier do **not**
> publish a Linux ARM64 prebuilt; users on HarmonyOS thin-and-light, Asahi
> Linux, Raspberry Pi, AWS Graviton, etc. saw `Unsupported architecture: arm64`
> from `npm i -g codewhale`. v0.8.8 publishes both `codewhale-linux-arm64`
> and `codewhale-tui-linux-arm64`, so a plain `npm i -g codewhale` works
> on any glibc-based ARM64 Linux. If you're stuck on v0.8.7, jump to
> [Build from source](#7-build-from-source) — `cargo install` works fine.
> For HarmonyOS PC and OpenHarmony cross-build setup, see
> [HarmonyOS and OpenHarmony](HarmonyOS.md).

### Android / Termux arm64

Termux runs on Android's Bionic libc and uses `$PREFIX` as its Unix prefix, so
it needs a Termux-specific Android arm64 archive. The Linux arm64 release asset
is a GNU libc build for normal Linux distributions and should not be used on
Android.

Install the minimum archive/runtime tools first:

```bash
pkg update
pkg install -y ca-certificates curl tar gzip coreutils
```

When the release includes `codewhale-android-arm64.tar.gz`, install it with the
archive's bundled installer. Passing `PREFIX="$PREFIX"` matters: the installer
defaults to `~/.local`, while Termux users normally expect commands under
`$PREFIX/bin`.

```bash
cd "$HOME"
curl -L -O https://github.com/Hmbown/CodeWhale/releases/latest/download/codewhale-android-arm64.tar.gz
curl -L -O https://github.com/Hmbown/CodeWhale/releases/latest/download/codewhale-bundles-sha256.txt
sha256sum -c codewhale-bundles-sha256.txt --ignore-missing

tar xzf codewhale-android-arm64.tar.gz
cd codewhale-android-arm64
PREFIX="$PREFIX" ./install.sh
hash -r
```

If you are validating from source or building a release candidate locally,
install the build packages before running Cargo:

```bash
pkg install -y rust clang pkg-config make git
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
```

First-run setup is the same as other platforms. Prefer `codewhale auth set` or
provider environment variables; do not assume a desktop Secret Service keyring
exists inside Termux.

```bash
codewhale auth set --provider deepseek
codewhale auth status
codewhale doctor
```

Maintainers should use this repeatable smoke checklist for a Termux / Android
arm64 release candidate:

```bash
command -v codewhale codew codewhale-tui
test -x "$PREFIX/bin/codewhale"
test -x "$PREFIX/bin/codew"
test -x "$PREFIX/bin/codewhale-tui"

codewhale --version
codewhale doctor
codewhale exec --auto "run pwd"
codewhale-tui --no-alt-screen
```

Known limitations:

- Sandbox behavior must be verified on-device. Android kernels and Termux
  packaging may not expose the same Landlock, seccomp, or Bubblewrap behavior
  documented for desktop/server Linux.
- OS keyring behavior is best-effort. If Termux cannot provide a usable secret
  store, use `codewhale auth status` to confirm the actual source and fall back
  to provider env vars or config-backed auth.
- Terminal rendering varies by Android terminal app. Use
  `codewhale-tui --no-alt-screen` in smoke tests before trying the full-screen
  TUI.

---

## 2. Download safety and checksums

Official release binaries are published only from
`https://github.com/Hmbown/CodeWhale/releases` and the npm package named
`codewhale`. Do not install release assets from look-alike repositories,
archives, or search-result mirrors unless you deliberately trust that mirror.

Every GitHub release includes checksum manifests. Use
`codewhale-artifacts-sha256.txt` for bare binaries and
`codewhale-bundles-sha256.txt` for `.tar.gz` / `.zip` platform archives. If you
download binaries manually, verify them before running:

```bash
# Run from the directory containing the downloaded binaries.
curl -L -O https://github.com/Hmbown/CodeWhale/releases/latest/download/codewhale-artifacts-sha256.txt
sha256sum -c codewhale-artifacts-sha256.txt --ignore-missing
```

On macOS, use `shasum -a 256 -c codewhale-artifacts-sha256.txt` instead of
`sha256sum`.

If antivirus software flags an official release binary, treat it as unresolved
until the exact artifact is identified. Please include all of the following in
the GitHub issue:

- the release tag, for example `v0.8.36`
- the exact download URL
- the filename, for example `codewhale-linux-x64`
- the file SHA-256 from your machine
- the antivirus product name and detection name

That lets maintainers distinguish a false positive on an official artifact from
a download sourced from an impersonating repository or mirror.

---

## 3. Install via npm

npm is the recommended install path. The `codewhale` wrapper is published at
v0.8.67 (Node 18+; wrapper available for v0.8.56 and later).

```bash
npm install -g codewhale
codewhale --version   # 0.8.67
```

`postinstall` downloads the right pair of binaries from the matching GitHub
release, verifies a SHA-256 manifest, and exposes `codewhale`, `codew`, and
`codewhale-tui` on your `PATH`.

Useful environment variables:

| Variable                            | Purpose                                                                                |
| ----------------------------------- | -------------------------------------------------------------------------------------- |
| `CODEWHALE_VERSION`                 | Pin which release the wrapper downloads (canonical)                                    |
| `DEEPSEEK_TUI_VERSION`              | Legacy alias for `CODEWHALE_VERSION` (defaults to `codewhaleBinaryVersion`)            |
| `DEEPSEEK_TUI_GITHUB_REPO`          | Point the downloader at a fork (`owner/repo`)                                          |
| `DEEPSEEK_TUI_RELEASE_BASE_URL`     | Override the download root (e.g. an internal mirror or release-asset proxy)            |
| `DEEPSEEK_TUI_FORCE_DOWNLOAD=1`     | Re-download even if a cached binary marker matches                                     |
| `DEEPSEEK_TUI_DISABLE_INSTALL=1`    | Skip the `postinstall` download entirely (CI smoke, vendored binaries)                 |
| `DEEPSEEK_TUI_OPTIONAL_INSTALL=1`   | Don't fail `npm install` on download/extract errors — useful in CI matrices            |

> **Slow npm download from mainland China?** If `npm install` itself is slow
> (not just the postinstall binary download), use an npm registry mirror:
> ```bash
> npm config set registry https://registry.npmmirror.com
> npm install -g codewhale
> ```
> See also [Section 4](#4-install-via-cargo-any-tier-1-rust-target) if you
> prefer Cargo over npm.

---

## 4. Install via Cargo (any Tier-1 Rust target)

If GitHub releases are slow, blocked, or you're on an unsupported architecture,
install from crates.io directly. Both crates are required — the dispatcher
delegates to the TUI runtime at runtime.

```bash
# Requires Rust 1.88+ (https://rustup.rs)
cargo install codewhale-cli --locked   # provides `codewhale` and `codew`
cargo install codewhale-tui     --locked   # provides `codewhale-tui`
codewhale --version
```

> **Linux: install build-time dependencies first.** `cargo install` compiles
> from source, and on Linux the `codewhale-tui` crate links against
> `libdbus-1` (used by the D-Bus secret-service backend for credential
> storage). Install the required system packages before running `cargo install`:
>
> ```bash
> # Debian / Ubuntu
> sudo apt-get install -y build-essential pkg-config libdbus-1-dev
>
> # Fedora / RHEL
> sudo dnf install -y gcc make pkgconf-pkg-config dbus-devel
> ```
>
> If you use the npm wrapper or download GitHub Release binaries, these
> build-time packages are **not** required — the prebuilt binary only
> needs the runtime library (`libdbus-1`), which is already present on
> most desktop Linux installs.

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

If the TUNA mirror is slow from your network, `rsproxy.cn` is another
rustup mirror option for Linux/macOS:

```bash
export RUSTUP_DIST_SERVER=https://rsproxy.cn
export RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup
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

## 5. Install via Nix

**Try it**

If you already have Nix with flake support, run:

```sh
nix run github:Hmbown/CodeWhale
```

Nix builds `codewhale-tui` and then starts the `codewhale` dispatcher. Pass
arguments after `--`, for example:

```sh
nix run github:Hmbown/CodeWhale -- --help
```

### Flake

Add inputs to `flake.nix`:

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    codewhale-tui.url = "github:Hmbown/CodeWhale";
    codewhale-tui.inputs.nixpkgs.follows = "nixpkgs";
  };
}
```

Install into a NixOS module:

```nix
{
  outputs = { self, nixpkgs, codewhale-tui }:
  let
    # replace system "x86_64-linux" with your system
    system = "x86_64-linux";
  in
  {
    # change `yourhostname` to your actual hostname
    nixosConfigurations.yourhostname = nixpkgs.lib.nixosSystem {
      inherit system;
      modules = [
        # ...
        {
          environment.systemPackages = [ codewhale-tui.packages.${system}.default ];
        }
      ];
    };
  };
}
```

---

## Homebrew (legacy tap)

Homebrew currently ships only the legacy `deepseek-tui` tap, kept for
compatibility while the formula is renamed to `codewhale`. It installs the
same current-release binaries:

```bash
brew tap Hmbown/deepseek-tui
brew install deepseek-tui
```

Update with `brew upgrade deepseek-tui`. There is no `codewhale` formula yet;
once the rename lands, this section will switch to it.

---

## 6. Manual download from GitHub Releases

Each platform appears on the Releases page in **two forms** (this is intentional — see #3208):
the **bare binaries** (`codewhale-<platform>`, `codew-<platform>`, and
`codewhale-tui-<platform>`, no extension) and a **`.tar.gz` / `.zip` archive**
(`codewhale-<platform>.tar.gz`) that bundles the same commands plus an
`install.sh`. The npm wrapper and the in-app `codewhale update` download the
matched runtime binaries; the archive is the easiest manual install (see §5).
The steps below use the bare binaries directly.

Grab the matching command set for your platform from the
[Releases page](https://github.com/Hmbown/CodeWhale/releases) and drop them
side by side into a directory on your `PATH` (e.g. `~/.local/bin`):

```bash
# Linux ARM64 example
mkdir -p ~/.local/bin
curl -L -o ~/.local/bin/codewhale      \
    https://github.com/Hmbown/CodeWhale/releases/latest/download/codewhale-linux-arm64
curl -L -o ~/.local/bin/codew          \
    https://github.com/Hmbown/CodeWhale/releases/latest/download/codew-linux-arm64
curl -L -o ~/.local/bin/codewhale-tui  \
    https://github.com/Hmbown/CodeWhale/releases/latest/download/codewhale-tui-linux-arm64
chmod +x ~/.local/bin/codewhale ~/.local/bin/codew ~/.local/bin/codewhale-tui
codewhale --version
```

> **macOS Gatekeeper note.** If you downloaded the binaries with a browser,
> macOS may block them with "Apple cannot verify" warnings. Clear the quarantine
> attribute on all three binaries and retry:
> ```bash
> xattr -d com.apple.quarantine ~/.local/bin/codewhale ~/.local/bin/codew ~/.local/bin/codewhale-tui 2>/dev/null || true
> ```

Verify integrity against the per-release SHA-256 manifest:

```bash
curl -L -o /tmp/codewhale-artifacts-sha256.txt \
    https://github.com/Hmbown/CodeWhale/releases/latest/download/codewhale-artifacts-sha256.txt
( cd ~/.local/bin && sha256sum -c /tmp/codewhale-artifacts-sha256.txt --ignore-missing )
```

(Use `shasum -a 256 -c` instead of `sha256sum` on macOS.)

### Roll back to a previous release

If a new release is bad on your machine, install the last known-good version
explicitly. Replace `X.Y.Z` with the version you want to restore.

```bash
# npm wrapper, only for versions that were published to npm
npm install -g codewhale@X.Y.Z

# Cargo install path; both crates are required
cargo install codewhale-cli --version X.Y.Z --locked --force
cargo install codewhale-tui --version X.Y.Z --locked --force
```

For manual installs, download the matched binaries or the platform archive from the
exact release tag and verify the matching checksum manifest from that same tag:

```bash
# individual binaries
curl -L -o codewhale-artifacts-sha256.txt \
  https://github.com/Hmbown/CodeWhale/releases/download/vX.Y.Z/codewhale-artifacts-sha256.txt

# platform archives
curl -L -o codewhale-bundles-sha256.txt \
  https://github.com/Hmbown/CodeWhale/releases/download/vX.Y.Z/codewhale-bundles-sha256.txt
```

Inside a CodeWhale workspace, `/restore list [N]` lists side-git file snapshots
and `/restore <N>` restores files from the chosen snapshot. That workspace
rollback does not change your installed binary version and does not rewrite
conversation history.

### Windows Scoop

The `codewhale` package is listed in Scoop's main bucket:

```powershell
scoop update
scoop install codewhale
codewhale --version
```

Scoop manifests are maintained outside this repository's release workflow and
can lag GitHub/npm/Cargo releases. Use npm or manual GitHub release downloads
when you need the newest version immediately.

### Windows NSIS Installer

A standalone NSIS-based installer is available starting with v0.8.50 for
Windows users who prefer a traditional double-click setup (no npm, no Scoop, no
Cargo required).

**Download** `CodeWhaleSetup.exe` from the
[Releases page](https://github.com/Hmbown/CodeWhale/releases/latest).

**Install** by double-clicking the setup executable. The installer:

- Installs `codewhale.exe`, `codew.exe`, and `codewhale-tui.exe` side-by-side into
  `%LOCALAPPDATA%\Programs\CodeWhale\bin`
- Adds the install directory to the **current user** `PATH`
- Registers in Windows **Apps & Features** for easy uninstall

**Silent install** (for IT admins, SCCM, Intune):

```powershell
CodeWhaleSetup.exe /S
```

The installer is per-user and does not request elevation. Run silent installs in
the target user's context, or use a deployment tool that can run the installer
for each user profile that needs CodeWhale.

The release-built installer is currently unsigned and may trigger Windows
SmartScreen. Verify the SHA-256 checksum from `codewhale-artifacts-sha256.txt`
before deploying, and sign the installer in your internal deployment pipeline if
your environment requires signed application packages.

**Build the installer yourself** (requires [NSIS](https://nsis.sourceforge.io)):

```powershell
cd scripts\installer
# Place codewhale.exe and codewhale-tui.exe here, then:
makensis /DVERSION=<version> codewhale.nsi
```

**Manual fallback** — if the installer is blocked by group policy, see the
[CLASSROOM_INSTALL.md](CLASSROOM_INSTALL.md) guide for step-by-step PowerShell
commands.

> **Deploying to a classroom or lab?** See the full
> [Classroom Install Checklist](CLASSROOM_INSTALL.md) for silent install,
> API key provisioning, imaging notes, and troubleshooting.

---

## 7. Build from source

This is the catch-all for platforms we don't ship, including musl non-x64,
LoongArch, FreeBSD, and pre-2024 ARM64 distros. Linux RISC-V currently also
needs upstream `rquickjs-sys` RISC-V bindings or a bindgen-enabled dependency
build before source builds are expected to work.

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
git clone https://github.com/Hmbown/CodeWhale.git
cd CodeWhale

cargo install --path crates/cli --locked   # provides `codewhale` and `codew`
cargo install --path crates/tui --locked   # provides `codewhale-tui`

codewhale --version
```

The commands land in `~/.cargo/bin/` by default; make sure that directory is
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
cross build --release --target aarch64-unknown-linux-gnu -p codewhale-cli
cross build --release --target aarch64-unknown-linux-gnu -p codewhale-tui
```

The resulting binaries land in
`target/aarch64-unknown-linux-gnu/release/codewhale` and
`target/aarch64-unknown-linux-gnu/release/codewhale-tui`. Copy the matched pair
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

cargo build --release --target aarch64-unknown-linux-gnu -p codewhale-cli
cargo build --release --target aarch64-unknown-linux-gnu -p codewhale-tui
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
git clone https://github.com/Hmbown/CodeWhale.git
cd CodeWhale
set CARGO_HTTP_CHECK_REVOKE=false   # may be needed behind some Chinese ISPs
cargo build --release
```

The binaries appear in `target\release\codewhale.exe`,
`target\release\codew.exe`, and `target\release\codewhale-tui.exe`.

> Prefer not to build? Install via npm, Cargo, GitHub Releases, or the CNB
> mirror — see the sections above.

---

## 8. Troubleshooting

### `Unsupported architecture: arm64 on platform linux`

You're on a release earlier than v0.8.8 that doesn't publish Linux ARM64
binaries. Either upgrade (`npm i -g codewhale@latest`) or use
`cargo install` per [Section 4](#4-install-via-cargo-any-tier-1-rust-target).

### `MISSING_COMPANION_BINARY` at runtime

The dispatcher (`codewhale`) requires the TUI runtime (`codewhale-tui`) to be on
the same `PATH`. If you installed only one crate via `cargo install`, install
both:

```bash
cargo install codewhale-cli --locked
cargo install codewhale-tui     --locked
```

### `codewhale update` reports `no asset found for platform codewhale-linux-aarch64`

This is [#503](https://github.com/Hmbown/CodeWhale/issues/503) in v0.8.7 —
the self-updater used Rust's `aarch64`/`x86_64` arch names instead of the
release artifact's `arm64`/`x64`. Workaround until v0.8.8:

```bash
npm i -g codewhale@latest
# or
cargo install codewhale-cli --locked
```

### npm download is slow or times out from mainland China

Set `CODEWHALE_RELEASE_BASE_URL` to a mirrored release-asset directory
(rsproxy, TUNA, Tencent COS, Aliyun OSS), or skip npm entirely and use the
Cargo mirror setup in [Section 4](#4-install-via-cargo-any-tier-1-rust-target).
The legacy `DEEPSEEK_TUI_RELEASE_BASE_URL` name is still accepted.

### `codewhale update` is blocked by GitHub from mainland China

`codewhale update` normally contacts GitHub Releases for metadata and binary
assets. On networks where GitHub is blocked or unreliable, use the CNB source
mirror instead and install both binaries from the release tag:

To check the latest release without downloading or replacing binaries, run
`codewhale update --check`.

```bash
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag vX.Y.Z codewhale-cli --locked --force
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag vX.Y.Z codewhale-tui     --locked --force
```

If you operate a binary asset mirror, `codewhale update` can use it directly:

```bash
CODEWHALE_RELEASE_BASE_URL=https://your-mirror.example.com/CodeWhale/vX.Y.Z/ \
DEEPSEEK_TUI_VERSION=X.Y.Z \
codewhale update
```

The mirror directory must contain `codewhale-artifacts-sha256.txt` and the
platform binaries from the GitHub release. The legacy
`DEEPSEEK_TUI_RELEASE_BASE_URL` mirror variable remains supported as an alias.

### Debian/Ubuntu: `feature edition2024 is required` from `cargo install`

Some Debian/Ubuntu distro packages ship an older Cargo that cannot parse Rust
2024 crates. For example, Cargo 1.75.0 on Ubuntu 24.04 fails before building
with:

```text
feature `edition2024` is required
The package requires the Cargo feature called `edition2024`, but that feature
is not stabilized in this version of Cargo
```

Install current stable Rust through rustup, then rerun the two Cargo install
commands from [Section 4](#4-install-via-cargo-any-tier-1-rust-target). For
mainland China networks, this rsproxy-based sequence has been verified to work:

```bash
export RUSTUP_DIST_SERVER=https://rsproxy.cn
export RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
rustup default stable
cargo install codewhale-cli --locked
cargo install codewhale-tui     --locked
```

Afterward, `which cargo` should point to `~/.cargo/bin/cargo`, not
`/usr/bin/cargo`.

### Debian/Ubuntu: `error: linker 'cc' not found` while building

Install the C toolchain:

```bash
sudo apt-get install -y build-essential pkg-config libdbus-1-dev
```

### WSL2 / Ubuntu: `dbus-1` or `pkg-config` not found while building

WSL2 uses the same Linux source-build path as Ubuntu. If `cargo install
codewhale-tui --locked` fails while compiling the keyring or D-Bus secret
storage crates, install the Linux build dependencies inside the WSL distro,
then rerun both Cargo install commands:

```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libdbus-1-dev
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
```

The prebuilt npm/GitHub binaries do not need these build-time packages; they
only apply when WSL2 is compiling CodeWhale from source.

### Wrapper installs but `codewhale` isn't found

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
3. **Use the GitHub Release installer/archive instead** — the release assets
   ship prebuilt binaries and skip the Cargo build entirely
   ([Section 6](#6-manual-download-from-github-releases)).
4. **Use `cargo install codewhale-cli --locked`** from crates.io — this
   changes the binary path, which some AV tools treat differently.

To verify that the build-script binary itself is valid (not corrupted), locate
it under `target/debug/build/<crate>/build-script-build` and run it manually:

```bash
target/debug/build/libsqlite3-sys-*/build-script-build
# If this runs but panics with "NotPresent" (no C compiler), the binary is
# fine — the AV is blocking Cargo's process-spawning path specifically.
```

### npm binary download times out

If `codewhale` waits several seconds and prints `connect ETIMEDOUT` or
`EAI_AGAIN` while fetching from `github.com`, the npm wrapper installed
successfully but the prebuilt binary download from GitHub Releases is blocked
or unreliable on your network. This download is separate from the npm registry
package download.

Use one of these paths:

1. Set a proxy and retry:

   ```bash
   export HTTPS_PROXY=http://your-proxy:port
   codewhale
   ```

2. Mirror the release assets internally and set `DEEPSEEK_TUI_RELEASE_BASE_URL`:

   ```bash
   export DEEPSEEK_TUI_RELEASE_BASE_URL=https://your-mirror.example.com/CodeWhale/
   codewhale
   ```

   The directory must contain `codewhale-artifacts-sha256.txt` and the platform
   binaries from the GitHub release.

3. Install via Cargo, which builds locally and does not download GitHub release
   assets. See [Section 4](#4-install-via-cargo-any-tier-1-rust-target).

4. Download both `codewhale` and `codewhale-tui` manually from the
   [Releases page](https://github.com/Hmbown/CodeWhale/releases), place them
   in a directory on `PATH`, and make them executable. See
   [Section 6](#6-manual-download-from-github-releases).

---

## 9. Verifying your install

```bash
codewhale --version
codewhale doctor       # checks API key, provider, runtime, and PATH integrity
codewhale doctor --json
```

`doctor` exits non-zero if it finds a problem and prints structured remediation
hints. Paste the JSON output into a GitHub issue if you need help.
