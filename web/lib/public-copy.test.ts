import { existsSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { FACTS } from "./facts.generated";
import { RELEASE_CONTRIBUTORS, RELEASE_HELPERS } from "./release-credits";
import { EN_CHROME, EN_DOCS_SHELL, EN_HOME, getChrome, getHome } from "./i18n/dictionaries";

function pageSource(path: string): string {
  return readFileSync(new URL(`../app/[locale]/${path}`, import.meta.url), "utf8");
}

describe("public website copy contracts", () => {
  it("keeps the docs hub on the compact ocean portal instead of the old almanac treatment", () => {
    const layout = pageSource("docs/layout.tsx");
    const search = readFileSync(new URL("../components/docs-search.tsx", import.meta.url), "utf8");

    expect(layout).toContain("docs-portal-band");
    // The hero copy is dictionary-driven now (#5337), so assert it where the
    // string actually lives rather than in the TSX.
    expect(EN_DOCS_SHELL.heroTitle).toBe("Find the guidance you need.");
    expect(layout).not.toContain("Section 02");
    expect(layout).not.toContain("How Codewhale works: ego");
    expect(layout).not.toContain("<Seal");
    expect(layout.indexOf('<article className="docs-content')).toBeLessThan(
      layout.indexOf("<DocsSidebar"),
    );
    expect(search).toContain("docs-topic-row");
    expect(search).not.toContain("40+ Markdown documents");
  });

  it("keeps unreleased managed-product surfaces out of public copy", () => {
    const roadmap = pageSource("roadmap/page.tsx");
    const footer = readFileSync(new URL("../components/footer.tsx", import.meta.url), "utf8");

    expect(roadmap).toContain("Required account for the local runtime");
    expect(roadmap).not.toContain("Managed app preview");
    expect(roadmap).not.toContain("Hosted SaaS dashboard");
    expect(roadmap).not.toContain("Required login / accounts");
    expect(footer).not.toContain("App preview");
    expect(footer).not.toContain("app.codewhale.net");
    expect(footer).not.toMatch(/Create account|Sign up/);

    // Footer copy is dictionary-driven now, so the same ban has to hold
    // wherever the strings actually live — in every locale, not just the TSX.
    for (const locale of [
      "en", "zh", "ja", "vi", "ko", "ru", "uk", "es", "pt-BR", "id",
      "fr", "de", "ca", "hi", "tr", "it", "pl", "ar",
    ]) {
      const values = Object.values(getChrome(locale)).join("\n");
      expect(values, `${locale} chrome`).not.toContain("app.codewhale.net");
      expect(values, `${locale} chrome`).not.toMatch(/Create account|Sign up|App preview/);
    }
    expect(EN_CHROME.footerLicense).toBe("MIT license");
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

  it("keeps source-candidate facts separate from published install facts", () => {
    const homepage = pageSource("page.tsx");
    const install = pageSource("install/page.tsx");
    const community = pageSource("community/page.tsx");

    expect(homepage).toContain("facts.latestPublishedRelease");
    // The machine-readable source-state attribute stays a literal.
    expect(homepage).toContain('data-source-state={sourceIsPublished ? "published release" : "source candidate"}');
    expect(homepage).toContain("publishedRelease.url");
    // The visible wording moved into the dictionary layer (#4934). Assert the
    // rendered contract — the EN reference value plus the page's use of it —
    // instead of a raw TSX string, and hold every locale to a real value.
    expect(homepage).toContain("d.sourceCandidate");
    expect(homepage).toContain("d.currentSource");
    // Plain words on the marketing surface (docs/design/WEB_VOICE.md): the
    // reader sees "Unreleased v0.9.x" next to "Latest release v0.9.y", not
    // the internal "source candidate" / "provider routes" vocabulary.
    expect(EN_HOME.sourceCandidate).toBe("Unreleased");
    expect(EN_HOME.currentSource).toBe("Source");
    expect(homepage).toContain("fill(d.providerRoutes, { count: providerCount })");
    expect(EN_HOME.providerRoutes).toBe("{count} providers");
    for (const locale of ["zh", "ja", "ru", "pt-BR"]) {
      expect(getHome(locale).providerRoutes, `${locale} providerRoutes`).toContain("{count}");
      expect(getHome(locale).sourceCandidate.trim().length).toBeGreaterThan(0);
    }
    expect(homepage).not.toContain("releases/tag/v${version}");
    expect(homepage).not.toMatch(/Codewhale v0\.9\.1|\"v0\.9\.1 \u00b7/);
    expect(install).toContain("publishedRelease.tag");
    expect(install).not.toContain('"v0.8.x"');
    expect(install).not.toContain("cnbInstall(facts.version");
    expect(community).toContain("credit (unreleased)");
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

  it("keeps the models settings preview read-only, repository-driven, and responsive", () => {
    const models = pageSource("models/page.tsx");
    const styles = readFileSync(new URL("../app/globals.css", import.meta.url), "utf8");

    expect(models).toContain('className="portal-section settings-preview"');
    expect(models).toContain('isZh ? "只读设置预览" : "Read-only settings preview"');
    expect(models).toContain("facts.defaultModel");
    expect(models).toContain("facts.providers.map((provider)");
    expect(models).toContain('href={p("/docs/configuration")}');
    expect(models).toContain("does not change your local configuration");
    expect(models).toContain("不会更改你的本地配置");
    expect(models).toContain('className="settings-registry-marker"');
    expect(models).not.toContain('className="settings-status-dot"');
    expect(models).not.toMatch(/Save settings|Save changes|Apply changes|Create account|Sign up/);

    expect(styles).toContain("--settings-state-active: var(--indigo);");
    expect(styles).toContain("--settings-state-ready: var(--jade);");
    expect(styles).toContain("--settings-state-muted: var(--ink-mute);");
    expect(styles).toMatch(/\.settings-shell\s*\{[^}]*width: min\(100%, 800px\);/s);
    expect(styles).toMatch(/\.settings-shell\s*\{[^}]*grid-template-columns: 188px minmax\(0, 1fr\);/s);
    expect(styles).toMatch(/\.settings-preview a:focus-visible\s*\{[^}]*outline:/s);
    expect(styles).toMatch(/\.settings-preview a\s*\{[^}]*min-height: 44px;/s);
    expect(styles).toMatch(/\.settings-provider-code\s*\{[^}]*overflow-wrap: anywhere;/s);
    expect(styles).toMatch(/\.settings-registry-marker\s*\{[^}]*background: var\(--settings-state-muted\);/s);
    expect(styles).toMatch(/@media \(max-width: 720px\)[\s\S]*?\.settings-shell\s*\{[^}]*grid-template-columns: 1fr;/);
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
