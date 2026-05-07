function assertSupportedNode() {
  const version = process.versions && process.versions.node ? process.versions.node : "unknown";
  const major = Number.parseInt(String(version).split(".")[0], 10);
  if (Number.isNaN(major) || major < 18) {
    process.stderr.write(
      "deepseek-tui: Node.js 18 or newer is required for npm installation. " +
      `Current Node.js version is ${version}. ` +
      "Please upgrade Node.js and rerun `npm install -g deepseek-tui`.\n",
    );
    process.exit(1);
  }
}

assertSupportedNode();

const fs = require("fs");
const https = require("https");
const http = require("http");
const net = require("net");
const tls = require("tls");
const crypto = require("crypto");
const { URL } = require("url");
const { mkdir, chmod, stat, rename, readFile, unlink, writeFile } = fs.promises;
const { createWriteStream } = fs;
const path = require("path");

const {
  checksumManifestUrl,
  detectBinaryNames,
  releaseAssetUrl,
  releaseBinaryDirectory,
} = require("./artifacts");
const { preflightGlibc } = require("./preflight-glibc");
const pkg = require("../package.json");

const DEFAULT_TIMEOUT_MS = 300_000; // 5 minutes per attempt
const DEFAULT_STALL_MS = 30_000; // abort if no bytes for 30s
const MAX_ATTEMPTS = 5;
const BASE_BACKOFF_MS = 1_000;

const RETRYABLE_NET_CODES = new Set([
  "ECONNRESET",
  "ECONNREFUSED",
  "ETIMEDOUT",
  "EAI_AGAIN",
  "ENETUNREACH",
  "EHOSTUNREACH",
  "EPIPE",
  "ECONNABORTED",
]);

class NonRetryableError extends Error {
  constructor(message) {
    super(message);
    this.name = "NonRetryableError";
    this.nonRetryable = true;
  }
}

class HttpStatusError extends Error {
  constructor(status, url) {
    super(`Request failed with status ${status}: ${url}`);
    this.name = "HttpStatusError";
    this.status = status;
  }
}

class DownloadTimeoutError extends Error {
  constructor(message) {
    super(message);
    this.name = "DownloadTimeoutError";
    this.code = "EDOWNLOADTIMEOUT";
  }
}

function resolvePackageVersion() {
  const configuredVersion =
    process.env.DEEPSEEK_TUI_VERSION ||
    process.env.DEEPSEEK_VERSION ||
    pkg.deepseekBinaryVersion ||
    pkg.version;
  return String(configuredVersion).trim();
}

function resolveRepo() {
  return process.env.DEEPSEEK_TUI_GITHUB_REPO || process.env.DEEPSEEK_GITHUB_REPO || "Hmbown/DeepSeek-TUI";
}

function binaryPaths() {
  const { deepseek, tui } = detectBinaryNames();
  const releaseDir = releaseBinaryDirectory();
  return {
    deepseek: {
      asset: deepseek,
      target: path.join(releaseDir, process.platform === "win32" ? "deepseek.exe" : "deepseek"),
    },
    tui: {
      asset: tui,
      target: path.join(releaseDir, process.platform === "win32" ? "deepseek-tui.exe" : "deepseek-tui"),
    },
  };
}

// ────────────────────────────────────────────────────────────────────────────
// Logging / progress
// ────────────────────────────────────────────────────────────────────────────

function isQuietInstall() {
  if (process.env.DEEPSEEK_TUI_QUIET_INSTALL === "1") {
    return true;
  }
  const level = (process.env.npm_config_loglevel || "").toLowerCase();
  return level === "silent" || level === "error";
}

function logInfo(message) {
  if (isQuietInstall()) {
    return;
  }
  process.stderr.write(`deepseek-tui: ${message}\n`);
}

function envInt(name, fallback) {
  const raw = process.env[name];
  if (!raw) {
    return fallback;
  }
  const parsed = Number.parseInt(String(raw).trim(), 10);
  if (!Number.isFinite(parsed) || parsed <= 0) {
    return fallback;
  }
  return parsed;
}

