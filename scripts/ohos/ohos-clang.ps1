$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($env:OHOS_NATIVE_SDK)) {
    [Console]::Error.WriteLine("error: set OHOS_NATIVE_SDK to the OpenHarmony native SDK directory. It must contain llvm\bin and sysroot.")
    exit 1
}

$sdk = $env:OHOS_NATIVE_SDK
$clang = [System.IO.Path]::Combine($sdk, "llvm", "bin", "clang.exe")
$sysroot = [System.IO.Path]::Combine($sdk, "sysroot")

if (-not (Test-Path -LiteralPath $clang -PathType Leaf -ErrorAction SilentlyContinue)) {
    [Console]::Error.WriteLine("error: OHOS_NATIVE_SDK does not contain llvm\bin\clang.exe: $sdk")
    exit 1
}

if (-not (Test-Path -LiteralPath $sysroot -PathType Container -ErrorAction SilentlyContinue)) {
    [Console]::Error.WriteLine("error: OHOS_NATIVE_SDK does not contain sysroot: $sdk")
    exit 1
}

& $clang -target aarch64-linux-ohos "--sysroot=$sysroot" -D__MUSL__ @args
exit $LASTEXITCODE
