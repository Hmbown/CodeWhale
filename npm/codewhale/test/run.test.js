const assert = require("node:assert/strict");
const test = require("node:test");

const { _internal } = require("../scripts/run");

test("version fallback handles only version flags", () => {
  assert.equal(_internal.isVersionFlag(["--version"]), true);
  assert.equal(_internal.isVersionFlag(["-V"]), true);
  assert.equal(_internal.isVersionFlag(["-v"]), false);
  assert.equal(_internal.isVersionFlag(["--verbose"]), false);
});
