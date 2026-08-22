# winget packaging for CodeWhale

This directory holds the source winget manifest for `Hmbown.CodeWhale` (resolves #1561).
Winget installs the single runtime under `codewhale` + `codew`; it never
installs a `codewhale-tui` command. GitHub Releases retain byte-identical
`codewhale-tui-*` filenames only for legacy updater compatibility.

## Files

- `Hmbown.CodeWhale.yaml` — singleton manifest for `winget install Hmbown.CodeWhale`. The
  installers all point at the signed (or checksum-verified) GitHub Release assets for the same
  version (`CodeWhaleSetup.exe` for x64 NSIS, plus portable ZIP fallbacks for x64/arm64).
- `generate-winget-manifest.sh` — bumps `PackageVersion`, `ReleaseDate`, and the four
  `InstallerSha256` placeholders from a local `release-assets/` checkout.
- `.winget/Hmbown.CodeWhale.yaml` (repo root) is a verbatim mirror for tooling that expects `.winget/`.
  Keep both in sync; `packaging/winget/Hmbown.CodeWhale.yaml` is canonical.

## Version flow

1. Tag `vX.Y.Z` publishes `CodeWhaleSetup.exe`, `codewhale-windows-x64.zip`,
   `codewhale-windows-x64-portable.zip`, `codewhale-windows-arm64.zip`,
   `codewhale-windows-arm64-portable.zip`, and `codewhale-artifacts-sha256.txt`.
2. From the release tag checkout, run:
   ```bash
   ./packaging/winget/generate-winget-manifest.sh X.Y.Z /path/to/release-assets
   ```
   It rewrites both `packaging/winget/Hmbown.CodeWhale.yaml` and `.winget/Hmbown.CodeWhale.yaml`
   with the fresh version and the four SHA-256 values extracted from `codewhale-artifacts-sha256.txt`.
3. Validate locally with `winget validate` (requires winget + the manifest schema):
   ```bash
   winget validate --manifest packaging/winget/Hmbown.CodeWhale.yaml
   # or the Microsoft validator in winget-pkgs CI:
   # https://github.com/microsoft/winget-pkgs#validation
   ```
4. Submit to [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs) via
   `wingetcreate` or a manual PR that adds `manifests/h/Hmbown/CodeWhale/X.Y.Z/`:
   ```bash
   wingetcreate update Hmbown.CodeWhale --version X.Y.Z --urls \
     https://github.com/Hmbown/CodeWhale/releases/download/vX.Y.Z/CodeWhaleSetup.exe \
     https://github.com/Hmbown/CodeWhale/releases/download/vX.Y.Z/codewhale-windows-x64.zip \
     https://github.com/Hmbown/CodeWhale/releases/download/vX.Y.Z/codewhale-windows-x64-portable.zip \
     https://github.com/Hmbown/CodeWhale/releases/download/vX.Y.Z/codewhale-windows-arm64.zip \
     https://github.com/Hmbown/CodeWhale/releases/download/vX.Y.Z/codewhale-windows-arm64-portable.zip
   ```
   The generated PR must pass the winget-pkgs validation workflow before merge.

## Single-binary note

Until v0.9.4 the release matrix installed three commands (`codewhale`, `codew`,
and `codewhale-tui`). Since v0.9.5 each target installs only the byte-identical
`codewhale` + `codew` commands (Windows also ships `codewhale.bat`). GitHub
Releases retain `codewhale-tui-*` compatibility filenames for old updater
clients, but the winget ZIP `NestedInstallerFiles` lists only the two current
PATH commands; `codewhale-tui.exe` is intentionally absent.

## FreeBSD

FreeBSD has no prebuilt GitHub Release asset (see `docs/INSTALL.md` § FreeBSD). Install via Cargo:

```bash
pkg install -y rust pkgconf  # or ports-mgmt/pkg
cargo install codewhale-cli --locked   # provides `codewhale`
```

The npm wrapper on FreeBSD exits with `Unsupported platform: freebsd` and points to the Cargo path.
A native `pkg install codewhale` port is tracked as a follow-up to #1097 — contributions welcome
under `packaging/freebsd/`.
