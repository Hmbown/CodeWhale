# AUR packaging for Codewhale

`codewhale-bin` installs the prebuilt `codewhale` runtime and its `codew`
convenience command from the same Linux archives published for each Codewhale
release. It does not compile a fork or carry a separate Codewhale version.

The AUR repository is a separate publication destination. This directory is
the upstream source of its `PKGBUILD` and `.SRCINFO`, but the files with those
final names are generated only after the matching release archives and
checksum manifests exist. Nothing here publishes to AUR.

## Render a release update

From a checkout whose `Cargo.toml` has the released `X.Y.Z` workspace version,
obtain the complete, verified `codewhale-release-assets` directory and run:

```bash
./packaging/aur/render.sh /path/to/release-assets /tmp/codewhale-bin
```

The renderer:

- reads `pkgver` from `[workspace.package]` rather than accepting a second
  product version;
- requires the x64 and arm64 Linux archives under their canonical release
  names;
- requires both release checksum manifests to agree with each other and with
  the archive bytes;
- inserts real SHA-256 values for both archives and the tagged MIT license;
- rejects unresolved placeholders, `SKIP`, malformed archives, and non-empty
  output directories; and
- compares the rendered `.SRCINFO` with `makepkg --printsrcinfo` when `makepkg`
  is available.

The initial AUR revision for each upstream release is `pkgrel=1`. A recipe-only
correction may use a higher Arch package revision without changing Codewhale's
semantic version:

```bash
./packaging/aur/render.sh /path/to/release-assets /tmp/codewhale-bin 2
```

Do not invent checksums before release artifacts exist, use `SKIP`, or copy
hashes from an older release.

## Validate on Arch or Omarchy

Review the generated files, then validate them in an unprivileged clean build
environment:

```bash
cd /tmp/codewhale-bin
makepkg --verifysource
makepkg --printsrcinfo | cmp - .SRCINFO
makepkg --cleanbuild
namcap PKGBUILD codewhale-bin-*.pkg.tar.zst
```

Inspect the package contents before installation. They should contain
`/usr/bin/codewhale`, `/usr/bin/codew`, the existing `codewhale-tui`
compatibility symlink to `codewhale`, and the MIT license. After a test install,
verify the canonical entrypoints report `X.Y.Z` and the compatibility symlink
resolves to the same runtime.

Publishing the generated files to the `codewhale-bin` AUR repository requires
separate authorization from an AUR maintainer. Regenerate `.SRCINFO` whenever
package metadata changes, and never publish this recipe before the matching
Codewhale tag, archives, and checksum manifests are public and verified.

Arch's `PKGBUILD(5)` contract defines architecture-specific sources and
checksums, and the AUR requires `.SRCINFO` to accompany metadata changes:

- <https://man.archlinux.org/man/PKGBUILD.5.en>
- <https://wiki.archlinux.org/title/.SRCINFO>
- <https://wiki.archlinux.org/title/AUR_submission_guidelines>
