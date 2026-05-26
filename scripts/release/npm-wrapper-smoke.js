#!/usr/bin/env node

const fs = require("fs");
const fsp = require("fs/promises");
const http = require("http");
const os = require("os");
const path = require("path");
const { spawn } = require("child_process");

const repoRoot = path.resolve(__dirname, "..", "..");
const packageDir = path.join(repoRoot, "npm", "codewhale");
const prepareAssetsScript = path.join(
  repoRoot,
  "scripts",
  "release",
  "prepare-local-release-assets.js",
);

function shellQuote(value) {
  return /\s/.test(value) ? JSON.stringify(value) : value;
}

function usesWindowsCommandShim(command) {
  return process.platform === "win32" && (command === "npm" || command === "npx");
}

function runCommand(command, args, options = {}) {
  const cwd = options.cwd || repoRoot;
  console.log(`$ ${[command, ...args].map(shellQuote).join(" ")}`);
  const child = spawn(command, args, {
    cwd,
    env: {
      ...process.env,
      ...(options.env || {}),
    },
    encoding: "utf8",
    shell: usesWindowsCommandShim(command),
    stdio: options.capture ? ["ignore", "pipe", "pipe"] : "inherit",
    windowsHide: true,
  });

  if (!options.capture) {
    return new Promise((resolve, reject) => {
      child.once("error", reject);
      child.once("close", (status) => {
        if (status === 0) {
          resolve({ stdout: "", stderr: "" });
        } else {
          reject(new Error(`${command} exited with status ${status}`));
        }
      });
    });
  }

  let stdout = "";
  let stderr = "";
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk) => {
    stdout += chunk;
  });
  child.stderr.on("data", (chunk) => {
    stderr += chunk;
  });
  return new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("close", (status) => {
      if (status === 0) {
        resolve({ stdout, stderr });
        return;
      }
      process.stdout.write(stdout);
      process.stderr.write(stderr);
      reject(new Error(`${command} exited with status ${status}`));
    });
  });
}

function serveDirectory(root) {
  const server = http.createServer(async (request, response) => {
    try {
      const requestUrl = new URL(request.url || "/", "http://127.0.0.1");
      const decodedPath = decodeURIComponent(requestUrl.pathname);
      const filePath = path.resolve(root, `.${decodedPath}`);
      const relative = path.relative(root, filePath);
      if (relative.startsWith("..") || path.isAbsolute(relative)) {
        response.writeHead(403);
        response.end("forbidden");
        return;
      }

      const fileStat = await fsp.stat(filePath);
      if (!fileStat.isFile()) {
        response.writeHead(404);
        response.end("not found");
        return;
      }

      response.writeHead(200, {
        "Content-Length": fileStat.size,
        "Content-Type": "application/octet-stream",
      });
      fs.createReadStream(filePath).pipe(response);
    } catch (error) {
      response.writeHead(error && error.code === "ENOENT" ? 404 : 500);
      response.end(error && error.message ? error.message : "not found");
    }
  });

  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      resolve({
        baseUrl: `http://127.0.0.1:${address.port}/`,
        server,
      });
    });
  });
}

function parsePackJson(stdout) {
  const trimmed = stdout.trim();
  if (!trimmed) {
    throw new Error("npm pack did not return package metadata");
  }
  const parsed = JSON.parse(trimmed);
  const first = Array.isArray(parsed) ? parsed[0] : parsed;
  if (!first || !first.filename) {
    throw new Error(`npm pack metadata did not include a filename: ${trimmed}`);
  }
  return first.filename;
}

async function main() {
  const tempRoot = await fsp.mkdtemp(path.join(os.tmpdir(), "codewhale-npm-smoke-"));
  const releaseAssetsDir = path.join(tempRoot, "release-assets");
  const packDir = path.join(tempRoot, "pack");
  const installDir = path.join(tempRoot, "install");
  let keepTemp = process.env.DEEPSEEK_TUI_KEEP_SMOKE_DIR === "1";
  let server;

  try {
    await fsp.mkdir(packDir, { recursive: true });
    await fsp.mkdir(installDir, { recursive: true });

    await runCommand(process.execPath, [prepareAssetsScript, releaseAssetsDir]);
    const served = await serveDirectory(releaseAssetsDir);
    server = served.server;

    const env = {
      DEEPSEEK_TUI_FORCE_DOWNLOAD: "1",
      DEEPSEEK_TUI_RELEASE_BASE_URL: served.baseUrl,
    };
    const pack = await runCommand(
      "npm",
      ["pack", "--json", "--pack-destination", packDir],
      {
        capture: true,
        cwd: packageDir,
        env,
      },
    );
    const tarball = path.join(packDir, parsePackJson(pack.stdout));

    await runCommand("npm", ["init", "-y"], { cwd: installDir });
    await runCommand("npm", ["install", tarball], { cwd: installDir, env });
    await runCommand("npx", ["--no-install", "codewhale", "doctor", "--help"], {
      cwd: installDir,
      env,
    });
    await runCommand("npx", ["--no-install", "codewhale-tui", "--help"], {
      cwd: installDir,
      env,
    });

    console.log(`npm wrapper smoke passed with local assets from ${served.baseUrl}`);
  } catch (error) {
    keepTemp = true;
    console.error(`npm wrapper smoke failed: ${error.message}`);
    console.error(`Smoke workspace retained at ${tempRoot}`);
    process.exitCode = 1;
  } finally {
    if (server) {
      await new Promise((resolve) => server.close(resolve));
    }
    if (!keepTemp) {
      await fsp.rm(tempRoot, { force: true, recursive: true });
    }
  }
}

main();
