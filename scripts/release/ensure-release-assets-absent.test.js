#!/usr/bin/env node

const assert = require("node:assert/strict");
const test = require("node:test");

const {
  assertReleaseAssetsAbsent,
  fetchRelease,
  isNotFoundError,
  validateTarget,
} = require("./ensure-release-assets-absent");

test("release immutability guard accepts a missing release or empty inventory", () => {
  assert.doesNotThrow(() => assertReleaseAssetsAbsent(null, "v0.9.1"));
  assert.doesNotThrow(() => assertReleaseAssetsAbsent({ assets: [] }, "v0.9.1"));
});

test("release immutability guard refuses any existing public asset", () => {
  assert.throws(
    () =>
      assertReleaseAssetsAbsent(
        {
          assets: [
            { name: "codewhale-linux-x64" },
            { name: "codewhale-artifacts-sha256.txt" },
          ],
        },
        "v0.9.1",
      ),
    /Refusing to replace existing assets for v0\.9\.1: codewhale-linux-x64, codewhale-artifacts-sha256\.txt/,
  );
});

test("release immutability guard fails closed on malformed inventories", () => {
  assert.throws(
    () => assertReleaseAssetsAbsent({}, "v0.9.1"),
    /did not provide an asset inventory/,
  );
});

test("release lookup treats only an explicit GitHub 404 as absent", () => {
  const notFound = Object.assign(new Error("not found"), {
    status: 1,
    stderr: "gh: Not Found (HTTP 404)\n",
  });
  assert.equal(isNotFoundError(notFound), true);
  assert.equal(
    fetchRelease("Hmbown/CodeWhale", "v0.9.1", "gh", () => {
      throw notFound;
    }),
    null,
  );

  const forbidden = Object.assign(new Error("forbidden"), {
    status: 1,
    stderr: "gh: Resource not accessible (HTTP 403)\n",
  });
  assert.throws(
    () =>
      fetchRelease("Hmbown/CodeWhale", "v0.9.1", "gh", () => {
        throw forbidden;
      }),
    /Could not inspect GitHub Release v0\.9\.1.*HTTP 403/,
  );
});

test("release lookup parses the exact tag endpoint and validates targets", () => {
  let invocation;
  const release = fetchRelease("Hmbown/CodeWhale", "v0.9.1", "/fake/gh", (...args) => {
    invocation = args;
    return JSON.stringify({ assets: [] });
  });
  assert.deepEqual(release, { assets: [] });
  assert.equal(invocation[0], "/fake/gh");
  assert.deepEqual(invocation[1], ["api", "repos/Hmbown/CodeWhale/releases/tags/v0.9.1"]);
  assert.doesNotThrow(() => validateTarget("Hmbown/CodeWhale", "v0.9.1"));
  assert.doesNotThrow(() => validateTarget("Hmbown/CodeWhale", "v0.9.12-rc.1"));
  assert.throws(() => validateTarget("Hmbown/CodeWhale", "v0.9.12-rc.0"), /Invalid release tag/);
  assert.throws(() => validateTarget("Hmbown/CodeWhale", "v0.9.12-beta.1"), /Invalid release tag/);
  assert.throws(() => validateTarget("bad repo", "v0.9.1"), /Invalid GitHub repository/);
  assert.throws(() => validateTarget("Hmbown/CodeWhale", "main"), /Invalid release tag/);
});

test("release lookup rejects malformed successful API output", () => {
  assert.throws(
    () => fetchRelease("Hmbown/CodeWhale", "v0.9.1", "gh", () => "not-json"),
    /returned invalid JSON/,
  );
});
