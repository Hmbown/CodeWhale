import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

// fileURLToPath, not `.pathname`: the latter stays percent-encoded, so a
// checkout under a path with non-ASCII characters spawns a filename that
// does not exist and every case here fails on a module-not-found exit.
const script = fileURLToPath(new URL("../scripts/check-cloudflare-deploy-env.mjs", import.meta.url));

function run(overrides: Record<string, string>, args: string[] = []) {
  return spawnSync(process.execPath, [script, ...args], {
    encoding: "utf8",
    env: {
      ...process.env,
      GITHUB_ACTIONS: "",
      GITHUB_EVENT_NAME: "",
      GITHUB_REF: "",
      GITHUB_SHA: "",
      CLOUDFLARE_ACCOUNT_ID: "",
      CLOUDFLARE_API_TOKEN: "",
      ...overrides,
    },
  });
}

describe("Cloudflare deploy preflight", () => {
  it("reports intentionally withheld credentials without deploying", () => {
    const result = run({}, ["--preflight"]);

    expect(result.status).toBe(0);
    expect(result.stdout).toContain("credentialState\":\"withheld");
    expect(result.stdout).toContain("deploymentStarted\":false");
  });

  it("rejects malformed supplied values even in credential-free preflight mode", () => {
    const result = run(
      {
        CLOUDFLARE_ACCOUNT_ID: "not-an-account-id",
        CLOUDFLARE_API_TOKEN: "not-a-token",
      },
      ["--preflight"],
    );

    expect(result.status).toBe(1);
    expect(result.stderr).toContain("malformed credential placeholders");
    expect(result.stdout).toContain("credentialState\":\"invalid");
  });

  it("keeps the normal deploy check fail-closed when credentials are missing", () => {
    const result = run({});

    expect(result.status).toBe(1);
    expect(result.stderr).toContain("Cloudflare deploy configuration is incomplete");
  });

  it("requires an exact manual-main context inside GitHub Actions", () => {
    const result = run({
      GITHUB_ACTIONS: "true",
      GITHUB_EVENT_NAME: "push",
      GITHUB_REF: "refs/heads/main",
      GITHUB_SHA: "a".repeat(40),
      CLOUDFLARE_ACCOUNT_ID: "a".repeat(32),
      CLOUDFLARE_API_TOKEN: "token-" + "b".repeat(32),
    });

    expect(result.status).toBe(1);
    expect(result.stderr).toContain("workflow_dispatch on refs/heads/main");
  });

  it("rejects a dispatch on a non-main ref", () => {
    const result = run({
      GITHUB_ACTIONS: "true",
      GITHUB_EVENT_NAME: "workflow_dispatch",
      GITHUB_REF: "refs/heads/release",
      GITHUB_SHA: "a".repeat(40),
      CLOUDFLARE_ACCOUNT_ID: "a".repeat(32),
      CLOUDFLARE_API_TOKEN: "token-" + "b".repeat(32),
    });

    expect(result.status).toBe(1);
    expect(result.stderr).toContain("workflow_dispatch on refs/heads/main");
  });

  it("rejects a dispatch without an exact 40-hex revision", () => {
    const result = run({
      GITHUB_ACTIONS: "true",
      GITHUB_EVENT_NAME: "workflow_dispatch",
      GITHUB_REF: "refs/heads/main",
      GITHUB_SHA: "main",
      CLOUDFLARE_ACCOUNT_ID: "a".repeat(32),
      CLOUDFLARE_API_TOKEN: "token-" + "b".repeat(32),
    });

    expect(result.status).toBe(1);
    expect(result.stderr).toContain("exact SHA");
  });

  it("accepts a manual dispatch on main at an exact SHA", () => {
    const result = run({
      GITHUB_ACTIONS: "true",
      GITHUB_EVENT_NAME: "workflow_dispatch",
      GITHUB_REF: "refs/heads/main",
      GITHUB_SHA: "a".repeat(40),
      CLOUDFLARE_ACCOUNT_ID: "a".repeat(32),
      CLOUDFLARE_API_TOKEN: "token-" + "b".repeat(32),
    });

    expect(result.status).toBe(0);
    expect(result.stdout).toContain("Cloudflare deploy environment is present");
  });
});