function downloadTimeoutMs() {
  return envInt(
    "DEEPSEEK_TUI_DOWNLOAD_TIMEOUT_MS",
    envInt("DEEPSEEK_DOWNLOAD_TIMEOUT_MS", DEFAULT_TIMEOUT_MS),
  );
}

function downloadStallMs() {
  return envInt(
    "DEEPSEEK_TUI_DOWNLOAD_STALL_MS",
    envInt("DEEPSEEK_DOWNLOAD_STALL_MS", DEFAULT_STALL_MS),
  );
}

function formatMb(bytes) {
  return (bytes / (1024 * 1024)).toFixed(0);
}

function createProgressReporter(assetName, totalBytes) {
  if (isQuietInstall()) {
    return { onChunk: () => {}, finish: () => {} };
  }
  const isTty = !!process.stderr.isTTY;
  const interactive = isTty;
  const tickBytes = interactive ? 1 * 1024 * 1024 : 5 * 1024 * 1024;
  const tickMs = 2_000;

  let received = 0;
  let lastBytesPrinted = 0;
  let lastTimePrinted = 0;
  let everPrinted = false;

  const render = (final) => {
    if (totalBytes && totalBytes > 0) {
      const pct = Math.min(100, Math.round((received / totalBytes) * 100));
      const line = `deepseek-tui: downloading ${assetName}: ${formatMb(received)} / ${formatMb(totalBytes)} MB (${pct}%)`;
      if (interactive) {
        process.stderr.write(`${line}\r`);
      } else {
        process.stderr.write(`${line}\n`);
      }
    } else {
      const line = `deepseek-tui: downloading ${assetName}: ${formatMb(received)} MB downloaded`;
      if (interactive) {
        process.stderr.write(`${line}\r`);
      } else {
        process.stderr.write(`${line}\n`);
      }
    }
    everPrinted = true;
    lastBytesPrinted = received;
    lastTimePrinted = Date.now();
  };

  return {
    onChunk(chunkLen) {
      received += chunkLen;
      const now = Date.now();
      if (
        received - lastBytesPrinted >= tickBytes ||
        (interactive && now - lastTimePrinted >= tickMs)
      ) {
        render(false);
      }
    },
    finish() {
      // Final line — always render once.
      render(true);
      if (interactive && everPrinted) {
        // Move past the carriage-return line and emit a "done" footer.
        process.stderr.write("\n");
      }
      process.stderr.write(`deepseek-tui: ${assetName} ... done.\n`);
    },
  };
}

// ────────────────────────────────────────────────────────────────────────────
// Proxy support (HTTPS_PROXY / HTTP_PROXY / NO_PROXY) — pure Node, CONNECT
// tunnel + TLS upgrade for HTTPS targets.
// ────────────────────────────────────────────────────────────────────────────

function getProxyUrl(targetUrl) {
  const isHttps = targetUrl.protocol === "https:";
  const candidates = isHttps
    ? ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"]
    : ["HTTP_PROXY", "http_proxy"];
  for (const name of candidates) {
    const raw = process.env[name];
    if (raw && String(raw).trim() !== "") {
      return String(raw).trim();
    }
  }
  return null;
}

function shouldBypassProxy(host) {
  const raw = process.env.NO_PROXY || process.env.no_proxy;
  if (!raw) {
    return false;
  }
  const lower = String(host).toLowerCase();
  for (const part of String(raw).split(",")) {
    const entry = part.trim().toLowerCase();
    if (!entry) {
      continue;
    }
    if (entry === "*") {
      return true;
    }
    // Strip leading dot and any explicit port.
    const stripped = entry.replace(/^\./, "").replace(/:.*$/, "");
    if (!stripped) {
      continue;
    }
    if (lower === stripped || lower.endsWith(`.${stripped}`)) {
      return true;
    }
  }
  return false;
}

function parseProxy(proxyStr) {
  // Accept "http://user:pass@host:port" and bare "host:port".
  const normalized = /^[a-z][a-z0-9+\-.]*:\/\//i.test(proxyStr)
    ? proxyStr
    : `http://${proxyStr}`;
  const u = new URL(normalized);
  const port = u.port
    ? Number.parseInt(u.port, 10)
    : u.protocol === "https:"
      ? 443
      : 80;
  let auth = null;
  if (u.username) {
    const user = decodeURIComponent(u.username);
    const pass = u.password ? decodeURIComponent(u.password) : "";
    auth = Buffer.from(`${user}:${pass}`).toString("base64");
  }
  return {
    protocol: u.protocol,
    host: u.hostname,
    port,
    auth,
    raw: proxyStr,
  };
}

