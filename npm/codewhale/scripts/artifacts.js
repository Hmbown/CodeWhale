const path = require("path");
const os = require("os");

const CHECKSUM_MANIFEST = "codewhale-artifacts-sha256.txt";

const ASSET_MATRIX = {
  linux: {
    x64: ["codewhale-linux-x64", "codewhale-tui-linux-x64", "codew-linux-x64"],
    arm64: ["codewhale-linux-arm64", "codewhale-tui-linux-arm64", "codew-linux-arm64"],
  },
  android: {
    arm64: ["codewhale-android-arm64", "codewhale-tui-android-arm64", "codew-android-arm64"],
  },
  darwin: {
    x64: ["codewhale-macos-x64", "codewhale-tui-macos-x64", "codew-macos-x64"],
    arm64: ["codewhale-macos-arm64", "codewhale-tui-macos-arm64", "codew-macos-arm64"],
  },
  win32: {
    x64: ["codewhale-windows-x64.exe", "codewhale-tui-windows-x64.exe", "codew-windows-x64.exe", "codewhale.bat"],
  },
};

// HarmonyPC (openharmony) is an x86_64 Linux-compatible environment; map it to
// the linux binary family so npm install succeeds without a separate build target.
const PLATFORM_ALIASES = {
  openharmony: "linux",
};

function detectBinaryNames() {
  const rawPlatform = os.platform();
  const platform = PLATFORM_ALIASES[rawPlatform] || rawPlatform;
  const arch = os.arch();
  const defaults = ASSET_MATRIX[platform];
  if (!defaults) {
    const supported = Object.keys(ASSET_MATRIX).map(p => `'${p}'`).join(', ');
    throw new Error(
      `Unsupported platform: ${rawPlatform}. Supported platforms: ${supported}.\n\n` +
      unsupportedBuildHint(),
    );
  }
  const pair = defaults[arch];
  if (!pair) {
    const supported = Object.keys(defaults).map(a => `'${a}'`).join(', ');
    const hint = platform === "linux" && arch === "riscv64" ? unsupportedRiscvHint() : unsupportedBuildHint();
    throw new Error(
      `Unsupported architecture: ${arch} on platform ${platform}. ` +
      `Supported architectures: ${supported}.\n\n` +
      hint,
    );
  }
  return {
    platform,
    arch,
    codewhale: pair[0],
    tui: pair[1],
    codew: pair[2],
  };
}

function unsupportedBuildHint() {
  return [
    "No prebuilt binary is available for this platform/architecture combo.",
    "You can still run codewhale by building from source with Cargo:",
    "",
    "  # Requires Rust 1.88+ (https://rustup.rs)",
    "  cargo install codewhale-cli --locked   # provides `codewhale` and `codew`",
    "  cargo install codewhale-tui --locked   # provides `codewhale-tui`",
    "",
    "Or build from a checkout:",
    "",
    "  git clone https://github.com/Hmbown/CodeWhale.git",
    "  cd CodeWhale",
    "  cargo install --path crates/cli --locked",
    "  cargo install --path crates/tui --locked",
    "",
    "See https://github.com/Hmbown/CodeWhale/blob/main/docs/INSTALL.md",
    "for cross-compilation, mirror, and Linux ARM64 specifics.",
  ].join("\n");
}

function unsupportedRiscvHint() {
  return [
    "Linux riscv64 prebuilt binaries are temporarily unavailable.",
    "CodeWhale currently depends on rquickjs-sys, which does not ship",
    "riscv64gc-unknown-linux-gnu bindings in the locked dependency set.",
    "",
    "Track the release notes and docs/INSTALL.md for the next RISC-V support update.",
  ].join("\n");
}

function executableName(base, platform) {
  return platform === "win32" ? `${base}.exe` : base;
}

function releaseBaseUrl(version, repo = "Hmbown/CodeWhale") {
  // CODEWHALE_RELEASE_BASE_URL is the canonical override.
  // DEEPSEEK_TUI_RELEASE_BASE_URL / DEEPSEEK_RELEASE_BASE_URL are legacy aliases.
  const override =
    process.env.CODEWHALE_RELEASE_BASE_URL ||
    process.env.DEEPSEEK_TUI_RELEASE_BASE_URL ||
    process.env.DEEPSEEK_RELEASE_BASE_URL;
  if (override) {
    const trimmed = String(override).trim();
    return trimmed.endsWith("/") ? trimmed : `${trimmed}/`;
  }
  // When CODEWHALE_USE_CNB_MIRROR is set, use the CNB (China-friendly)
  // mirror that already builds and publishes binary release assets.
  if (process.env.CODEWHALE_USE_CNB_MIRROR) {
    return `https://cnb.cool/Hmbown/CodeWhale/-/releases/v${version}/`;
  }
  return `https://github.com/${repo}/releases/download/v${version}/`;
}

function releaseAssetUrl(baseName, version, repo = "Hmbown/CodeWhale") {
  return new URL(baseName, releaseBaseUrl(version, repo)).toString();
}

function checksumManifestUrl(version, repo = "Hmbown/CodeWhale") {
  return releaseAssetUrl(CHECKSUM_MANIFEST, version, repo);
}

function releaseBinaryDirectory() {
  return path.join(__dirname, "..", "bin", "downloads");
}

function allAssetNames() {
  const names = [];
  for (const platformAssets of Object.values(ASSET_MATRIX)) {
    for (const assets of Object.values(platformAssets)) {
      names.push(...assets);
    }
  }
  return Array.from(new Set(names));
}

function allReleaseAssetNames() {
  return [...allAssetNames(), CHECKSUM_MANIFEST];
}

module.exports = {
  allAssetNames,
  allReleaseAssetNames,
  CHECKSUM_MANIFEST,
  checksumManifestUrl,
  detectBinaryNames,
  executableName,
  releaseAssetUrl,
  releaseBaseUrl,
  releaseBinaryDirectory,
};
