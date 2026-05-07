const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

const installScript = fs.readFileSync(
  path.join(__dirname, "..", "scripts", "install.js"),
  "utf8",
);

test("install script checks Node support before loading helpers", () => {
  const guardIndex = installScript.indexOf("assertSupportedNode();");
  const firstRequireIndex = installScript.indexOf("require(");

  assert.notEqual(guardIndex, -1);
  assert.notEqual(firstRequireIndex, -1);
  assert.ok(guardIndex < firstRequireIndex);
});

test("install script remains parseable before the Node support guard runs", () => {
  assert.equal(installScript.includes("??"), false);
  assert.equal(installScript.includes("?."), false);
});
