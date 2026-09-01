# Installing Codewhale

> 阅读简体中文版：[zh_hans/INSTALL.md](zh_hans/INSTALL.md)

This page covers every supported install path and the most common
"it didn't install" failures, including **Linux ARM64** and other less
common platforms.

If you just want the short version, see the
[main README](../README.md#install) or
[简体中文 README](../README.zh-CN.md#安装).

This branch describes the **v0.9.11 source candidate**. Install commands that use
`latest` resolve to the latest published package or GitHub Release, which may
trail the source candidate. A candidate is not a published install until the
matching package, tag, checksums, and release assets exist.

On macOS and Linux, the website installer is the shortest install/update path:

```bash
curl -fsSL https://codewhale.net/install.sh | sh
```

It downloads the matching `codewhale` and `codew` release binaries,
verifies them against `codewhale-artifacts-sha256.txt`, installs to
`~/.local/bin` by default, and exposes the `codew` convenience command.

---

## 1. Supported platforms

Published Codewhale releases ship matched `codewhale` and `codew` prebuilt binaries for their supported platform/architecture
combinations. The table below is the intended v0.9.11 candidate matrix;
Android/Termux is preview pending real-device QA. Linux ARM64 is available from
v0.8.8 onward. Linux RISC-V prebuilts are temporarily paused because the locked
`rquickjs-sys` dependency does not ship `riscv64gc-unknown-linux-gnu` bindings.

| Platform     | Architecture | npm install | `cargo install` | GitHub release asset                                  |
| ------------ | ------------ | :---------: | :-------------: | ----------------------------------------------------- |
| Linux        | x64 (x86_64) |     ✅      |       ✅        | `codewhale-linux-x64`, `codew-linux-x64`        |
| Linux        | arm64        |     ✅      |       ✅        | `codewhale-linux-arm64`, `codew-linux-arm64`    |
| Android / Termux | arm64 (aarch64) | ⚠️⁴ preview | ⚠️⁴ preview | `codewhale-android-arm64.tar.gz` preview archive when published |
| Linux        | riscv64      |     ❌¹     |       ❌³       | temporarily unsupported until upstream bindings land |
| macOS        | x64          |     ✅      |       ✅        | `codewhale-macos-x64`, `codew-macos-x64`        |
| macOS        | arm64 (M-series) | ✅      |       ✅        | `codewhale-macos-arm64`, `codew-macos-arm64`    |
| Windows      | x64          |     ✅      |       ✅        | `codewhale-windows-x64.exe`, `codew-windows-x64.exe` |
| Windows      | arm64        |     ✅      |       ✅        | `codewhale-windows-arm64.exe`, `codew-windows-arm64.exe` |
| Linux x64 or arm64 on musl (Alpine) | native arch | ✅ (static) | ✅ | matching static Linux asset |
| Other Linux (musl on other arches) | — | ❌¹ | ✅² | build from source                                     |
| FreeBSD 14+ / OpenBSD          | x64, arm64 |   ❌      |       ✅²       | `cargo install codewhale-cli --locked` (no prebuilt; see § FreeBSD) |

¹ The npm package will exit with a clear error and point you here.
² Provided your toolchain can compile a recent Rust workspace; see
  [Build from source](#7-build-from-source) below.
³ RISC-V source builds currently need upstream `rquickjs-sys` RISC-V bindings or
  a bindgen-enabled dependency build.
⁴ The v0.9.11 source-candidate npm wrapper recognizes Android arm64 and resolves
  the matching `codewhale` and `codew` Android assets. npm
  installation works only for a package version whose GitHub Release publishes
  those matching assets. The Android/Termux path remains preview-only until the
  real-device compile, startup, approval, file-tool, and update checks tracked
  in #4236 and #4242 are complete.

Android / Termux is not the same target as Linux arm64. Do not install the
Linux `codewhale-linux-arm64` archive in Termux; use the Termux-specific
Android archive when a release or release candidate publishes one, or build
from source inside Termux.

The Linux **x64 and arm64** v0.9.11 candidate assets are **static musl builds**.
The x64 release path has used musl since v0.8.65; v0.9.6 extends the same build
and static-launch check to arm64. These binaries have no glibc dependency and
run on their matching architecture across Ubuntu, Debian, RHEL/CentOS, and
Alpine/musl. SQLite is bundled through `rusqlite`, so no separate `libsqlite3`
runtime package is needed.

### Linux ARM64 portability

Linux arm64 assets before v0.9.6 were GNU libc builds and could inherit the
Ubuntu 24.04 build host's `GLIBC_2.39` floor. Ubuntu 22.04 ships glibc 2.35, so
those older arm64 binaries can fail with errors such as:

```text
version `GLIBC_2.39' not found
```

The npm wrapper, `codewhale update`, and the Unix archive installer retain their
GNU-binary preflight for older releases. The v0.9.11 arm64 candidate instead uses
`aarch64-unknown-linux-musl`, so it has no `GLIBC_*` floor. If you are installing
an earlier release on an older arm64 distribution, use:

```bash
cargo install codewhale-cli --locked   # installs `codewhale`
```

> **Linux ARM64 note (v0.8.7 and earlier).** v0.8.7 and earlier do **not**
> publish a Linux ARM64 prebuilt; users on HarmonyOS thin-and-light, Asahi
> Linux, Raspberry Pi, AWS Graviton, etc. saw `Unsupported architecture: arm64`
> from `npm i -g codewhale`. v0.8.8 publishes `codewhale-linux-arm64`, so a plain `npm i -g codewhale` works
> on any glibc-based ARM64 Linux. If you're stuck on v0.8.7, jump to
> [Build from source](#7-build-from-source) — `cargo install` works fine.
> For HarmonyOS PC and OpenHarmony cross-build setup, see
> [HarmonyOS and OpenHarmony](HarmonyOS.md).

### Android / Termux arm64

Termux runs on Android's Bionic libc and uses `$PREFIX` as its Unix prefix, so
it needs a Termux-specific Android arm64 archive. The Linux arm64 release asset
targets standard Linux with musl; Android uses a distinct Rust target, so the
Linux asset should not be used there.

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
cargo install codewhale-cli --locked   # installs `codewhale`
```

The normal first-run setup path is implemented, but its Android interaction is
still part of the preview QA above. Prefer provider environment variables for
temporary credentials. `codewhale auth set` is available, but the Termux build
has no supported OS keyring integration and falls back to file-backed secrets
by writing `~/.codewhale/config.toml` and mirroring keys to
`~/.codewhale/secrets/secrets.json`. Both are plaintext files protected by
`0600` permissions and are not encrypted at rest.

```bash
codewhale auth set --provider deepseek
codewhale auth status
codewhale doctor
```

Maintainers should use this repeatable smoke checklist for a Termux / Android
arm64 release candidate:

```bash
command -v codewhale codew
test -x "$PREFIX/bin/codewhale"
test -x "$PREFIX/bin/codew"

codewhale --version
codewhale doctor
codewhale exec --auto "run pwd"
```

Known limitations:

- Commands inherit Android's per-app UID, SELinux, and seccomp protections and
  any permissions granted to Termux. Codewhale's opt-in bubblewrap
  child-process sandbox is Linux-only and is not built on Android, so approved
  commands receive no Codewhale-specific filesystem narrowing.
- The Termux build has no supported Android Keystore or desktop Secret Service
  integration. Use `codewhale auth status` to confirm the active source and
  prefer provider environment variables when file-backed plaintext storage is
  not acceptable.
- Terminal rendering varies by Android terminal app. The TUI always owns the
  alternate screen. If a terminal app cannot render the full-screen TUI,
  use `codewhale exec` for headless runs instead.

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

On macOS, use
`shasum -a 256 -c codewhale-artifacts-sha256.txt --ignore-missing` instead of
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

npm is the recommended install path (Node 18+; wrapper available for v0.8.56
and later). It installs the registry's latest published version, not an
unpublished source candidate.

```bash
npm install -g codewhale
codewhale --version   # prints the published version that was installed
```

`postinstall` downloads the matching `codewhale` and `codew` binaries, verifies
them against that source's SHA-256 manifest, and exposes `codewhale` and `codew`
on your `PATH`.

On **Linux x64** (including OpenHarmony x64) the wrapper does **not** wait for
a slow GitHub binary download or a long failure timeout. Unless you set an
explicit release base URL or `CODEWHALE_USE_CNB_MIRROR=1`, it concurrently
fetches the small `codewhale-artifacts-sha256.txt` manifests from GitHub
Releases and the first-party CNB release for the exact package version, accepts
the first source whose HTTP response and manifest validate for the required
assets, cancels the other probe, and downloads the binaries only from that
locked source. CNB publishes Linux x64 only; other targets keep the GitHub-only
path. The selected source is printed in install progress and written to
`<binary>.source` next to the downloaded file. A checksum or source mismatch
fails closed.

On Windows, run those commands from **Windows Terminal** rather than `cmd.exe`
so fonts and colors match the supported TUI. The GitHub Release also publishes
`codewhale.bat` next to the bare x64 exe; that launcher prefers `wt.exe` and
falls back to a direct launch when Windows Terminal is absent.

Useful environment variables:

| Variable                            | Purpose                                                                                |
| ----------------------------------- | -------------------------------------------------------------------------------------- |
| `CODEWHALE_RELEASE_BASE_URL`        | Override the download root. Skips the Linux x64 GitHub/CNB race.                        |
| `CODEWHALE_USE_CNB_MIRROR=1`        | Force the CNB first-party mirror on Linux x64 / OpenHarmony x64. Other targets fail.   |
| `CODEWHALE_VERSION`                 | Pin which release the wrapper downloads (defaults to `codewhaleBinaryVersion`).        |
| `CODEWHALE_GITHUB_REPO`             | Point the downloader at a fork (`owner/repo`).                                          |
| `CODEWHALE_FORCE_DOWNLOAD=1`        | Re-download even if a cached binary marker matches.                                    |
| `CODEWHALE_DISABLE_INSTALL=1`       | Skip the `postinstall` download entirely (CI smoke, vendored binaries).                 |
| `CODEWHALE_OPTIONAL_INSTALL=1`      | Don't fail `npm install` on retryable download errors — useful in CI matrices.          |
| `CODEWHALE_QUIET_INSTALL=1`         | Suppress installer progress messages.                                                   |
| `CODEWHALE_DOWNLOAD_TIMEOUT_MS`     | Override the total download budget in milliseconds.                                     |
| `CODEWHALE_DOWNLOAD_STALL_MS`       | Override the no-progress stall budget in milliseconds.                                  |

The corresponding `DEEPSEEK_TUI_*` and `DEEPSEEK_*` variables remain accepted
as legacy aliases, after the canonical `CODEWHALE_*` names. New automation and
support instructions should use only the Codewhale names.

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
install from crates.io directly. One Cargo package is required:
`codewhale-cli` installs the `codewhale` command. npm and prebuilt releases also
expose `codew` as a convenience name for the same compiled runtime; Cargo does
not create that alias, so define a shell alias yourself if you want the shorter
name.

```bash
# Requires Rust 1.88+ (https://rustup.rs)
cargo install codewhale-cli --locked   # installs `codewhale`
codewhale --version
```

> **Linux: install build-time dependencies first.** `cargo install` compiles
> from source, and on Linux the `codewhale-cli` crate links against
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

Nix builds `codewhale` (single binary) and then starts the dispatcher. Pass
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

    codewhale.url = "github:Hmbown/CodeWhale";
    codewhale.inputs.nixpkgs.follows = "nixpkgs";
  };
}
```

Install into a NixOS module:

```nix
{
  outputs = { self, nixpkgs, codewhale }:
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
          environment.systemPackages = [ codewhale.packages.${system}.default ];
        }
      ];
    };
  };
}
```

---

## Omarchy / AUR

On Omarchy, install the prebuilt AUR package:

```bash
omarchy pkg aur add codewhale-bin
codewhale --version
```

`codewhale-bin` packages the same checksum-pinned Linux release archives as the
other binary install paths and provides both `codewhale` and `codew`. It does
not carry a separate Codewhale version; the existing `codewhale-tui`
compatibility command remains an alias to the same runtime. Package updates
arrive through `omarchy update`; the in-app updater leaves the pacman-owned
binary to Omarchy.

The AUR update follows the matching Codewhale tag and release assets, so it may
appear after the GitHub release while its generated `PKGBUILD` and `.SRCINFO`
are validated. Release-maintainer instructions live in
[`packaging/aur/README.md`](../packaging/aur/README.md).

---

## Homebrew

The formula is `codewhale`. The tap GitHub repo is still
`Hmbown/homebrew-deepseek-tui` until it is renamed; `brew tap Hmbown/deepseek-tui`
keeps working either way.

```bash
brew tap Hmbown/deepseek-tui
brew install codewhale
```

Update with `brew upgrade codewhale`. Existing Cellar installs under the
legacy `deepseek-tui` formula name can still run `brew upgrade deepseek-tui`
for one overlap release; new installs should use `codewhale`.

---

## 6. Manual download from GitHub Releases

Each platform appears on the Releases page in **two forms** (this is intentional — see #3208):
the **bare binaries** (`codewhale-<platform>` and `codew-<platform>`, no extension) and a **`.tar.gz` / `.zip` archive**
(`codewhale-<platform>.tar.gz`) that bundles the same commands plus an
`install.sh`. The npm wrapper and the in-app `codewhale update` download the
matched runtime binaries; the archive is the easiest manual install (see §6).
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
chmod +x ~/.local/bin/codewhale ~/.local/bin/codew
codewhale --version
```

