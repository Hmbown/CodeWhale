#!/usr/bin/env sh
set -eu

if [ -z "${OHOS_NATIVE_SDK:-}" ]; then
    echo "error: set OHOS_NATIVE_SDK to the OpenHarmony native SDK directory. It must contain llvm/bin and sysroot." >&2
    exit 1
fi

sdk=$OHOS_NATIVE_SDK
clangxx=$sdk/llvm/bin/clang++
sysroot=$sdk/sysroot

if [ ! -x "$clangxx" ]; then
    echo "error: OHOS_NATIVE_SDK does not contain executable llvm/bin/clang++: $sdk" >&2
    exit 1
fi

if [ ! -d "$sysroot" ]; then
    echo "error: OHOS_NATIVE_SDK does not contain sysroot: $sdk" >&2
    exit 1
fi

exec "$clangxx" -target aarch64-linux-ohos "--sysroot=$sysroot" -D__MUSL__ "$@"
