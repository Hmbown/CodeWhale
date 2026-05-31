#!/usr/bin/env node

const { run } = require("../scripts/run");

run("codewhale").catch((error) => {
  console.error("Failed to start codewhale:", error.message);
  process.exit(1);
});
