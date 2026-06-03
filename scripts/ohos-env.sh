#!/usr/bin/env sh

if [ -z "${OHOS_NATIVE_SDK:-}" ]; then
    echo "error: set OHOS_NATIVE_SDK to the OpenHarmony native SDK directory." >&2
    return 1 2>/dev/null || exit 1
fi

if [ ! -d "$OHOS_NATIVE_SDK" ]; then
    echo "error: OHOS_NATIVE_SDK does not exist: $OHOS_NATIVE_SDK" >&2
    return 1 2>/dev/null || exit 1
fi

sdk=$(cd "$OHOS_NATIVE_SDK" && pwd)
clang=$sdk/llvm/bin/clang
clangxx=$sdk/llvm/bin/clang++
ar=$sdk/llvm/bin/llvm-ar
sysroot=$sdk/sysroot
cmake_toolchain=$sdk/build/cmake/ohos.toolchain.cmake

for file in "$clang" "$clangxx" "$ar" "$cmake_toolchain"; do
    if [ ! -f "$file" ]; then
        echo "error: required OpenHarmony SDK file is missing: $file" >&2
        return 1 2>/dev/null || exit 1
    fi
done

if [ ! -d "$sysroot" ]; then
    echo "error: required OpenHarmony SDK sysroot is missing: $sysroot" >&2
    return 1 2>/dev/null || exit 1
fi

export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_LINKER=$clang
export AR_aarch64_unknown_linux_ohos=$ar
export CC_aarch64_unknown_linux_ohos=$clang
export CXX_aarch64_unknown_linux_ohos=$clangxx
export CC_SHELL_ESCAPED_FLAGS=1
export CFLAGS_aarch64_unknown_linux_ohos="-target aarch64-linux-ohos --sysroot=\"$sysroot\" -D__MUSL__"
export CXXFLAGS_aarch64_unknown_linux_ohos="-target aarch64-linux-ohos --sysroot=\"$sysroot\" -D__MUSL__"
export CMAKE_TOOLCHAIN_FILE_aarch64_unknown_linux_ohos=$cmake_toolchain

sep=$(printf '\037')
export CARGO_ENCODED_RUSTFLAGS="-Clink-arg=-target${sep}-Clink-arg=aarch64-linux-ohos${sep}-Clink-arg=--sysroot=$sysroot${sep}-Clink-arg=-D__MUSL__"

echo "Configured OpenHarmony Cargo environment for AARCH64_UNKNOWN_LINUX_OHOS from $sdk"
