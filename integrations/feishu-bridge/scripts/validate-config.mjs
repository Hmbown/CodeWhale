#!/usr/bin/env node
import { constants as fsConstants } from "node:fs";
import fs from "node:fs/promises";
import path from "node:path";

import {
  cleanEnvValue,
  formatValidationReport,
  parseEnvText,
  validateBridgeConfig
} from "../src/lib.mjs";

const args = parseArgs(process.argv.slice(2));

try {
  const bridgeEnv = args.env ? parseEnvText(await fs.readFile(args.env, "utf8")) : process.env;
  const runtimeEnv = args.runtimeEnv
    ? parseEnvText(await fs.readFile(args.runtimeEnv, "utf8"))
    : null;
  const result = validateBridgeConfig(bridgeEnv, {
    runtimeEnv,
    workspaceRoot: args.workspaceRoot || "/opt/whalebro"
  });

  if (args.checkFilesystem) {
    await appendFilesystemChecks(result, bridgeEnv, args);
  }

  if (args.json) {
    console.log(JSON.stringify(result, null, 2));
  } else {
    console.log(formatValidationReport(result));
  }
  process.exitCode = result.ok ? 0 : 1;
} catch (error) {
  console.error(`Config validation failed: ${error.message}`);
  process.exitCode = 1;
}

function parseArgs(argv) {
  const parsed = {
    env: "",
    runtimeEnv: "",
    workspaceRoot: "/opt/whalebro",
    checkFilesystem: false,
    json: false
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    switch (arg) {
      case "--env":
        parsed.env = argv[++index];
        break;
      case "--runtime-env":
        parsed.runtimeEnv = argv[++index];
        break;
      case "--workspace-root":
        parsed.workspaceRoot = argv[++index];
        break;
      case "--check-filesystem":
        parsed.checkFilesystem = true;
        break;
      case "--json":
        parsed.json = true;
        break;
      case "-h":
      case "--help":
        printHelp();
        process.exit(0);
        break;
      default:
        throw new Error(`unknown argument: ${arg}`);
    }
  }
  return parsed;
}

async function appendFilesystemChecks(result, env, args) {
  const workspace = cleanEnvValue(env.DEEPSEEK_WORKSPACE);
  if (workspace) {
    await checkReadableDirectory(result, workspace, "workspace");
  }

  const threadMapPath = cleanEnvValue(env.FEISHU_THREAD_MAP_PATH);
  if (threadMapPath) {
    const parent = path.dirname(threadMapPath);
    await checkWritableDirectory(result, parent, "thread map directory");
  }

  if (args.env) {
    await checkReadableFile(result, args.env, "bridge env file");
  }
  if (args.runtimeEnv) {
    await checkReadableFile(result, args.runtimeEnv, "runtime env file");
  }
}

async function checkReadableDirectory(result, dir, label) {
  try {
    const stat = await fs.stat(dir);
    if (!stat.isDirectory()) {
      result.errors.push({ code: "not_directory", message: `${label} is not a directory: ${dir}` });
      result.ok = false;
      return;
    }
    await fs.access(dir, fsConstants.R_OK | fsConstants.X_OK);
    result.info.push({ code: "readable_directory", message: `${label} is readable: ${dir}` });
  } catch (error) {
    result.errors.push({ code: "directory_access", message: `${label} is not readable: ${dir}` });
    result.ok = false;
  }
}

async function checkWritableDirectory(result, dir, label) {
  try {
    const stat = await fs.stat(dir);
    if (!stat.isDirectory()) {
      result.errors.push({ code: "not_directory", message: `${label} is not a directory: ${dir}` });
      result.ok = false;
      return;
    }
    await fs.access(dir, fsConstants.R_OK | fsConstants.W_OK | fsConstants.X_OK);
    result.info.push({ code: "writable_directory", message: `${label} is writable: ${dir}` });
  } catch {
    result.errors.push({ code: "directory_access", message: `${label} is not writable: ${dir}` });
    result.ok = false;
  }
}

async function checkReadableFile(result, filePath, label) {
  try {
    const stat = await fs.stat(filePath);
    if (!stat.isFile()) {
      result.errors.push({ code: "not_file", message: `${label} is not a file: ${filePath}` });
      result.ok = false;
      return;
    }
    await fs.access(filePath, fsConstants.R_OK);
    result.info.push({ code: "readable_file", message: `${label} is readable: ${filePath}` });
  } catch {
    result.errors.push({ code: "file_access", message: `${label} is not readable: ${filePath}` });
    result.ok = false;
  }
}

function printHelp() {
  console.log(`Usage: node scripts/validate-config.mjs [options]

Options:
  --env FILE             Read bridge env from FILE instead of process.env.
  --runtime-env FILE     Read runtime env and verify the shared bearer token.
  --workspace-root DIR   Expected remote workspace root (default: /opt/whalebro).
  --check-filesystem     Verify workspace and thread-map paths are usable.
  --json                 Print machine-readable JSON.
  -h, --help             Show this help.
`);
}
