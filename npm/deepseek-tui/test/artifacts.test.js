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
    assert.equal(result.deepseek, "deepseek-linux-x64");
    assert.equal(result.tui, "deepseek-tui-linux-x64");
  });
});

test("openharmony arm64 resolves to linux arm64 binaries", () => {
  withMockedOs("openharmony", "arm64", () => {
    const { detectBinaryNames } = require(ARTIFACTS_PATH);
    const result = detectBinaryNames();
    assert.equal(result.deepseek, "deepseek-linux-arm64");
    assert.equal(result.tui, "deepseek-tui-linux-arm64");
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
  for (const [platform, arch, expectedDeepseek] of [
    ["linux", "x64", "deepseek-linux-x64"],
    ["darwin", "arm64", "deepseek-macos-arm64"],
    ["win32", "x64", "deepseek-windows-x64.exe"],
  ]) {
    withMockedOs(platform, arch, () => {
      const { detectBinaryNames } = require(ARTIFACTS_PATH);
      const result = detectBinaryNames();
      assert.equal(result.deepseek, expectedDeepseek);
    });
  }
});
