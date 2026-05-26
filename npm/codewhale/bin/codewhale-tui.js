#!/usr/bin/env node

const { runCodeWhaleTui } = require("../scripts/run");

runCodeWhaleTui().catch((error) => {
  console.error("Failed to start codewhale-tui:", error.message);
  process.exit(1);
});