// Minimal, dependency-free reader for the two-space-indented job blocks in
// .github/workflows/web.yml. A real YAML parser is not a web dependency, and
// this file only needs the `on:` triggers plus the deploy job's `if:` guard.
function readWebWorkflow() {
  const path = new URL("../../.github/workflows/web.yml", import.meta.url);
  return readFileSync(path, "utf8");
}

function jobBlock(source: string, job: string) {
  const lines = source.split("\n");
  const start = lines.findIndex((line) => line === `  ${job}:`);
  expect(start, `job ${job} not found in web.yml`).toBeGreaterThanOrEqual(0);
  const rest = lines.slice(start + 1);
  const end = rest.findIndex((line) => /^ {2}\S/.test(line));
  return (end === -1 ? rest : rest.slice(0, end)).join("\n");
}

describe("web workflow deploy trigger contract", () => {
  const workflow = readWebWorkflow();
  const deploy = jobBlock(workflow, "deploy");
  const deployReminder = jobBlock(workflow, "deploy-reminder");

  it("still runs lint on pushes and pull requests", () => {
    expect(workflow).toContain("  push:\n    branches: [master, main]");
    expect(workflow).toContain("  pull_request:\n    branches: [master, main]");
    expect(workflow).toContain("  workflow_dispatch:");
    expect(jobBlock(workflow, "lint")).not.toContain("if:");
  });

  it("gates deploy on a manual dispatch of main only", () => {
    const guard = deploy
      .slice(deploy.indexOf("if:"))
      .split("\n")
      .slice(0, 3)
      .join(" ")
      .replace(/\s+/g, " ");

    expect(guard).toContain("github.event_name == 'workflow_dispatch'");
    expect(guard).toContain("github.ref == 'refs/heads/main'");
    // The preflight script fails closed on any non-dispatch event, so a push
    // trigger here could only ever produce a red deploy job (#4907).
    expect(guard).not.toContain("'push'");
    expect(deploy).toContain("needs: lint");
  });

  it("surfaces an actionable deployment reminder after a green main push", () => {
    expect(deployReminder).toContain("needs: lint");
    expect(deployReminder).toContain(
      "github.event_name == 'push' && github.ref == 'refs/heads/main'",
    );
    expect(deployReminder).toContain("::notice title=Web deployment approval needed::");
    expect(deployReminder).toContain("gh workflow run web.yml");
    expect(deployReminder).not.toContain("npm run deploy");
  });

  it("checks out the exact dispatched revision before deploying", () => {
    expect(deploy).toContain("ref: ${{ github.sha }}");
    expect(deploy).toContain('--expected-revision "$GITHUB_SHA"');
  });

  it("builds one OpenNext bundle before preview or deploy without a Wrangler rebuild", () => {
    const packageJson = JSON.parse(
      readFileSync(new URL("../package.json", import.meta.url), "utf8"),
    ) as { scripts: Record<string, string> };
    const wrangler = JSON.parse(
      readFileSync(new URL("../wrangler.jsonc", import.meta.url), "utf8"),
    ) as { build?: { command?: string } };

    expect(packageJson.scripts.preview).toBe(
      "opennextjs-cloudflare build && opennextjs-cloudflare preview",
    );
    expect(packageJson.scripts.deploy).toBe(
      "opennextjs-cloudflare build && opennextjs-cloudflare deploy",
    );
    expect(wrangler.build).toBeUndefined();
    expect(deploy).toContain("run: npm run deploy");
    expect(deploy).not.toContain("npm run build");
    expect(deploy).not.toContain("npx opennextjs-cloudflare build");
  });
});
