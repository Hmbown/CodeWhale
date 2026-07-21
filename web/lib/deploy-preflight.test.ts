import { spawnSync } from "node:child_process";
import { describe, expect, it } from "vitest";

const script = new URL("../scripts/check-cloudflare-deploy-env.mjs", import.meta.url);

function run(overrides: Record<string, string>, args: string[] = []) {
  return spawnSync(process.execPath, [script.pathname, ...args], {
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
});
