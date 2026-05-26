const https = require("https");
const http = require("http");
const {
  allAssetNames,
  allReleaseAssetNames,
  checksumManifestUrl,
  releaseAssetUrl,
} = require("./artifacts");

const pkg = require("../package.json");

function resolveBinaryVersion() {
  const configuredVersion =
    process.env.DEEPSEEK_TUI_VERSION ||
    process.env.DEEPSEEK_VERSION ||
    pkg.codewhaleBinaryVersion || pkg.deepseekBinaryVersion ||
    pkg.version;
  return String(configuredVersion).trim();
}

function resolveRepo() {
  return process.env.DEEPSEEK_TUI_GITHUB_REPO || process.env.DEEPSEEK_GITHUB_REPO || "Hmbown/CodeWhale";
}

function requestStatus(url, method = "HEAD", redirects = 0) {
  if (redirects > 10) {
    throw new Error(`Too many redirects while checking ${url}`);
  }
  const client = url.startsWith("https:") ? https : http;
  return new Promise((resolve, reject) => {
    const req = client.request(
      url,
      {
        method,
        headers: {
          "User-Agent": "codewhale-npm-release-check",
        },
      },
      (res) => {
        const status = res.statusCode || 0;
        const location = res.headers.location;
        res.resume();
        if (status >= 300 && status < 400 && location) {
          const next = new URL(location, url).toString();
          resolve(requestStatus(next, method, redirects + 1));
          return;
        }
        resolve(status);
      },
    );
    req.on("error", reject);
    req.end();
  });
}

async function verifyAsset(url, label) {
  let status = await requestStatus(url, "HEAD");
  if (status === 403 || status === 405) {
    status = await requestStatus(url, "GET");
  }
  if (status < 200 || status >= 400) {
    throw new Error(`${label} returned HTTP ${status} (${url})`);
  }
}

async function downloadText(url) {
  const client = url.startsWith("https:") ? https : http;
  return new Promise((resolve, reject) => {
    client
      .get(
        url,
        {
          headers: {
            "User-Agent": "codewhale-npm-release-check",
          },
        },
        (res) => {
          const status = res.statusCode || 0;
          if (status >= 300 && status < 400 && res.headers.location) {
            const next = new URL(res.headers.location, url).toString();
            resolve(downloadText(next));
            return;
          }
          if (status !== 200) {
            reject(new Error(`Request failed with status ${status}: ${url}`));
            res.resume();
            return;
          }
          const chunks = [];
          res.setEncoding("utf8");
          res.on("data", (chunk) => chunks.push(chunk));
          res.on("end", () => resolve(chunks.join("")));
        },
      )
      .on("error", reject);
  });
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

async function run() {
  const version = resolveBinaryVersion();
  const repo = resolveRepo();
  const assets = allReleaseAssetNames();

  console.log(`Verifying ${assets.length} release assets for ${repo}@v${version}...`);
  for (const asset of assets) {
    const url = releaseAssetUrl(asset, version, repo);
    await verifyAsset(url, asset);
    console.log(`  ok ${asset}`);
  }
  const checksums = parseChecksumManifest(
    await downloadText(checksumManifestUrl(version, repo)),
  );
  for (const asset of allAssetNames()) {
    if (!checksums.has(asset)) {
      throw new Error(`Checksum manifest is missing ${asset}`);
    }
  }
  console.log("Release assets verified.");
}

run().catch((error) => {
  console.error("Release asset verification failed:", error.message);
  process.exit(1);
});
