#!/usr/bin/env node
/**
 * Compare the public deployment receipt with an exact local/source revision.
 * No Cloudflare or GitHub credentials are required. By default this is a
 * report-only command; --require-current turns drift/unavailability into a
 * failing post-deploy gate.
 */
import { execFileSync } from "node:child_process";
import { buildFacts, REPO_ROOT } from "./facts-lib.mjs";

function parseArgs(argv) {
  const out = {
    baseUrl: "https://codewhale.net",
    expectedRevision: process.env.CODEWHALE_SOURCE_REVISION || process.env.GITHUB_SHA || null,
    requireCurrent: false,
    attempts: null,
    retryDelayMs: 3_000,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--require-current") {
      out.requireCurrent = true;
    } else if (arg === "--base-url") {
      out.baseUrl = argv[++index];
    } else if (arg === "--expected-revision") {
      out.expectedRevision = argv[++index];
    } else if (arg === "--attempts") {
      out.attempts = Number(argv[++index]);
    } else if (arg === "--retry-delay-ms") {
      out.retryDelayMs = Number(argv[++index]);
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }
  out.attempts ??= out.requireCurrent ? 6 : 1;
  return out;
}

function localRevision() {
  try {
    return execFileSync("git", ["rev-parse", "HEAD"], {
      cwd: REPO_ROOT,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    return null;
  }
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function fetchReceipt(url) {
  try {
    const response = await fetch(`${url}?receipt=${Date.now()}`, {
      cache: "no-store",
      headers: {
        Accept: "application/json",
        "User-Agent": "codewhale-deploy-facts-check",
      },
    });
    if (!response.ok) {
      return { error: `HTTP ${response.status}` };
    }
    return { receipt: await response.json() };
  } catch (error) {
    return { error: error instanceof Error ? error.message : String(error) };
  }
}

function compare(expected, receipt) {
  const differences = [];
  const deployed = receipt?.deployed;
  if (receipt?.schemaVersion !== 1 || !deployed || typeof deployed !== "object") {
    return [{ field: "receipt", expected: "schemaVersion=1", deployed: null }];
  }

  const checks = [
    ["sourceRevision", expected.sourceRevision, deployed.sourceRevision],
    ["version", expected.version, deployed.version],
    ["providerCount", expected.providerCount, deployed.providerCount],
    ["toolCount", expected.toolCount, deployed.toolCount],
    [
      "latestPublishedRelease.tag",
      expected.latestPublishedRelease?.tag ?? null,
      receipt.latestPublishedRelease?.tag ?? null,
    ],
  ];
  for (const [field, expectedValue, deployedValue] of checks) {
    if (expectedValue !== deployedValue) {
      differences.push({ field, expected: expectedValue, deployed: deployedValue });
    }
  }
  return differences;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  let baseUrl;
  try {
    baseUrl = new URL(args.baseUrl);
  } catch {
    throw new Error(`invalid --base-url: ${args.baseUrl}`);
  }
  if (!/^https?:$/.test(baseUrl.protocol)) {
    throw new Error("--base-url must use http or https");
  }

  const expectedRevision = args.expectedRevision || localRevision();
  if (!expectedRevision || !/^[0-9a-f]{40}$/i.test(expectedRevision)) {
    throw new Error("expected revision must be an exact 40-character Git SHA");
  }
  if (!Number.isInteger(args.attempts) || args.attempts < 1 || args.attempts > 20) {
    throw new Error("--attempts must be an integer from 1 to 20");
  }
  if (!Number.isFinite(args.retryDelayMs) || args.retryDelayMs < 0 || args.retryDelayMs > 30_000) {
    throw new Error("--retry-delay-ms must be between 0 and 30000");
  }

  const facts = buildFacts();
  const expected = {
    sourceRevision: expectedRevision,
    version: facts.version,
    providerCount: facts.providers.length,
    toolCount: facts.toolCount,
    latestPublishedRelease: facts.latestPublishedRelease,
  };
  const endpoint = new URL("/api/facts", baseUrl).toString();
  let last = { error: "not attempted" };
  let differences = [];

  for (let attempt = 1; attempt <= args.attempts; attempt += 1) {
    last = await fetchReceipt(endpoint);
    differences = last.receipt ? compare(expected, last.receipt) : [];
    if (last.receipt && differences.length === 0) break;
    if (attempt < args.attempts) await delay(args.retryDelayMs);
  }

  const status = last.receipt
    ? differences.length === 0
      ? "current"
      : "drift"
    : "unavailable";
  const report = {
    schemaVersion: 1,
    checkedAt: new Date().toISOString(),
    endpoint,
    status,
    expected,
    deployed: last.receipt ?? null,
    differences,
    error: last.error ?? null,
    deploymentAttempted: false,
  };
  console.log(JSON.stringify(report, null, 2));

  if (args.requireCurrent && status !== "current") {
    process.exitCode = 1;
  }
}

main().catch((error) => {
  console.error(`[compare-deployed-facts] ERROR: ${error.message}`);
  process.exitCode = 1;
});