function connectThroughProxy(proxy, targetHost, targetPort, timeoutMs) {
  return new Promise((resolve, reject) => {
    const socket = net.connect({ host: proxy.host, port: proxy.port });
    let settled = false;
    const fail = (err) => {
      if (settled) return;
      settled = true;
      try {
        socket.destroy();
      } catch {
        // ignore
      }
      reject(err);
    };

    const timer = timeoutMs > 0
      ? setTimeout(() => fail(new DownloadTimeoutError(
          `proxy CONNECT to ${proxy.host}:${proxy.port} timed out after ${timeoutMs} ms`,
        )), timeoutMs)
      : null;

    socket.once("error", (err) => {
      if (timer) clearTimeout(timer);
      // Surface proxy host so the user can fix it.
      const wrapped = new Error(
        `proxy connection failed (${proxy.host}:${proxy.port}): ${err.message}`,
      );
      wrapped.code = err.code;
      fail(wrapped);
    });

    socket.once("connect", () => {
      const lines = [
        `CONNECT ${targetHost}:${targetPort} HTTP/1.1`,
        `Host: ${targetHost}:${targetPort}`,
        "User-Agent: deepseek-tui-installer",
        "Proxy-Connection: keep-alive",
      ];
      if (proxy.auth) {
        lines.push(`Proxy-Authorization: Basic ${proxy.auth}`);
      }
      const req = `${lines.join("\r\n")}\r\n\r\n`;

      let buf = Buffer.alloc(0);
      const onData = (chunk) => {
        buf = Buffer.concat([buf, chunk]);
        const idx = buf.indexOf("\r\n\r\n");
        if (idx === -1) {
          if (buf.length > 16 * 1024) {
            socket.removeListener("data", onData);
            fail(new Error(
              `proxy ${proxy.host}:${proxy.port} returned an oversized response header`,
            ));
          }
          return;
        }
        socket.removeListener("data", onData);
        const head = buf.slice(0, idx).toString("utf8");
        const firstLine = head.split(/\r?\n/, 1)[0] || "";
        const m = firstLine.match(/^HTTP\/\d\.\d\s+(\d{3})/);
        if (!m) {
          fail(new Error(`proxy ${proxy.host}:${proxy.port} returned invalid CONNECT reply: ${firstLine}`));
          return;
        }
        const code = Number.parseInt(m[1], 10);
        if (code !== 200) {
          fail(new Error(
            `proxy ${proxy.host}:${proxy.port} refused CONNECT to ${targetHost}:${targetPort}: HTTP ${code}`,
          ));
          return;
        }
        if (timer) clearTimeout(timer);
        if (settled) return;
        settled = true;
        // Any bytes past the header belong to the tunneled stream — but in
        // practice CONNECT 200 has no body; if it did, we'd lose those bytes
        // here. Keep it simple: trust well-behaved proxies.
        resolve(socket);
      };
      socket.on("data", onData);
      socket.write(req, "utf8");
    });
  });
}

// ────────────────────────────────────────────────────────────────────────────
// HTTP request with timeout, stall detection, and proxy support.
// ────────────────────────────────────────────────────────────────────────────

