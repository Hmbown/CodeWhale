#!/usr/bin/env node

const crypto = require("crypto");
const fs = require("fs/promises");
const path = require("path");

const {
  allAssetNames,
  CHECKSUM_MANIFEST,
  detectBinaryNames,
} = require("../../npm/codewhale/scripts/artifacts");

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
  const { codewhale, tui } = detectBinaryNames();
  const isWindows = process.platform === "win32";

  const assets = [
    {
      source: path.join(buildDir, isWindows ? "codewhale.exe" : "codewhale"),
      target: codewhale,
    },
    {
      source: path.join(buildDir, isWindows ? "codewhale-tui.exe" : "codewhale-tui"),
      target: tui,
    },
  ];

  if (prepareAllAssets) {
    for (const assetName of allAssetNames()) {
      if (assets.some((asset) => asset.target === assetName)) {
        continue;
      }
      assets.push({
        source: assetName.startsWith("codewhale-tui")
          ? path.join(buildDir, isWindows ? "codewhale-tui.exe" : "codewhale-tui")
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

  manifestLines.sort();
  const manifestPath = path.join(outputDir, CHECKSUM_MANIFEST);
  await fs.writeFile(manifestPath, `${manifestLines.join("\n")}\n`, "utf8");

  console.log(`Prepared ${assets.length} assets in ${outputDir}`);
  console.log(`Wrote checksum manifest ${manifestPath}`);
}

main().catch((error) => {
  console.error("Failed to prepare local release assets:", error.message);
  process.exit(1);
});
