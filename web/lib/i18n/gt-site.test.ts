import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { getChrome, getDocsGuide, getHome } from "./dictionaries";

const webRoot = fileURLToPath(new URL("../..", import.meta.url));

function runGtSite(command: string, extraEnv: Record<string, string> = {}) {
  return spawnSync(process.execPath, [join(webRoot, "scripts", "gt-site.mjs"), command], {
    cwd: webRoot,
    encoding: "utf8",
    env: {
      ...process.env,
      GT_API_KEY: "",
      GT_PROJECT_ID: "",
      ...extraEnv,
    },
  });
}

describe("website GT catalog pipeline", () => {
  it("keeps local catalogs in sync with the dictionary runtime", () => {
    const check = runGtSite("check");
    expect(check.status, check.stderr || check.stdout).toBe(0);
    expect(check.stdout).toContain("TUI packs untouched");

    const zh = JSON.parse(readFileSync(join(webRoot, "gt-catalog", "zh.json"), "utf8"));
    expect(zh.chrome.navDocs).toBe(getChrome("zh").navDocs);
    expect(zh.home.kicker).toBe(getHome("zh").kicker);
    expect(zh["docs-guide"].overviewTitle).toBe(getDocsGuide("zh").overviewTitle);

    const config = readFileSync(join(webRoot, "gt.config.json"), "utf8");
    expect(config).toContain("gt-catalog/[locale].json");
    expect(config).not.toContain("crates/tui");
    expect(config).not.toContain("prompts/text.rs");
  });

  it("fails closed when translate is asked to call the GT API without BYOK env", () => {
    const translate = runGtSite("translate");
    expect(translate.status).not.toBe(0);
    expect(`${translate.stderr}${translate.stdout}`).toMatch(/fail-closed|GT_API_KEY/);
  });
});