function httpRequest(rawUrl, opts = {}) {
  const totalTimeoutMs =
    opts.totalTimeoutMs === undefined || opts.totalTimeoutMs === null
      ? downloadTimeoutMs()
      : opts.totalTimeoutMs;
  const stallMs =
    opts.stallMs === undefined || opts.stallMs === null
      ? downloadStallMs()
      : opts.stallMs;

  return new Promise((resolve, reject) => {
    let url;
    try {
      url = new URL(rawUrl);
    } catch (err) {
      reject(new NonRetryableError(`Invalid URL: ${rawUrl} (${err.message})`));
      return;
    }
    if (url.protocol !== "https:" && url.protocol !== "http:") {
      reject(new NonRetryableError(`Unsupported protocol: ${url.protocol}`));
      return;
    }

    const proxyStr = !shouldBypassProxy(url.hostname) ? getProxyUrl(url) : null;
    const isHttps = url.protocol === "https:";
    const port = url.port
      ? Number.parseInt(url.port, 10)
      : isHttps
        ? 443
        : 80;

    let totalTimer = null;
    let stallTimer = null;
    let settled = false;
    let req = null;
    let res = null;

    const cleanup = () => {
      if (totalTimer) {
        clearTimeout(totalTimer);
        totalTimer = null;
      }
      if (stallTimer) {
        clearTimeout(stallTimer);
        stallTimer = null;
      }
    };

    const fail = (err) => {
      if (settled) return;
      settled = true;
      cleanup();
      try {
        if (req && !req.destroyed) req.destroy();
      } catch {
        // ignore
      }
      try {
        if (res && !res.destroyed) res.destroy();
      } catch {
        // ignore
      }
      reject(err);
    };

    if (totalTimeoutMs > 0) {
      totalTimer = setTimeout(() => {
        fail(new DownloadTimeoutError(
          `download exceeded total timeout of ${totalTimeoutMs} ms ` +
          `(set DEEPSEEK_TUI_DOWNLOAD_TIMEOUT_MS to raise it; current stall budget is ${stallMs} ms)`,
        ));
      }, totalTimeoutMs);
    }

    const armStallTimer = () => {
      if (stallMs <= 0) return;
      if (stallTimer) clearTimeout(stallTimer);
      stallTimer = setTimeout(() => {
        fail(new DownloadTimeoutError(
          `download stalled — no bytes received for ${stallMs} ms ` +
          `(set DEEPSEEK_TUI_DOWNLOAD_STALL_MS to raise it; total budget is ${totalTimeoutMs} ms)`,
        ));
      }, stallMs);
    };

    const launch = (socket) => {
      const reqOptions = {
        method: "GET",
        host: url.hostname,
        port,
        path: `${url.pathname}${url.search || ""}`,
        headers: {
          Host: url.host,
          "User-Agent": "deepseek-tui-installer",
          Accept: "*/*",
          Connection: "close",
        },
      };
      if (socket) {
        reqOptions.createConnection = () => socket;
        if (isHttps) {
          // Wrap raw TCP socket from CONNECT in TLS.
          const tlsSocket = tls.connect({
            socket,
            servername: url.hostname,
            ALPNProtocols: ["http/1.1"],
          });
          tlsSocket.once("error", (err) => fail(err));
          reqOptions.createConnection = () => tlsSocket;
        }
      }
      const client = isHttps ? https : http;
      try {
        req = client.request(reqOptions, (response) => {
          res = response;
          armStallTimer();
          response.on("data", () => {
            armStallTimer();
          });
          response.on("end", () => {
            cleanup();
          });
          response.on("error", (err) => fail(err));

          const status = response.statusCode || 0;
          if (status >= 300 && status < 400 && response.headers.location) {
            cleanup();
            settled = true;
            response.resume();
            resolve({ redirect: response.headers.location, response: null });
            return;
          }
          if (status < 200 || status >= 300) {
            const err = new HttpStatusError(status, rawUrl);
            // 4xx: non-retryable; 5xx: retryable.
            if (status >= 400 && status < 500) {
              err.nonRetryable = true;
            }
            fail(err);
            return;
          }
          if (settled) return;
          settled = true;
          // Hand the live response stream to the caller.
          resolve({ redirect: null, response });
        });
        req.once("error", (err) => fail(err));
        req.once("socket", (s) => {
          // Belt-and-suspenders: surface socket-level errors quickly.
          s.once("error", (err) => fail(err));
        });
        req.end();
      } catch (err) {
        fail(err);
      }
    };

    if (proxyStr) {
      let proxy;
      try {
        proxy = parseProxy(proxyStr);
      } catch (err) {
        fail(new NonRetryableError(
          `Invalid proxy URL "${proxyStr}": ${err.message}`,
        ));
        return;
      }
      if (!isHttps) {
        // Plain HTTP through proxy — send absolute URI, no CONNECT.
        const client = http;
        try {
          req = client.request(
            {
              host: proxy.host,
              port: proxy.port,
              method: "GET",
              path: rawUrl,
              headers: {
                Host: url.host,
                "User-Agent": "deepseek-tui-installer",
                Accept: "*/*",
                Connection: "close",
                ...(proxy.auth ? { "Proxy-Authorization": `Basic ${proxy.auth}` } : {}),
              },
            },
            (response) => {
              res = response;
              armStallTimer();
              response.on("data", () => armStallTimer());
              response.on("end", () => cleanup());
              response.on("error", (err) => fail(err));
              const status = response.statusCode || 0;
              if (status >= 300 && status < 400 && response.headers.location) {
                cleanup();
                settled = true;
                response.resume();
                resolve({ redirect: response.headers.location, response: null });
                return;
              }
              if (status < 200 || status >= 300) {
                const err = new HttpStatusError(status, rawUrl);
                if (status >= 400 && status < 500) err.nonRetryable = true;
                fail(err);
                return;
              }
              if (settled) return;
              settled = true;
              resolve({ redirect: null, response });
            },
          );
          req.once("error", (err) => fail(err));
          req.end();
        } catch (err) {
          fail(err);
        }
        return;
      }

      // HTTPS through proxy: CONNECT tunnel + TLS upgrade.
      connectThroughProxy(proxy, url.hostname, port, Math.max(stallMs, 5_000))
        .then((tcpSocket) => {
          if (settled) {
            try { tcpSocket.destroy(); } catch { /* ignore */ }
            return;
          }
          const tlsSocket = tls.connect({
            socket: tcpSocket,
            servername: url.hostname,
            ALPNProtocols: ["http/1.1"],
          });
          tlsSocket.once("error", (err) => fail(err));
          tlsSocket.once("secureConnect", () => {
            if (settled) {
              try { tlsSocket.destroy(); } catch { /* ignore */ }
              return;
            }
            const reqOptions = {
              method: "GET",
              createConnection: () => tlsSocket,
              path: `${url.pathname}${url.search || ""}`,
              headers: {
                Host: url.host,
                "User-Agent": "deepseek-tui-installer",
                Accept: "*/*",
                Connection: "close",
              },
            };
            try {
              req = https.request(reqOptions, (response) => {
                res = response;
                armStallTimer();
                response.on("data", () => armStallTimer());
                response.on("end", () => cleanup());
                response.on("error", (err) => fail(err));
                const status = response.statusCode || 0;
                if (status >= 300 && status < 400 && response.headers.location) {
                  cleanup();
                  settled = true;
                  response.resume();
                  resolve({ redirect: response.headers.location, response: null });
                  return;
                }
                if (status < 200 || status >= 300) {
                  const err = new HttpStatusError(status, rawUrl);
                  if (status >= 400 && status < 500) err.nonRetryable = true;
                  fail(err);
                  return;
                }
                if (settled) return;
                settled = true;
                resolve({ redirect: null, response });
              });
              req.once("error", (err) => fail(err));
              req.end();
            } catch (err) {
              fail(err);
            }
          });
        })
        .catch((err) => fail(err));
      return;
    }

    // No proxy — direct connection.
    launch(null);
  });
}

