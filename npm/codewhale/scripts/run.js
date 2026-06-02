const { spawnSync } = require("child_process");
const { getBinaryPath } = require("./install");

const pkg = require("../package.json");

function isVersionFlag(args = process.argv.slice(2)) {
  return args.includes("--version") || args.includes("-V");
}

function printVersionFallback(binaryName) {
  const binVersion =
    pkg.codewhaleBinaryVersion || pkg.deepseekBinaryVersion || pkg.version;
  console.log(`${binaryName} (npm wrapper) v${pkg.version}`);
  console.log(`binary version: v${binVersion}`);
  console.log(`repo: ${pkg.repository?.url || "N/A"}`);
}

async function run(binaryName, options = {}) {
  const args = options.args || process.argv.slice(2);
  const resolveBinaryPath = options.getBinaryPath || getBinaryPath;
  const spawn = options.spawnSync || spawnSync;
  const exit = options.exit || process.exit;
  const versionFlag = isVersionFlag(args);

  let binaryPath;
  try {
    binaryPath = await resolveBinaryPath(binaryName);
  } catch (error) {
    if (versionFlag) {
      printVersionFallback(binaryName);
      return exit(0);
    }
    throw error;
  }

  const result = spawn(binaryPath, args, {
    stdio: "inherit",
  });
  if (result.error) {
    if (versionFlag) {
      printVersionFallback(binaryName);
      return exit(0);
    }
    throw result.error;
  }
  return exit(result.status ?? 1);
}

async function runCodeWhale() {
  await run("codewhale");
}

async function runCodeWhaleTui() {
  await run("codewhale-tui");
}

module.exports = {
  run,
  runCodeWhale,
  runCodeWhaleTui,
  _internal: { isVersionFlag, printVersionFallback },
};

if (require.main === module) {
  const command = process.argv[1] || "";
  if (command.includes("tui")) {
    runCodeWhaleTui();
  } else {
    runCodeWhale();
  }
}
