const assert = require("node:assert/strict");
const path = require("node:path");
const test = require("node:test");
const os = require("os");

const ARTIFACTS_PATH = path.join(__dirname, "..", "scripts", "artifacts.js");

function withMockedOs(platform, arch, fn) {
  const origPlatform = os.platform;
  const origArch = os.arch;
  os.platform = () => platform;
  os.arch = () => arch;
  delete require.cache[ARTIFACTS_PATH];
  try {
    return fn();
  } finally {
    os.platform = origPlatform;
    os.arch = origArch;
    delete require.cache[ARTIFACTS_PATH];
  }
}

test("openharmony x64 resolves to linux x64 binaries", () => {
  withMockedOs("openharmony", "x64", () => {
    const { detectBinaryNames } = require(ARTIFACTS_PATH);
    const result = detectBinaryNames();
    assert.equal(result.codewhale, "codewhale-linux-x64");
    assert.equal(result.tui, "codewhale-tui-linux-x64");
    assert.equal(result.codew, "codew-linux-x64");
  });
});

test("openharmony arm64 resolves to linux arm64 binaries", () => {
  withMockedOs("openharmony", "arm64", () => {
    const { detectBinaryNames } = require(ARTIFACTS_PATH);
    const result = detectBinaryNames();
    assert.equal(result.codewhale, "codewhale-linux-arm64");
    assert.equal(result.tui, "codewhale-tui-linux-arm64");
    assert.equal(result.codew, "codew-linux-arm64");
  });
});

test("android arm64 resolves to Termux-native Android assets", () => {
  withMockedOs("android", "arm64", () => {
    const { detectBinaryNames } = require(ARTIFACTS_PATH);
    const result = detectBinaryNames();
    assert.equal(result.codewhale, "codewhale-android-arm64");
    assert.equal(result.tui, "codewhale-tui-android-arm64");
    assert.equal(result.codew, "codew-android-arm64");
  });
});

test("genuinely unsupported platform throws with raw platform name", () => {
  withMockedOs("freebsd", "x64", () => {
    const { detectBinaryNames } = require(ARTIFACTS_PATH);
    assert.throws(
      () => detectBinaryNames(),
      (err) => {
        assert.match(err.message, /Unsupported platform: freebsd/);
        return true;
      },
    );
  });
});

test("known platforms are unaffected by alias map", () => {
  for (const [platform, arch, expectedCodeWhale] of [
    ["linux", "x64", "codewhale-linux-x64"],
    ["darwin", "arm64", "codewhale-macos-arm64"],
    ["win32", "x64", "codewhale-windows-x64.exe"],
  ]) {
    withMockedOs(platform, arch, () => {
      const { detectBinaryNames } = require(ARTIFACTS_PATH);
      const result = detectBinaryNames();
      assert.equal(result.codewhale, expectedCodeWhale);
    });
  }
});

test("linux riscv64 reports the temporary upstream binding blocker", () => {
  withMockedOs("linux", "riscv64", () => {
    const { detectBinaryNames } = require(ARTIFACTS_PATH);
    assert.throws(
      () => detectBinaryNames(),
      (err) => {
        assert.match(err.message, /Unsupported architecture: riscv64 on platform linux/);
        assert.match(err.message, /rquickjs-sys/);
        assert.match(err.message, /riscv64gc-unknown-linux-gnu/);
        return true;
      },
    );
  });
});

test("allAssetNames includes every matrix entry", () => {
  const { allAssetNames, allReleaseAssetNames } = require(ARTIFACTS_PATH);
  const assetNames = allAssetNames();
  assert.ok(assetNames.includes("codewhale-windows-x64.exe"));
  assert.ok(assetNames.includes("codewhale-tui-windows-x64.exe"));
  assert.ok(assetNames.includes("codew-windows-x64.exe"));
  assert.ok(assetNames.includes("codewhale.bat"));
  assert.ok(assetNames.includes("codewhale-android-arm64"));
  assert.ok(assetNames.includes("codewhale-tui-android-arm64"));
  assert.ok(assetNames.includes("codew-android-arm64"));
  assert.ok(!assetNames.includes("codewhale-linux-riscv64"));
  assert.ok(allReleaseAssetNames().includes("codew-windows-x64.exe"));
  assert.ok(allReleaseAssetNames().includes("codewhale.bat"));
  assert.ok(allReleaseAssetNames().includes("codew-android-arm64"));
});