> **macOS Gatekeeper note.** If you downloaded the binaries with a browser,
> macOS may block them with "Apple cannot verify" warnings. Clear the quarantine
> attribute on both binaries and retry:
> ```bash
> xattr -d com.apple.quarantine ~/.local/bin/codewhale ~/.local/bin/codew 2>/dev/null || true
> ```

Verify integrity against the per-release SHA-256 manifest:

```bash
curl -L -o /tmp/codewhale-artifacts-sha256.txt \
    https://github.com/Hmbown/CodeWhale/releases/latest/download/codewhale-artifacts-sha256.txt
( cd ~/.local/bin && sha256sum -c /tmp/codewhale-artifacts-sha256.txt --ignore-missing )
```

(Use `shasum -a 256 -c /tmp/codewhale-artifacts-sha256.txt --ignore-missing`
instead of `sha256sum -c` on macOS.)

### Roll back to a previous release

If a new release is bad on your machine, install the last known-good version
explicitly. Replace `X.Y.Z` with the version you want to restore.

```bash
# npm wrapper, only for versions that were published to npm
npm install -g codewhale@X.Y.Z

# Cargo path: one package installs codewhale
cargo install codewhale-cli --version X.Y.Z --locked --force
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

Inside a Codewhale workspace, `/restore list [N]` lists side-git file snapshots
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

### Windows winget (v0.9.5+)

Codewhale publishes a winget manifest for `Hmbown.CodeWhale` (resolves #1561).
Winget installs only the `codewhale` + `codew` commands. GitHub Releases retain
byte-identical `codewhale-tui-*` filenames only for legacy updater compatibility;
they are not a third installed command.

```powershell
winget install Hmbown.CodeWhale
codewhale --version
```

The manifest is at [`packaging/winget/Hmbown.CodeWhale.yaml`](../packaging/winget/Hmbown.CodeWhale.yaml)
(also mirrored at [`.winget/Hmbown.CodeWhale.yaml`](../.winget/Hmbown.CodeWhale.yaml)) and lists both
the NSIS installer (`CodeWhaleSetup.exe`, per-user, adds `%LOCALAPPDATA%\Programs\CodeWhale\bin` to the user PATH)
and the portable ZIP fallback (`codewhale-windows-x64.zip` / `codewhale-windows-arm64.zip`). winget
selects the matching architecture automatically; both install the single binary (`codewhale.exe` + `codew.exe`).
The zips also include `codewhale.bat`. Double-click that launcher (not the raw `.exe`) so the first
window is Windows Terminal when it is installed.

Update via `winget upgrade Hmbown.CodeWhale` or `codewhale update`. The winget package is
maintained outside this repo's release workflow and can lag GitHub/npm/Cargo releases by one
validation cycle — use npm or the GitHub Release asset when you need the newest version immediately.
If `winget install` reports a hash mismatch, verify `codewhale-artifacts-sha256.txt` for the same
tag and regenerate the manifest via `packaging/winget/generate-winget-manifest.sh` (see
[`packaging/winget/README.md`](../packaging/winget/README.md)) before re-submitting to
[microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs).

> **Windows ARM64 note.** The NSIS installer currently contains only the x64 binaries.
> Windows ARM64 users should install via `winget install Hmbown.CodeWhale` (ARM64 ZIP) or
> `npm install -g codewhale` under native ARM64 Node.js, or download
> `codewhale-windows-arm64.zip` directly — all paths install native ARM64 binaries.

### Windows NSIS Installer

A standalone NSIS-based installer is available starting with v0.8.50 for
Windows users who prefer a traditional double-click setup (no npm, no Scoop, no
Cargo required).

The NSIS installer currently contains the Windows x64 binaries. Windows ARM64
users should install through npm running under native ARM64 Node.js or download
`codewhale-windows-arm64.zip` from the same release; both paths then use native
ARM64 binaries.

**Download** `CodeWhaleSetup.exe` from the
[Releases page](https://github.com/Hmbown/CodeWhale/releases/latest).

**Install** by double-clicking the setup executable. The installer:

- Installs `codewhale.exe` and `codew.exe` side-by-side (single binary, no `codewhale-tui.exe`) into
  `%LOCALAPPDATA%\Programs\CodeWhale\bin`
- Installs `codewhale.bat`, which prefers Windows Terminal (`wt.exe`) when it is on `PATH` and
  otherwise launches the exe directly
- Creates a current-user Start Menu shortcut that opens that launcher, not the raw `.exe`
- Adds the install directory to the **current user** `PATH`
- Registers in Windows **Apps & Features** for easy uninstall

Uninstall removes the binaries, `codewhale.bat`, the Start Menu shortcut, and the user `PATH` entry.

**Silent install** (for IT admins, SCCM, Intune):

```powershell
CodeWhaleSetup.exe /S
```

The installer is per-user and does not request elevation. Run silent installs in
the target user's context, or use a deployment tool that can run the installer
for each user profile that needs Codewhale.

The release-built installer is currently unsigned and may trigger Windows
SmartScreen. Verify the SHA-256 checksum from `codewhale-artifacts-sha256.txt`
before deploying, and sign the installer in your internal deployment pipeline if
your environment requires signed application packages.

**Build the installer yourself** (requires [NSIS](https://nsis.sourceforge.io)):

```powershell
cd scripts\installer
# Place codewhale.exe and codew.exe here (single binary, no codewhale-tui.exe), then:
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