// ────────────────────────────────────────────────────────────────────────────
// Retry wrapper
// ────────────────────────────────────────────────────────────────────────────

function isRetryable(err) {
  if (!err) return false;
  if (err.nonRetryable) return false;
  if (err instanceof NonRetryableError) return false;
  if (err instanceof DownloadTimeoutError) return true;
  if (err instanceof HttpStatusError) {
    return err.status >= 500;
  }
  if (err.code && RETRYABLE_NET_CODES.has(err.code)) return true;
  // Network-flavored messages we may see without a code.
  const msg = String(err.message || "").toLowerCase();
  if (msg.includes("network") && msg.includes("unreachable")) return true;
  if (msg.includes("socket hang up")) return true;
  if (msg.includes("aborted")) return true;
  return false;
}

function backoffDelay(attempt) {
  // attempt is 1-indexed; first retry waits ~1s.
  const base = BASE_BACKOFF_MS * 2 ** (attempt - 1);
  const jitter = (Math.random() * 0.4 - 0.2) * base; // ±20%
  return Math.max(0, Math.round(base + jitter));
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function withRetry(label, fn) {
  let lastErr;
  for (let attempt = 1; attempt <= MAX_ATTEMPTS; attempt++) {
    try {
      return await fn(attempt);
    } catch (err) {
      lastErr = err;
      if (!isRetryable(err) || attempt === MAX_ATTEMPTS) {
        break;
      }
      const wait = backoffDelay(attempt);
      logInfo(
        `${label} failed (attempt ${attempt}/${MAX_ATTEMPTS}): ${err.message}; retrying in ${wait} ms`,
      );
      await sleep(wait);
    }
  }
  const msg = lastErr && lastErr.message ? lastErr.message : String(lastErr);
  const wrapped = new Error(
    `${label} failed after ${MAX_ATTEMPTS} attempt(s): ${msg}`,
  );
  if (lastErr && lastErr.stack) {
    wrapped.cause = lastErr;
  }
  throw wrapped;
}

// ────────────────────────────────────────────────────────────────────────────
// Public download primitives (now retry + progress aware)
// ────────────────────────────────────────────────────────────────────────────

async function followRedirects(url, opts) {
  const maxRedirects = 10;
  let current = url;
  for (let hop = 0; hop < maxRedirects; hop++) {
    const result = await httpRequest(current, opts);
    if (result.redirect) {
      try {
        current = new URL(result.redirect, current).toString();
      } catch {
        current = result.redirect;
      }
      continue;
    }
    return result;
  }
  throw new NonRetryableError(`too many redirects starting at ${url}`);
}

function streamToFile(response, destination, progress) {
  return new Promise((resolve, reject) => {
    const sink = createWriteStream(destination);
    let done = false;
    const finish = (err) => {
      if (done) return;
      done = true;
      if (err) {
        sink.destroy();
        reject(err);
      } else {
        resolve();
      }
    };
    response.on("data", (chunk) => {
      if (progress) progress.onChunk(chunk.length);
    });
    response.on("error", (err) => finish(err));
    sink.on("error", (err) => finish(err));
    sink.on("finish", () => finish(null));
    response.pipe(sink);
  });
}

async function download(url, destination, options = {}) {
  await mkdir(path.dirname(destination), { recursive: true });
  const assetName = options.assetName || path.basename(destination);
  await withRetry(`download ${assetName}`, async (attempt) => {
    const result = await followRedirects(url, {
      totalTimeoutMs: downloadTimeoutMs(),
      stallMs: downloadStallMs(),
    });
    const response = result.response;
    const lenHeader = response.headers["content-length"];
    const total = lenHeader ? Number.parseInt(lenHeader, 10) : 0;
    const progress = createProgressReporter(assetName, Number.isFinite(total) ? total : 0);
    if (attempt > 1) {
      logInfo(`retry attempt ${attempt}/${MAX_ATTEMPTS} for ${assetName}`);
    }
    try {
      await streamToFile(response, destination, progress);
    } catch (err) {
      // Ensure we don't leave a partial file confusing future attempts.
      try {
        await unlink(destination);
      } catch {
        // ignore
      }
      throw err;
    }
    progress.finish();
  });
}

async function downloadText(url) {
  return withRetry(`fetch ${url}`, async () => {
    const result = await followRedirects(url, {
      totalTimeoutMs: downloadTimeoutMs(),
      stallMs: downloadStallMs(),
    });
    const response = result.response;
    response.setEncoding("utf8");
    // NOTE: do NOT use `for await (const chunk of response)` here.
    // `httpRequest` attaches a `data` listener on the response to re-arm
    // the stall timer, which puts the stream in flowing mode. The async
    // iterator expects paused mode and will silently miss every chunk —
    // this manifested as an empty checksum manifest in the npm wrapper
    // smoke test ("Checksum manifest is missing <asset>"). Subscribing
    // to `data` events directly stacks alongside the stall listener and
    // both fire per chunk, so we collect the body correctly without
    // disturbing the stall detection.
    return new Promise((resolve, reject) => {
      const chunks = [];
      response.on("data", (chunk) => {
        chunks.push(chunk);
      });
      response.on("end", () => {
        resolve(chunks.join(""));
      });
      response.on("error", reject);
    });
  });
}

async function readLocalVersion(file) {
  return readFile(file, "utf8").catch(() => "");
}

async function fileExists(file) {
  try {
    const result = await stat(file);
    return result.isFile();
  } catch {
    return false;
  }
}

function parseChecksumManifest(text) {
  const checksums = new Map();
  for (const line of text.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed) {
      continue;
    }
    const match = trimmed.match(/^([a-fA-F0-9]{64})\s+\*?(.+)$/);
    if (!match) {
      throw new Error(`Invalid checksum manifest line: ${trimmed}`);
    }
    checksums.set(match[2], match[1].toLowerCase());
  }
  return checksums;
}

