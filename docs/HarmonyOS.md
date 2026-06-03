# HarmonyOS and OpenHarmony

This page covers CodeWhale on HarmonyOS PC and OpenHarmony cross-build setups.

## Running On HarmonyOS PC

HarmonyOS PC can use the normal Linux ARM64 package when its userspace is
glibc-compatible:

```bash
npm i -g codewhale
codewhale --version
```

You can also download `codewhale-linux-arm64` and
`codewhale-tui-linux-arm64` from the GitHub Releases page and place both
binaries on `PATH`.

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
CMake toolchain variables for `aarch64-unknown-linux-ohos`.

## Compiler Wrappers

For ad-hoc compiler calls, use the root wrappers. They read the same
`OHOS_NATIVE_SDK` variable and do not contain local paths.

Windows PowerShell:

```powershell
.\ohos-clang.ps1 --version
.\ohos-clangxx.ps1 --version
```

Linux or macOS:

```bash
sh ./ohos-clang.sh --version
sh ./ohos-clangxx.sh --version
```

If you want to run the POSIX wrappers directly as `./ohos-clang.sh`, make them
executable first:

```bash
chmod +x ./ohos-clang.sh ./ohos-clangxx.sh
```

## Cargo Config

`.cargo/config.toml` intentionally does not set a checked-in linker path.
Cargo cannot expand environment variables inside `linker` or CMake toolchain
path values there, so those values are exported by `scripts/ohos-env.ps1` and
`scripts/ohos-env.sh` instead.