cargo install --path crates/cli --locked   # installs `codewhale`

codewhale --version
```

The command lands in `~/.cargo/bin/` by default; make sure that directory is
on your `PATH`.

### FreeBSD 14+ (resolves #1097)

FreeBSD has no prebuilt GitHub Release asset — `npm install -g codewhale` intentionally
fails with `Unsupported platform: freebsd` and points to Cargo. Install from source:

```bash
pkg install -y rust pkgconf git
cargo install codewhale-cli --locked   # installs `codewhale`
codewhale --version
codewhale doctor
```

The `rquickjs` FreeBSD bindings are generated at build time via `bindgen` (see
`1582ba965`/`5eb0385e8`). No separate `pkg install codewhale` port exists yet —
a native port is tracked as the follow-up to #1097 under `packaging/freebsd/`
(contributions welcome). Validate with `cargo check --target x86_64-unknown-freebsd -p codewhale-cli --locked`
on the release branch; the 7×1 release matrix (Linux musl x64/arm64,
Android arm64, macOS x64/arm64, Windows x64/arm64) stays 7 targets — FreeBSD is a
source-build target, not a prebuilt asset.

### Cross-compiling from x64 to ARM64 Linux

The release asset uses `aarch64-unknown-linux-musl` and is built on a native ARM
runner. If you want to build a GNU-linked ARM64 Linux binary on an x64 Linux
host (e.g. for a HarmonyOS / openEuler ARM64 thin-and-light), use
[`cross`](https://github.com/cross-rs/cross), which wraps the official Rust
cross-targets in a Docker container:

```bash
# Once
rustup target add aarch64-unknown-linux-gnu
cargo install cross --locked