async function sha256File(filePath) {
  const content = await readFile(filePath);
  return crypto.createHash("sha256").update(content).digest("hex");
}

async function verifyChecksum(filePath, assetName, checksums) {
  const expected = checksums.get(assetName);
  if (!expected) {
    throw new NonRetryableError(`Checksum manifest is missing ${assetName}`);
  }
  const actual = await sha256File(filePath);
  if (actual !== expected) {
    // Bytes are corrupted; another fetch is unlikely to help without a fix
    // upstream. Mark non-retryable.
    throw new NonRetryableError(
      `Checksum mismatch for ${assetName}: expected ${expected}, got ${actual}`,
    );
  }
}

async function loadChecksums(version, repo) {
  return parseChecksumManifest(await downloadText(checksumManifestUrl(version, repo)));
}

async function ensureBinary(targetPath, assetName, version, repo, getChecksums) {
  const marker = `${targetPath}.version`;
  const downloadIfNeeded =
    process.env.DEEPSEEK_TUI_FORCE_DOWNLOAD === "1" || process.env.DEEPSEEK_FORCE_DOWNLOAD === "1";
  if (!downloadIfNeeded) {
    const existing = await fileExists(targetPath);
    if (existing) {
      const markerVersion = await readLocalVersion(marker);
      if (markerVersion === String(version)) {
        return targetPath;
      }
    }
  }
  const checksums = await getChecksums();
  const url = releaseAssetUrl(assetName, version, repo);
  const destination = `${targetPath}.${process.pid}.${Date.now()}.download`;
  await download(url, destination, { assetName });
  try {
    await verifyChecksum(destination, assetName, checksums);
    preflightGlibc(destination);
  } catch (error) {
    await unlink(destination).catch(() => {});
    throw error;
  }
  if (process.platform !== "win32") {
    await chmod(destination, 0o755);
  }
  await rename(destination, targetPath);
  await writeFile(marker, String(version), "utf8");
  return targetPath;
}

