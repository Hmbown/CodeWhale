const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");

const pkg = require("../package.json");
const {
  allReleaseAssetNames,
  BUNDLE_ASSET_NAMES,
  BUNDLE_CHECKSUM_MANIFEST,
  CHECKSUM_MANIFEST,
  checksummedReleaseAssetNames,
} = require("../scripts/artifacts");
const {
  assertChecksumManifestIncludes,
  assertPackageVersionMatchesBinaryVersion,
  assertReleaseAssetsFresh,
  parseChecksumManifest,
} = require("../scripts/verify-release-assets");

test("parseChecksumManifest accepts GNU and BSD filename forms", () => {
  const manifest = parseChecksumManifest(
    [
      "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  codewhale-linux-x64",
      "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb *codewhale-windows-x64.exe",
    ].join("\n"),
  );

  assert.equal(manifest.get("codewhale-linux-x64"), "a".repeat(64));
  assert.equal(manifest.get("codewhale-windows-x64.exe"), "b".repeat(64));
});

test("parseChecksumManifest rejects malformed checksum rows", () => {
  assert.throws(
    () => parseChecksumManifest("not-a-sha  codewhale-linux-x64"),
    /Invalid checksum manifest line/,
  );
});

test("assertReleaseAssetsFresh rejects missing release assets", () => {
  assert.throws(
    () =>
      assertReleaseAssetsFresh(
        { assets: [{ name: "codewhale-linux-x64", state: "uploaded", updated_at: "2026-06-26T00:10:00Z" }] },
        ["codewhale-linux-x64", "codewhale-artifacts-sha256.txt"],
        { database_id: 123, created_at: "2026-06-26T00:00:00Z" },
      ),
    /missing required release asset/,
  );
});

test("assertChecksumManifestIncludes rejects missing bundle manifest and archive rows", () => {
  const manifest = parseChecksumManifest(
    `${"a".repeat(64)}  codewhale-linux-x64.tar.gz`,
  );

  assert.throws(
    () =>
      assertChecksumManifestIncludes(
        manifest,
        ["codewhale-linux-x64.tar.gz", "codewhale-bundles-sha256.txt"],
        "Canonical checksum manifest",
      ),
    /Canonical checksum manifest is missing codewhale-bundles-sha256\.txt/,
  );
});

test("bundle checksum rows use public archive basenames", () => {
  const manifest = parseChecksumManifest(
    `${"a".repeat(64)}  bundles/codewhale-linux-x64.tar.gz`,
  );

  assert.throws(
    () =>
      assertChecksumManifestIncludes(
        manifest,
        ["codewhale-linux-x64.tar.gz"],
        "Bundle checksum manifest",
      ),
    /Bundle checksum manifest is missing codewhale-linux-x64\.tar\.gz/,
  );
});

test("assertReleaseAssetsFresh rejects assets older than the release workflow run", () => {
  assert.throws(
    () =>
      assertReleaseAssetsFresh(
        { assets: [{ name: "codewhale-linux-x64", state: "uploaded", updated_at: "2026-06-25T23:59:59Z" }] },
        ["codewhale-linux-x64"],
        { database_id: 123, created_at: "2026-06-26T00:00:00Z" },
      ),
    /asset set is stale/,
  );
});

test("assertReleaseAssetsFresh rejects non-uploaded assets", () => {
  assert.throws(
    () =>
      assertReleaseAssetsFresh(
        { assets: [{ name: "codewhale-linux-x64", state: "new", updated_at: "2026-06-26T00:10:00Z" }] },
        ["codewhale-linux-x64"],
        { database_id: 123, created_at: "2026-06-26T00:00:00Z" },
      ),
    /asset set is stale/,
  );
});

test("assertReleaseAssetsFresh accepts assets updated by the release workflow run", () => {
  assert.doesNotThrow(() =>
    assertReleaseAssetsFresh(
      { assets: [{ name: "codewhale-linux-x64", state: "uploaded", updated_at: "2026-06-26T00:10:00Z" }] },
      ["codewhale-linux-x64"],
      { database_id: 123, created_at: "2026-06-26T00:00:00Z" },
    ),
  );
});

test("assertPackageVersionMatchesBinaryVersion allows packaging-only releases only with an explicit override", () => {
  assert.doesNotThrow(() => assertPackageVersionMatchesBinaryVersion(pkg.version));
  assert.throws(
    () => assertPackageVersionMatchesBinaryVersion("0.0.0-packaging-test"),
    /does not match codewhaleBinaryVersion/,
  );

  const previous = process.env.CODEWHALE_ALLOW_NPM_BINARY_MISMATCH;
  process.env.CODEWHALE_ALLOW_NPM_BINARY_MISMATCH = "1";
  try {
    assert.doesNotThrow(() => assertPackageVersionMatchesBinaryVersion("0.0.0-packaging-test"));
  } finally {
    if (previous === undefined) {
      delete process.env.CODEWHALE_ALLOW_NPM_BINARY_MISMATCH;
    } else {
      process.env.CODEWHALE_ALLOW_NPM_BINARY_MISMATCH = previous;
    }
  }
});

test("npm publication requires the checkout guard and canonical release-asset gate", () => {
  assert.equal(
    pkg.scripts.prepublishOnly,
    "bash ../../scripts/release/require-release-tag-checkout.sh && " +
      "bash ../../scripts/release/verify-release-assets.sh",
  );
});

test("full local release fixture satisfies the public asset inventory", () => {
  const repoRoot = path.resolve(__dirname, "..", "..", "..");
  const fixtureRoot = fs.mkdtempSync(path.join(os.tmpdir(), "codewhale-assets-"));
  const buildDir = path.join(fixtureRoot, "build");
  const outputDir = path.join(fixtureRoot, "assets");
  const executableSuffix = process.platform === "win32" ? ".exe" : "";

  try {
    fs.mkdirSync(buildDir, { recursive: true });
    for (const binary of ["codewhale", "codew", "codewhale-tui"]) {
      fs.writeFileSync(
        path.join(buildDir, `${binary}${executableSuffix}`),
        `fixture:${binary}\n`,
      );
    }

    execFileSync(
      process.execPath,
      [
        path.join(repoRoot, "scripts", "release", "prepare-local-release-assets.js"),
        outputDir,
        buildDir,
      ],
      {
        env: { ...process.env, DEEPSEEK_TUI_PREPARE_ALL_ASSETS: "1" },
        stdio: "pipe",
      },
    );

    for (const assetName of allReleaseAssetNames()) {
      assert.equal(
        fs.existsSync(path.join(outputDir, assetName)),
        true,
        `missing fixture asset ${assetName}`,
      );
    }

    const canonicalChecksums = parseChecksumManifest(
      fs.readFileSync(path.join(outputDir, CHECKSUM_MANIFEST), "utf8"),
    );
    assert.doesNotThrow(() =>
      assertChecksumManifestIncludes(
        canonicalChecksums,
        checksummedReleaseAssetNames(),
        "Canonical checksum manifest",
      ),
    );

    const bundleChecksums = parseChecksumManifest(
      fs.readFileSync(path.join(outputDir, BUNDLE_CHECKSUM_MANIFEST), "utf8"),
    );
    assert.doesNotThrow(() =>
      assertChecksumManifestIncludes(
        bundleChecksums,
        BUNDLE_ASSET_NAMES,
        "Bundle checksum manifest",
      ),
    );
  } finally {
    fs.rmSync(fixtureRoot, { recursive: true, force: true });
  }
});
