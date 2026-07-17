import { existsSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { FACTS } from "./facts.generated";
import { RELEASE_CONTRIBUTORS, RELEASE_HELPERS } from "./release-credits";

function pageSource(path: string): string {
  return readFileSync(new URL(`../app/[locale]/${path}`, import.meta.url), "utf8");
}

describe("public website copy contracts", () => {
  it("keeps the README and website product screenshot synchronized and accessible", () => {
    const home = pageSource("page.tsx");
    const readme = readFileSync(new URL("../../README.md", import.meta.url), "utf8");
    const readmeShot = readFileSync(new URL("../../assets/screenshot.png", import.meta.url));
    const websiteShot = readFileSync(
      new URL("../public/codewhale-tui.png", import.meta.url),
    );
    const alt = "Codewhale TUI idle screen with GLM-5.2 selected through Z.ai";

    expect(readme).toContain(`![${alt}](assets/screenshot.png)`);
    expect(home).toContain(`src="/codewhale-tui.png"`);
    expect(home).toContain(`alt="${alt}"`);
    expect(websiteShot.equals(readmeShot)).toBe(true);
  });

  it("keeps the docs hub on the compact ocean portal instead of the old almanac treatment", () => {
    const layout = pageSource("docs/layout.tsx");
    const search = readFileSync(new URL("../components/docs-search.tsx", import.meta.url), "utf8");

    expect(layout).toContain("docs-portal-hero");
    expect(layout).toContain("Find the guidance you need.");
    expect(layout).not.toContain("Section 02");
    expect(layout).not.toContain("How Codewhale works: ego");
    expect(layout).not.toContain("<Seal");
    expect(layout.indexOf('<article className="docs-content')).toBeLessThan(
      layout.indexOf("<DocsSidebar"),
    );
    expect(search).toContain("docs-topic-row");
    expect(search).not.toContain("40+ Markdown documents");
  });

  it("does not rule out the managed app or make it a requirement for local use", () => {
    const roadmap = pageSource("roadmap/page.tsx");
    const footer = readFileSync(new URL("../components/footer.tsx", import.meta.url), "utf8");

    expect(roadmap).toContain("Managed app preview and optional accounts");
    expect(roadmap).toContain("Required account for the local runtime");
    expect(roadmap).not.toContain("Hosted SaaS dashboard");
    expect(roadmap).not.toContain("Required login / accounts");
    expect(footer).toContain("App preview");
    expect(footer).toContain("https://app.codewhale.net");
    expect(footer).not.toMatch(/Create account|Sign up/);
  });

  it("describes ACP and the VS Code extension at their implemented capability level", () => {
    const runtime = pageSource("runtime/page.tsx");
    const sourceDocTargets = [
      ...new Set(
        [...runtime.matchAll(/REPO_BLOB_BASE}\/([^`]+)`/g)].map((match) => match[1]),
      ),
    ];

    expect(runtime).toContain("ACP (Agent Client Protocol)");
    expect(runtime).toContain("Baseline JSON-RPC adapter over stdio");
    expect(runtime).toContain("Phase 0 companion for the local runtime");
    expect(runtime).not.toContain("Agent Communication Protocol");
    expect(runtime).not.toContain("IETF-standard");
    expect(runtime).not.toContain("embeds Codewhale as a side-panel agent");
    expect(runtime).not.toMatch(/\/(?:en|zh)\/docs#(?:runtime-api|acp|mcp)/);
    expect(runtime).toContain("docs/RUNTIME_API.md");
    expect(runtime).toContain("docs/MCP.md");
    expect(sourceDocTargets).toEqual(["docs/RUNTIME_API.md", "docs/MCP.md"]);
    for (const target of sourceDocTargets) {
      expect(existsSync(new URL(`../../${target}`, import.meta.url)), target).toBe(true);
    }
  });

  it("uses the current modes, permission postures, and key guidance", () => {
    const modes = pageSource("docs/modes/page.tsx");
    const install = pageSource("install/page.tsx");
    const faq = pageSource("faq/page.tsx");
    const modeCopy = `${modes}\n${install}\n${faq}`;

    expect(modeCopy).not.toMatch(/\bAgent mode\b|Agent 模式|\bYOLO\b|suggest\s*\/\s*auto\s*\/\s*never|approval_mode|审批模式（建议/);
    for (const label of ["Plan", "Act", "Operate", "Ask", "Auto-Review", "Full Access"]) {
      expect(modes).toContain(label);
      expect(install).toContain(label);
    }
    expect(modes).toContain("/mode act");
    expect(modes).toContain("Shift+Tab");
    expect(modes).toContain("Plan is always Read Only");
    expect(modes).toContain("same permission posture, sandbox, and safety rules as Act");
    expect(faq).toContain("delegation is not mandatory");
    expect(modeCopy).not.toContain("executable work is dispatched to background Fleet workers");
    expect(install).toContain("New sessions open in Act mode by default");
  });

  it("presents providers as peers and puts contributor actions near the top", () => {
    const providerCopy = `${pageSource("models/page.tsx")}\n${pageSource("faq/page.tsx")}`;
    const community = pageSource("community/page.tsx");

    expect(providerCopy).not.toMatch(/first-class|一级支持|一级模型/);
    expect(community).toContain("International open-source community");
    expect(community).toContain("issues/new/choose");
    expect(community).toContain("docs/LOCALIZATION.md");
    expect(community).toContain("Hmbown/CodeWhale/pulls");
    expect(community).toContain("keeps the weekly archive of repository activity");
    expect(community).not.toContain("latest one sits near the top");
    expect(community).not.toContain("<Ticker");
    expect(community).not.toContain("<StatGrid");
    expect(community).not.toContain("Today's dispatch");
  });

  it("keeps current-release website credits in exact changelog parity", () => {
    expect(FACTS.version).toBeTruthy();
    const changelog = readFileSync(new URL("../../CHANGELOG.md", import.meta.url), "utf8");
    const release = changelog
      .split(`## [${FACTS.version}]`)[1]
      ?.split("\n## ")[0];
    const contributorSection = release
      ?.split("### Contributors")[1]
      ?.split("\n### ")[0];
    expect(contributorSection, `missing ${FACTS.version} contributor ledger`).toBeTruthy();

    const changelogHandles = [
      ...new Set(contributorSection?.match(/@[A-Za-z0-9_-]+/g) ?? []),
    ].sort();
    const websiteHandles = [...RELEASE_CONTRIBUTORS, ...RELEASE_HELPERS];
    const contributorDoc = readFileSync(
      new URL("../../docs/CONTRIBUTORS.md", import.meta.url),
      "utf8",
    );
    const currentDocBand = contributorDoc
      .split(`<summary><strong>v${FACTS.version} `)[1]
      ?.split("</details>")[0];
    expect(currentDocBand, `missing ${FACTS.version} contributor-doc band`).toBeTruthy();
    const docHandles = [
      ...new Set(
        [...(currentDocBand?.matchAll(/github\.com\/([A-Za-z0-9_-]+)\)/g) ?? [])].map(
          (match) => `@${match[1]}`,
        ),
      ),
    ].sort();

    expect(new Set(websiteHandles).size, "credit arrays must not overlap or repeat").toBe(
      websiteHandles.length,
    );
    expect([...websiteHandles].sort()).toEqual(changelogHandles);
    expect(docHandles).toEqual(changelogHandles);
  });
});