async function run() {
  if (process.env.DEEPSEEK_TUI_DISABLE_INSTALL === "1" || process.env.DEEPSEEK_DISABLE_INSTALL === "1") {
    return;
  }
  const version = resolvePackageVersion();
  const repo = resolveRepo();
  const paths = binaryPaths();
  const releaseDir = releaseBinaryDirectory();
  await mkdir(releaseDir, { recursive: true });

  let checksumsPromise;
  const getChecksums = () => {
    if (!checksumsPromise) {
      checksumsPromise = loadChecksums(version, repo);
    }
    return checksumsPromise;
  };

  await Promise.all([
    ensureBinary(paths.deepseek.target, paths.deepseek.asset, version, repo, getChecksums),
    ensureBinary(paths.tui.target, paths.tui.asset, version, repo, getChecksums),
  ]);
}

async function getBinaryPath(name) {
  await run();
  const paths = binaryPaths();
  if (name === "deepseek") {
    return paths.deepseek.target;
  }
  if (name === "deepseek-tui") {
    return paths.tui.target;
  }
  throw new Error(`Unknown binary: ${name}`);
}

module.exports = {
  getBinaryPath,
  run,
};

if (require.main === module) {
  run().catch((error) => {
    console.error("deepseek-tui install failed:", error.message);
    if (process.env.DEEPSEEK_TUI_OPTIONAL_INSTALL === "1") {
      console.error(
        "DEEPSEEK_TUI_OPTIONAL_INSTALL=1 set; continuing without a usable binary.",
      );
      process.exit(0);
    }
    process.exit(1);
  });
}
