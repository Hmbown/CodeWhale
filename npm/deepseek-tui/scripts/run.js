const { spawnSync } = require("child_process");
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

async function run(binaryName) {
  // Intercept --version before attempting binary download/launch
  handleVersionFallback(binaryName);

  const binaryPath = await getBinaryPath(binaryName);
  const args = process.argv.slice(2);

  // On Windows, .cmd wrappers strip quotes from arguments. Use execSync
  // with manual quoting to preserve multi-word prompts. See issue #1101.
  if (process.platform === "win32") {
    const { execSync } = require("child_process");
    const quotedArgs = args.map((arg) => {
      // Quote arguments that contain spaces and aren't already quoted
      if (arg.includes(" ") && !arg.startsWith('"') && !arg.startsWith("'")) {
        return `"${arg.replace(/"/g, '\\"')}"`;
      }
      return arg;
    });
    const fullCmd = `"${binaryPath}" ${quotedArgs.join(" ")}`;
    try {
      execSync(fullCmd, { stdio: "inherit" });
      process.exit(0);
    } catch (err) {
      process.exit(err.status ?? 1);
    }
  }

  const result = spawnSync(binaryPath, args, {
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
