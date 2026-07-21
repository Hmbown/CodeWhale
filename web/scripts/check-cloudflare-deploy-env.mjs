#!/usr/bin/env node
/**
 * check-cloudflare-deploy-env.mjs - fail fast when the GitHub deploy job is
 * missing Cloudflare credentials.
 *
 * The actual deploy still belongs to Wrangler/OpenNext. This script only makes
 * the common GitHub Actions failure mode obvious before the expensive build
 * starts.
 */

const required = [
  {
    name: "CLOUDFLARE_ACCOUNT_ID",
    source: "repository variable",
    expected: "Settings > Secrets and variables > Actions > Variables",
    validate(value) {
      return /^[a-f0-9]{32}$/i.test(value);
    },
    detail: "expected the 32-character Cloudflare account id",
  },
  {
    name: "CLOUDFLARE_API_TOKEN",
    source: "repository secret",
    expected: "Settings > Secrets and variables > Actions > Secrets",
    validate(value) {
      return value.length >= 20;
    },
    detail: "expected a non-empty Cloudflare API token",
  },
];

const placeholderPattern = /^(changeme|replace(_with)?|todo|example|dummy|null|undefined)$/i;
const failures = [];
const preflight = process.argv.includes("--preflight");

function printReceipt(credentialState) {
  console.log(
    `[check-cloudflare-deploy-env] receipt ${JSON.stringify({
      event: process.env.GITHUB_EVENT_NAME || null,
      ref: process.env.GITHUB_REF || null,
      sourceRevision:
        process.env.CODEWHALE_SOURCE_REVISION || process.env.GITHUB_SHA || null,
      credentialState,
      deploymentStarted: false,
    })}`,
  );
}

for (const item of required) {
  const value = (process.env[item.name] ?? "").trim();
  if (!value) {
    failures.push({
      item,
      reason: `${item.name} is not set`,
      kind: "missing",
    });
    continue;
  }

  if (placeholderPattern.test(value)) {
    failures.push({
      item,
      reason: `${item.name} is a placeholder`,
      kind: "invalid",
    });
    continue;
  }

  if (!item.validate(value)) {
    failures.push({
      item,
      reason: `${item.name} is set but does not look valid`,
      kind: "invalid",
    });
  }
}

if (failures.length > 0) {
  if (preflight) {
    const invalid = failures.filter((failure) => failure.kind === "invalid");
    if (invalid.length > 0) {
      console.error(
        "[check-cloudflare-deploy-env] FAIL - malformed credential placeholders are not a valid preflight.",
      );
      for (const failure of invalid) console.error(`- ${failure.reason}`);
      printReceipt("invalid");
      process.exit(1);
    }
    console.log(
      "[check-cloudflare-deploy-env] PREFLIGHT - deploy credentials are intentionally unavailable in this environment.",
    );
    console.log(
      `[check-cloudflare-deploy-env] ${failures.length} credential input(s) must be supplied by the protected manual deploy job.`,
    );
    console.log("[check-cloudflare-deploy-env] Wrangler deploy was not started.");
    printReceipt("withheld");
    process.exit(0);
  }

  console.error("[check-cloudflare-deploy-env] FAIL - Cloudflare deploy configuration is incomplete.");
  for (const failure of failures) {
    const { item, reason } = failure;
    console.error("");
    console.error(`- ${reason}`);
    console.error(`  Configure ${item.name} as a GitHub ${item.source}.`);
    console.error(`  Location: ${item.expected}.`);
    console.error(`  Hint: ${item.detail}.`);
  }
  console.error("");
  console.error("Wrangler deploy was not started.");
  printReceipt(failures.some((failure) => failure.kind === "invalid") ? "invalid" : "missing");
  process.exit(1);
}

if (process.env.GITHUB_ACTIONS === "true") {
  const event = process.env.GITHUB_EVENT_NAME;
  const ref = process.env.GITHUB_REF;
  const revision = process.env.GITHUB_SHA;
  if (
    event !== "workflow_dispatch" ||
    ref !== "refs/heads/main" ||
    !revision ||
    !/^[0-9a-f]{40}$/i.test(revision)
  ) {
    console.error(
      "[check-cloudflare-deploy-env] FAIL - GitHub deployment requires workflow_dispatch on refs/heads/main at an exact SHA.",
    );
    printReceipt("present");
    process.exit(1);
  }
}

console.log("[check-cloudflare-deploy-env] OK - Cloudflare deploy environment is present.");
printReceipt("present");
