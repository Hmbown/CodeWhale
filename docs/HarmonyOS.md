# HarmonyOS and OpenHarmony

This page covers Codewhale on HarmonyOS PC and OpenHarmony cross-build setups.

## Support Tier

| Target | Codewhale tier | CI coverage | Distribution |
| --- | --- | --- | --- |
| HarmonyOS PC with a glibc-compatible userspace | Tier 1 Linux ARM64 runtime | Covered by the Linux ARM64 release build | npm and release binaries |
| `aarch64-unknown-linux-ohos` (OpenHarmony) | Tier 2 cross-build target | `codewhale-tui` is checked with a real OpenHarmony native SDK/sysroot | Build from source; no prebuilt release asset |

Tier 2 means every relevant source change is compile-checked, but maintainers do
not promise a release binary or full device-level runtime testing. The CI job
uses the published OpenHarmony 6.1 native SDK; it deliberately fails if the SDK,
Clang, or sysroot is unavailable rather than substituting host headers or a stub
that could report false success.

## Running On HarmonyOS PC

HarmonyOS PC can use the normal Linux ARM64 package when its userspace is
glibc-compatible:

```bash
npm i -g codewhale
codewhale --version
```

You can also download `codewhale-linux-arm64` and `codew-linux-arm64` from the
GitHub Releases page and place both binaries on `PATH`. The
`codewhale-tui-linux-arm64` filename is retained only for legacy updater
compatibility and is not a third command.

## Cross-Compiling To OpenHarmony

The repository does not check in machine-specific SDK paths. Set
`OHOS_NATIVE_SDK` to the OpenHarmony native SDK directory, the directory that
contains `llvm/bin`, `sysroot`, and `build/cmake/ohos.toolchain.cmake`.

On Windows PowerShell:

```powershell
$env:OHOS_NATIVE_SDK="<path-to-openharmony-native-sdk>"
. .\scripts\ohos-env.ps1
rustup target add aarch64-unknown-linux-ohos
cargo build --target aarch64-unknown-linux-ohos -p codewhale-cli
```

On Linux or macOS:

```bash
export OHOS_NATIVE_SDK=/path/to/openharmony/native
. ./scripts/ohos-env.sh
rustup target add aarch64-unknown-linux-ohos
cargo build --target aarch64-unknown-linux-ohos -p codewhale-cli
```

The setup scripts export Cargo's target-specific `linker`, `AR`, `CC`, `CXX`,
`CFLAGS`, `CXXFLAGS`, `CARGO_ENCODED_RUSTFLAGS`, `CC_SHELL_ESCAPED_FLAGS`, and
CMake toolchain variables for `aarch64-unknown-linux-ohos`. They also point
`bindgen` at the SDK's `libclang` and sysroot so `rquickjs-sys` can generate
the OpenHarmony bindings that it does not ship pre-generated.

On Windows, `ohos-env.ps1` points Cargo at the repository's
`ohos-clang.cmd` launcher. The launcher delegates to `ohos-clang.ps1`, so the
final Rust link—not only C/C++ compilation and bindgen—always carries
`-target aarch64-linux-ohos`, the SDK sysroot, and `-D__MUSL__` while preserving
Cargo's linker arguments and exit status. The launcher re-quotes every
argument before forwarding, so an SDK path containing spaces (for example the
default `D:\DevEco Studio\...` install) keeps its `--sysroot` intact through
the final link.

## Compiler Wrappers

For ad-hoc compiler calls, use the wrappers in `scripts/ohos/`. They read the same
`OHOS_NATIVE_SDK` variable and do not contain local paths.

Windows PowerShell:

```powershell
.\scripts\ohos\ohos-clang.ps1 --version
.\scripts\ohos\ohos-clangxx.ps1 --version
```

Linux or macOS:

```bash
sh ./scripts/ohos/ohos-clang.sh --version
sh ./scripts/ohos/ohos-clangxx.sh --version
```

If you want to run the POSIX wrappers directly as `./scripts/ohos/ohos-clang.sh`, make them
executable first:

```bash
chmod +x ./scripts/ohos/ohos-clang.sh ./scripts/ohos/ohos-clangxx.sh
```

## Linker And Toolchain Paths

The repository does not check in a Cargo linker path or CMake toolchain path.
Cargo cannot expand environment variables inside `linker` or CMake toolchain
path values, so those values are exported by `scripts/ohos-env.ps1` and
`scripts/ohos-env.sh` instead.

## Dependency Guard

Release prep runs a no-SDK dependency check:

```bash
./scripts/release/check-ohos-deps.sh
```

The guard asserts the Windows final-link wrapper contract, proves that OHOS
activates the `rquickjs-sys` bindgen feature, resolves the `codewhale-tui`
dependency graph for `aarch64-unknown-linux-ohos`, and fails if unsupported
host/UI crates re-enter that graph: `nix` 0.28/0.29, `portable-pty`, `starlark`,
`arboard`, or `keyring`. This no-SDK check does not replace a real SDK/sysroot
build, but it catches the known linker, bindgen, `starlark -> rustyline -> nix`,
and PTY/keyring regressions before release.

Because `portable-pty` is intentionally absent from the OpenHarmony graph, the
persistent `terminal/*` PTY tools are not registered on that target. The
ordinary `exec_shell` tools remain available through their non-PTY process
implementation.

Linux-only sandbox implementations (bubblewrap, seccomp, and `prctl` process
hardening) are compiled only for
`all(target_os = "linux", not(target_env = "ohos"))`. OpenHarmony therefore
reports no local OS sandbox instead of probing Linux kernel paths or syscalls it
does not support. External OpenSandbox execution remains separately available
when configured.

Native desktop clipboard libraries and Wayland helpers are also excluded from
the OpenHarmony graph. Text copy degrades to the terminal-client path (OSC 52,
or tmux `load-buffer -w` when inside tmux); paste is supplied by the terminal as
normal/bracketed input. Image clipboard reads are unavailable on this target.
If the terminal cannot accept OSC 52, copy returns a clear "Clipboard
unavailable" error rather than panicking or claiming success.