# Per build
cross build --release --target aarch64-unknown-linux-gnu -p codewhale-cli   # single binary
```

The resulting binary lands in
`target/aarch64-unknown-linux-gnu/release/codewhale`. Copy it to the ARM64 host
(e.g. via `scp`) and make it executable. This local GNU build is distinct from
the portable musl release asset; either executable can be copied under the
`codew` convenience name.

If you don't have Docker available, install the cross-linker directly and let
Cargo do the work:

```bash
sudo apt-get install -y gcc-aarch64-linux-gnu
rustup target add aarch64-unknown-linux-gnu

cat >> ~/.cargo/config.toml <<'EOF'
[target.aarch64-unknown-linux-gnu]
linker = "aarch64-linux-gnu-gcc"
EOF

cargo build --release --target aarch64-unknown-linux-gnu -p codewhale-cli   # single binary
```

Producing `aarch64-unknown-linux-musl` while cross-compiling requires an
appropriate musl cross-linker. The release workflow avoids that extra moving
part by building and launching the musl binary on GitHub's native ARM runner.

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

The Cargo-built binary appears at `target\release\codewhale.exe`. Release
packaging separately exposes the same executable as `codew.exe`.

> Prefer not to build? Install via npm, Cargo, GitHub Releases, or the CNB
> mirror — see the sections above.

---

## 8. Shell completions

Codewhale generates its own completion scripts. One command per shell; each
script completes **both** `codewhale` and the `codew` shorthand.

```bash
codewhale completion <bash|zsh|fish|powershell|elvish>
```

`codewhale completions` is an accepted alias for the same command.

The script is written to stdout, so installing it is a redirect to wherever
your shell loads completions from.

**Bash** — needs the `bash-completion` package loaded by your shell:

```bash
mkdir -p ~/.local/share/bash-completion/completions
codewhale completion bash > ~/.local/share/bash-completion/completions/codewhale
```

For the current shell only: `source <(codewhale completion bash)`.

**Zsh** — the script's `#compdef` line already covers both command names:

