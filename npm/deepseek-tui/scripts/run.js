const { spawnSync } = require("child_process");
const path = require("path");
const fs = require("fs");
const { getBinaryPath } = require("./install");

const pkg = require("../package.json");

function isVersionFlag(args = process.argv.slice(2)) {
  return args.includes("--version") || args.includes("-V");
}

function handleVersionFallback(binaryName) {
  if (isVersionFlag()) {
    const binVersion = pkg.deepseekBinaryVersion || pkg.version;
    console.log(`${binaryName} (npm wrapper) v${pkg.version}`);
    console.log(`binary version: v${binVersion}`);
    console.log(`repo: ${pkg.repository?.url || "N/A"}`);
    process.exit(0);
  }
}

// On macOS, find the bundled terminal-notifier.app binary so the Rust
// native_notify() code path can use it for click-to-focus notifications.
// terminal-notifier is an optional dependency — if missing (e.g. Cargo
// install), the Rust side falls back to plain osascript.
function prependTerminalNotifierToPath() {
  if (process.platform !== "darwin") return;
  const candidates = [
    // npm < 9 / older layout: binary lives in terminal-notifier/vendor/
    path.resolve(__dirname, "..", "node_modules", "terminal-notifier", "vendor", "terminal-notifier.app", "Contents", "MacOS"),
    // npm 9+ / workspaces flatten: .bin/ symlinks to terminal-notifier
    path.resolve(__dirname, "..", "node_modules", ".bin"),
  ];
  for (const dir of candidates) {
    if (fs.existsSync(dir)) {
      process.env.PATH = `${dir}${path.delimiter}${process.env.PATH || ""}`;
      return;
    }
  }
}

async function run(binaryName) {
  // Intercept --version before attempting binary download/launch
  handleVersionFallback(binaryName);

  prependTerminalNotifierToPath();

  const binaryPath = await getBinaryPath(binaryName);
  const result = spawnSync(binaryPath, process.argv.slice(2), {
    stdio: "inherit",
  });
  if (result.error) {
    // If binary fails and user asked for --version, show npm version instead
    handleVersionFallback(binaryName);
    throw result.error;
  }
  process.exit(result.status ?? 1);
}

async function runDeepseek() {
  await run("deepseek");
}

async function runDeepseekTui() {
  await run("deepseek-tui");
}

module.exports = {
  run,
  runDeepseek,
  runDeepseekTui,
  _internal: { isVersionFlag },
};

if (require.main === module) {
  const command = process.argv[1] || "";
  if (command.includes("tui")) {
    runDeepseekTui();
  } else {
    runDeepseek();
  }
}
