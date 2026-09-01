#!/usr/bin/env node
// Post a GitHub Check Run for the exact commit a CNB pipeline is building,
// authenticating as the codewhale-cnb-bridge GitHub App.
//
// Secret custody: the App credentials live in the CNB KeyStore repo
// codewhale.net/codewhale-ci-secrets (github-bridge.yml) and are injected as
// environment variables via `imports:` in .cnb.yml
// (https://docs.cnb.cool/en/repo/secret.html). This script reads them from the
// environment and never logs them. Accepted variable names (first match wins):
//   app id:           GITHUB_APP_ID, GH_APP_ID, APP_ID
//   installation id:  GITHUB_APP_INSTALLATION_ID, GH_APP_INSTALLATION_ID, INSTALLATION_ID
//   private key:      GITHUB_APP_PRIVATE_KEY, GH_APP_PRIVATE_KEY, PRIVATE_KEY
// The private key may be a raw PEM, a PEM with escaped \n, or base64-encoded.
//
// Usage:
//   node scripts/ci/cnb-github-checkrun.mjs \
//     --name "linux rust gates -cnb" --sha <40-hex> \
//     --status completed --conclusion success \
//     --details-url <url> --summary <text>
import crypto from "node:crypto";

const GITHUB_API = process.env.GITHUB_API_BASE || "https://api.github.com";
const REPO = process.env.GITHUB_REPOSITORY || "Hmbown/CodeWhale";
const USER_AGENT = "codewhale-cnb-bridge";

function fail(message) {
  console.error(`cnb-github-checkrun: ${message}`);
  process.exit(1);
}

function envAny(...names) {
  for (const name of names) {
    const value = process.env[name];
    if (value && value.trim()) return value.trim();
  }
  return "";
}

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i += 1) {
    const token = argv[i];
    if (!token.startsWith("--")) fail(`unexpected argument ${JSON.stringify(token)}`);
    const key = token.slice(2);
    const value = argv[i + 1];
    if (value === undefined || value.startsWith("--")) fail(`--${key} requires a value`);
    args[key] = value;
    i += 1;
  }
  return args;
}

function normalizePrivateKey(raw) {
  const withNewlines = raw.replace(/\\n/g, "\n");
  if (withNewlines.includes("BEGIN")) return withNewlines;
  const decoded = Buffer.from(raw, "base64").toString("utf8");
  if (decoded.includes("BEGIN")) return decoded;
  fail("private key does not look like a PEM (raw, escaped, or base64)");
}

function mintAppJwt(appId, privateKey) {
  const now = Math.floor(Date.now() / 1000);
  const encode = (obj) => Buffer.from(JSON.stringify(obj)).toString("base64url");
  const header = encode({ alg: "RS256", typ: "JWT" });
  const payload = encode({ iat: now - 60, exp: now + 540, iss: appId });
  const unsigned = `${header}.${payload}`;
  const signature = crypto.sign("sha256", Buffer.from(unsigned), privateKey).toString("base64url");
  return `${unsigned}.${signature}`;
}

async function githubApi(path, token, method, body) {
  const response = await fetch(`${GITHUB_API}${path}`, {
    method,
    headers: {
      Authorization: `Bearer ${token}`,
      Accept: "application/vnd.github+json",
      "X-GitHub-Api-Version": "2022-11-28",
      "User-Agent": USER_AGENT,
    },
    body: body ? JSON.stringify(body) : undefined,
  });
  if (!response.ok) {
    // GitHub error bodies never contain our credentials; cap the log line anyway.
    const text = (await response.text()).slice(0, 500);
    fail(`${method} ${path} -> HTTP ${response.status}: ${text}`);
  }
  return response.json();
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const name = args.name || fail("--name is required");
  const sha = args.sha || fail("--sha is required");
  if (!/^[0-9a-f]{40}$/.test(sha)) {
    fail(`--sha must be a 40-character lowercase hex commit, got ${JSON.stringify(sha)}`);
  }
  const status = args.status || "completed";
  if (!["queued", "in_progress", "completed"].includes(status)) {
    fail(`--status must be queued|in_progress|completed, got ${JSON.stringify(status)}`);
  }
  const conclusions = ["success", "failure", "cancelled", "neutral", "skipped", "timed_out"];
  const conclusion = args.conclusion || "";
  if (status === "completed" && !conclusions.includes(conclusion)) {
    fail(`--conclusion must be one of ${conclusions.join("|")} when --status completed`);
  }

  const appId = envAny("GITHUB_APP_ID", "GH_APP_ID", "APP_ID") ||
    fail("GitHub App id env var missing (expected GITHUB_APP_ID)");
  const installationId = envAny("GITHUB_APP_INSTALLATION_ID", "GH_APP_INSTALLATION_ID", "INSTALLATION_ID") ||
    fail("GitHub App installation id env var missing (expected GITHUB_APP_INSTALLATION_ID)");
  const privateKey = normalizePrivateKey(
    envAny("GITHUB_APP_PRIVATE_KEY", "GH_APP_PRIVATE_KEY", "PRIVATE_KEY") ||
      fail("GitHub App private key env var missing (expected GITHUB_APP_PRIVATE_KEY)"),
  );

  const jwt = mintAppJwt(appId, privateKey);
  const installation = await githubApi(
    `/app/installations/${encodeURIComponent(installationId)}/access_tokens`,
    jwt,
    "POST",
  );
  if (!installation.token) fail("installation token response had no token field");

  const now = new Date().toISOString();
  const checkRun = {
    name,
    head_sha: sha,
    status,
    output: {
      title: args.title || `${name}: ${status === "completed" ? conclusion : status}`,
      summary: args.summary || "",
    },
  };
  if (args["details-url"]) checkRun.details_url = args["details-url"];
  if (status === "completed") {
    checkRun.conclusion = conclusion;
    checkRun.completed_at = now;
  } else {
    checkRun.started_at = now;
  }

  const created = await githubApi(`/repos/${REPO}/check-runs`, installation.token, "POST", checkRun);
  // Receipt line: id, conclusion, and URL only — never any credential material.
  console.log(`check run ${created.id} ${status}${conclusion ? `/${conclusion}` : ""} ${created.html_url}`);
}

main().catch((error) => fail(error && error.message ? error.message : String(error)));