```bash
mkdir -p ~/.zfunc
codewhale completion zsh > ~/.zfunc/_codewhale
```

If `~/.zfunc` is not already on `fpath`, add this to `~/.zshrc`:

```zsh
fpath=(~/.zfunc $fpath)
autoload -Uz compinit && compinit
```

**Fish**:

```fish
mkdir -p ~/.config/fish/completions
codewhale completion fish > ~/.config/fish/completions/codewhale.fish
```

**PowerShell** — append to your profile so it loads in every session:

```powershell
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $PROFILE)
codewhale completion powershell >> $PROFILE
```

For the current session only:

```powershell
codewhale completion powershell | Out-String | Invoke-Expression
```

**Elvish** — the script registers both command names:

```elvish
codewhale completion elvish >> ~/.config/elvish/rc.elv
```

Regenerate the script after upgrading Codewhale — it is a snapshot of the
command surface at the version that produced it, not a live query.

> Upgrading from v0.9.10 or earlier? Those releases emitted a script that
> registered the internal `codewhale-tui` executable, so nothing completed for
> `codewhale` or `codew` ([#5526](https://github.com/Hmbown/CodeWhale/issues/5526)).
> Delete the old file and regenerate it with the commands above.

---

## 9. Troubleshooting

### `Unsupported architecture: arm64 on platform linux`

You're on a release earlier than v0.8.8 that doesn't publish Linux ARM64
binaries. Either upgrade (`npm i -g codewhale@latest`) or use
`cargo install` per [Section 4](#4-install-via-cargo-any-tier-1-rust-target).

### `MISSING_COMPANION_BINARY` after upgrading an older install

The current single binary runs the TUI in-process and does not require a
companion executable. This error identifies a stale pre-v0.9.5 dispatcher;
replace that installation with the current npm package or Cargo binary instead
of downloading an extra runtime:

```bash
npm install -g codewhale
# or
cargo install codewhale-cli --locked --force
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

On Linux x64 the npm wrapper already probes GitHub Releases and the CNB
first-party checksum manifests in parallel and downloads binaries only from
the first source that validates. You do not need `CODEWHALE_USE_CNB_MIRROR=1`
for that automatic path.

If both first-party sources fail, set `CODEWHALE_RELEASE_BASE_URL` to a
mirrored release-asset directory (rsproxy, TUNA, Tencent COS, Aliyun OSS),
or skip npm entirely and use the Cargo mirror setup in
[Section 4](#4-install-via-cargo-any-tier-1-rust-target). The legacy
`DEEPSEEK_TUI_RELEASE_BASE_URL` name is still accepted. `CODEWHALE_USE_CNB_MIRROR=1`
still forces CNB only on Linux x64 / OpenHarmony x64.

### `codewhale update` is blocked by GitHub from mainland China

`codewhale update` normally contacts GitHub Releases for metadata and binary
assets. On networks where GitHub is blocked or unreliable, use the CNB source
mirror instead and install the `codewhale-cli` package from the release tag.
Cargo installs the `codewhale` command:

To check the latest release without downloading or replacing binaries, run
`codewhale update --check`.

```bash
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag vX.Y.Z codewhale-cli --locked --force   # single binary
```

If you operate a binary asset mirror, `codewhale update` can use it directly:

```bash
CODEWHALE_RELEASE_BASE_URL=https://your-mirror.example.com/CodeWhale/vX.Y.Z/ \
CODEWHALE_VERSION=X.Y.Z \
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

Install current stable Rust through rustup, then rerun the one Cargo package
install command from [Section 4](#4-install-via-cargo-any-tier-1-rust-target).
It installs `codewhale`. For
mainland China networks, this rsproxy-based sequence has been verified to work:

```bash
export RUSTUP_DIST_SERVER=https://rsproxy.cn
export RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
rustup default stable
cargo install codewhale-cli --locked   # installs `codewhale`
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
codewhale-cli --locked` fails while compiling the keyring or D-Bus secret
storage crates, install the Linux build dependencies inside the WSL distro,
then rerun the one Cargo package install command. It installs `codewhale`:

```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libdbus-1-dev
cargo install codewhale-cli --locked   # installs `codewhale`
```

The prebuilt npm/GitHub binaries do not need these build-time packages; they
only apply when WSL2 is compiling Codewhale from source.

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
successfully but the prebuilt binary download is blocked or unreliable on
your network. This download is separate from the npm registry package
download. On Linux x64 the wrapper first races the small GitHub and CNB
checksum manifests and does not wait for a full GitHub binary to time out
before using a valid CNB manifest.

Use one of these paths:

1. Set a proxy and retry:

   ```bash
   export HTTPS_PROXY=http://your-proxy:port
   codewhale
   ```

2. Mirror the release assets internally and set `CODEWHALE_RELEASE_BASE_URL`:

   ```bash
   export CODEWHALE_RELEASE_BASE_URL=https://your-mirror.example.com/CodeWhale/
   codewhale
   ```

   The directory must contain `codewhale-artifacts-sha256.txt` and the platform
   binaries from the GitHub release.

3. Install via Cargo, which builds locally and does not download GitHub release
   assets. See [Section 4](#4-install-via-cargo-any-tier-1-rust-target).

4. Download both matching `codewhale` and `codew`
   binaries from the [Releases page](https://github.com/Hmbown/CodeWhale/releases),
   place them in a directory on `PATH`, and make them executable. See
   [Section 6](#6-manual-download-from-github-releases).

---

## 10. Verifying your install

```bash
codewhale --version
codewhale doctor       # checks API key, provider, runtime, and PATH integrity
codewhale doctor --json
```

`doctor` exits non-zero if it finds a problem and prints structured remediation
hints. Paste the JSON output into a GitHub issue if you need help.
