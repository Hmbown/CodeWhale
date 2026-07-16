#!/usr/bin/env node

const crypto = require("crypto");
const fs = require("fs/promises");
const path = require("path");

const {
  allReleaseAssetNames,
  BUNDLE_ASSET_NAMES,
  BUNDLE_CHECKSUM_MANIFEST,
  CHECKSUM_MANIFEST,
  detectBinaryNames,
} = require("../../npm/codewhale/scripts/artifacts");

const WINDOWS_LAUNCHER = "codewhale.bat";
const WINDOWS_CLI_ASSET = "codewhale-windows-x64.exe";

async function sha256(filePath) {
  const content = await fs.readFile(filePath);
  return crypto.createHash("sha256").update(content).digest("hex");
}

async function main() {
  const prepareAllAssets =
    process.env.DEEPSEEK_TUI_PREPARE_ALL_ASSETS === "1" ||
    process.env.DEEPSEEK_PREPARE_ALL_ASSETS === "1";
  const outputDir = path.resolve(
    process.argv[2] || path.join("target", "npm-release-assets"),
  );
  const buildDir = path.resolve(
    process.argv[3] || path.join("target", "release"),
  );
  const { codewhale, tui, codew } = detectBinaryNames();
  const isWindows = process.platform === "win32";

  const assets = [
    {
      source: path.join(buildDir, isWindows ? "codewhale.exe" : "codewhale"),
      target: codewhale,
    },
    {
      source: path.join(buildDir, isWindows ? "codew.exe" : "codew"),
      target: codew,
    },
    {
      source: path.join(buildDir, isWindows ? "codewhale-tui.exe" : "codewhale-tui"),
      target: tui,
    },
  ];

  if (prepareAllAssets) {
    for (const assetName of allReleaseAssetNames()) {
      if (
        assetName === WINDOWS_LAUNCHER ||
        assetName === CHECKSUM_MANIFEST ||
        assetName === BUNDLE_CHECKSUM_MANIFEST
      ) {
        continue;
      }
      if (assets.some((asset) => asset.target === assetName)) {
        continue;
      }
      assets.push({
        source: assetName.startsWith("codewhale-tui")
          ? path.join(buildDir, isWindows ? "codewhale-tui.exe" : "codewhale-tui")
          : assetName.startsWith("codew-")
            ? path.join(buildDir, isWindows ? "codew.exe" : "codew")
            : path.join(buildDir, isWindows ? "codewhale.exe" : "codewhale"),
        target: assetName,
      });
    }
  }

  await fs.mkdir(outputDir, { recursive: true });

  const manifestLines = [];
  for (const asset of assets) {
    const outputPath = path.join(outputDir, asset.target);
    await fs.copyFile(asset.source, outputPath);
    manifestLines.push(`${await sha256(outputPath)}  ${asset.target}`);
  }

  if (assets.some((asset) => asset.target === WINDOWS_CLI_ASSET)) {
    const batContent = [
      "@echo off",
      "where wt >nul 2>nul",
      "set NO_ANIMATIONS=1",
      'if "%ERRORLEVEL%"=="0" (',
      '    wt --title Codewhale cmd /k "%~dp0codewhale-windows-x64.exe"',
      ") else (",
      '    "%~dp0codewhale-windows-x64.exe"',
      ")",
      "",
    ].join("\r\n");
    const batPath = path.join(outputDir, WINDOWS_LAUNCHER);
    await fs.writeFile(batPath, batContent, "utf8");
    const batHash = await sha256(batPath);
    manifestLines.push(`${batHash}  ${WINDOWS_LAUNCHER}`);
    console.log(`Generated ${batPath}`);
  }

  if (prepareAllAssets) {
    const bundleManifestLines = [];
    for (const assetName of BUNDLE_ASSET_NAMES) {
      const assetPath = path.join(outputDir, assetName);
      bundleManifestLines.push(`${await sha256(assetPath)}  ${assetName}`);
    }
    bundleManifestLines.sort();
    const bundleManifestPath = path.join(outputDir, BUNDLE_CHECKSUM_MANIFEST);
    await fs.writeFile(
      bundleManifestPath,
      `${bundleManifestLines.join("\n")}\n`,
      "utf8",
    );
    manifestLines.push(
      `${await sha256(bundleManifestPath)}  ${BUNDLE_CHECKSUM_MANIFEST}`,
    );
    console.log(`Wrote bundle checksum manifest ${bundleManifestPath}`);
  }

  manifestLines.sort();
  const manifestPath = path.join(outputDir, CHECKSUM_MANIFEST);
  await fs.writeFile(manifestPath, `${manifestLines.join("\n")}\n`, "utf8");

  const preparedCount = prepareAllAssets
    ? allReleaseAssetNames().length
    : assets.length + 1;
  console.log(`Prepared ${preparedCount} assets in ${outputDir}`);
  console.log(`Wrote checksum manifest ${manifestPath}`);
}

main().catch((error) => {
  console.error("Failed to prepare local release assets:", error.message);
  process.exit(1);
});
