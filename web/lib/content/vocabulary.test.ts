/**
 * Shared-content contracts: the locale-aware vocabulary and getting-started
 * modules that pages render from. These tests pin the copy to the repo's
 * public fact matrix and to real routes/documents, so the localization lane
 * can extend the modules without ever drifting from runtime truth.
 */
import { existsSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  ADVISORY_ROLE,
  CONTROL_MODES,
  MEASUREMENT_PRINCIPLES,
  PERMISSION_POSTURES,
  PRODUCT_TERMS,
  ROUTE_IDENTITY,
} from "./vocabulary";
import { GETTING_STARTED_STEPS, GUIDE_NEXT_LINKS } from "./getting-started";

const root = new URL("../../../", import.meta.url);

function repoText(path: string): string {
  return readFileSync(new URL(path, root), "utf8");
}

const matrix = JSON.parse(repoText("docs/public-surface-facts.json")) as {
  product: { terminology: Record<string, string> };
  control: { modes: string[]; permissionPostures: string[] };
};

const BANNED_COPY = /\bAgent mode\b|Agent 模式|\bYOLO\b|approval_mode|first-class|一级支持/;

describe("shared product vocabulary", () => {
  it("keeps the execution nouns verbatim with the public fact matrix", () => {
    expect(PRODUCT_TERMS.map((t) => t.term)).toEqual(["Fleet", "Workflow", "Lane", "Runtime"]);
    for (const term of PRODUCT_TERMS) {
      expect(term.short.en, term.term).toBe(matrix.product.terminology[term.term]);
    }
  });

  it("keeps mode and posture names exact", () => {
    expect(CONTROL_MODES.map((t) => t.term)).toEqual(matrix.control.modes);
    expect(PERMISSION_POSTURES.map((t) => t.term)).toEqual(matrix.control.permissionPostures);
    for (const term of [...CONTROL_MODES, ...PERMISSION_POSTURES]) {
      expect(term.kind).toMatch(/^mode$|^permission-posture$/);
    }
  });

  it("has complete en/zh pairs everywhere and no banned legacy copy", () => {
    const pairs = [
      ...PRODUCT_TERMS.flatMap((t) => [t.short, t.long]),
      ...[...CONTROL_MODES, ...PERMISSION_POSTURES, ...ROUTE_IDENTITY].map((t) => t.description),
      ADVISORY_ROLE.description,
      ...MEASUREMENT_PRINCIPLES,
    ];
    for (const pair of pairs) {
      expect(pair.en.trim().length).toBeGreaterThan(0);
      expect(pair.zh.trim().length).toBeGreaterThan(0);
      expect(pair.en).not.toBe(pair.zh);
      expect(`${pair.en}\n${pair.zh}`).not.toMatch(BANNED_COPY);
    }
  });

  it("states measurement truth without claiming results", () => {
    expect(MEASUREMENT_PRINCIPLES.length).toBeGreaterThanOrEqual(3);
    const combined = MEASUREMENT_PRINCIPLES.map((p) => p.en).join("\n");
    // No fabricated numbers: the module must not publish any benchmark score.
    expect(combined).not.toMatch(/\b\d+(\.\d+)?\s?(%|percent|tokens\/s|tok\/s)\b/i);
    expect(combined).toContain("no benchmark leaderboard");
    expect(combined).toContain("provider, model, requested and effective reasoning");
  });

  it("separates requested and effective reasoning and names route provenance", () => {
    const requested = ROUTE_IDENTITY.find((r) => r.term === "Requested reasoning");
    const effective = ROUTE_IDENTITY.find((r) => r.term === "Effective reasoning");
    const source = ROUTE_IDENTITY.find((r) => r.term === "Routing source");
    expect(requested).toBeTruthy();
    expect(effective?.description.en).toContain("unavailable");
    expect(source?.description.en).toContain("provenance");
    for (const tier of ["off", "low", "medium", "high", "max", "auto"]) {
      expect(requested!.description.en).toContain(tier);
    }
  });

  it("uses Advisor publicly and keeps old advisory names as aliases only", () => {
    expect(ADVISORY_ROLE.term).toBe("Advisor");
    expect(ADVISORY_ROLE.description.en).toContain("oracle");
    expect(ADVISORY_ROLE.description.en).toContain("consultant");
    expect(ADVISORY_ROLE.description.en).toContain("compatibility aliases");
  });
});

describe("shared getting-started path", () => {
  it("keeps the four-step order: install → offline session → provider → fleet", () => {
    expect(GETTING_STARTED_STEPS.map((s) => s.id)).toEqual([
      "install",
      "first-session",
      "connect-provider",
      "fleet-workflow",
    ]);
  });

  it("points every step and next link at a real on-site route", () => {
    const knownRoutes = [
      "/install",
      "/models",
      "/docs",
      "/docs/guide",
      "/docs/vocabulary",
      "/docs/fleet",
      "/docs/hooks",
      "/docs/modes",
    ];
    for (const step of GETTING_STARTED_STEPS) {
      expect(knownRoutes, step.link.href).toContain(step.link.href);
      expect(step.title.en.trim().length).toBeGreaterThan(0);
      expect(step.title.zh.trim().length).toBeGreaterThan(0);
      expect(`${step.body.en}\n${step.body.zh}`).not.toMatch(BANNED_COPY);
    }
    for (const link of GUIDE_NEXT_LINKS) {
      expect(knownRoutes, link.href).toContain(link.href);
    }
    // Hooks discovery is a first-class next step, not buried prose.
    expect(GUIDE_NEXT_LINKS.some((l) => l.href === "/docs/hooks")).toBe(true);
  });

  it("describes the first session truthfully: keyless launch, provider for replies", () => {
    const first = GETTING_STARTED_STEPS.find((s) => s.id === "first-session")!;
    expect(first.body.en).toContain("without any API key");
    expect(first.body.en).toContain("Plan mode");
    expect(first.body.en).toMatch(/Model replies need a provider/);
    // The keyless-launch claim must stay backed by documented runtime
    // behavior. Assert the meaning docs/GUIDE.md owes this step -- a first
    // launch that asks only for the decisions still needed, and a provider
    // step that keeps an explicit offline route -- rather than one frozen
    // sentence, which is what broke when the first-run flow was rewritten.
    // Heading-level hashes only: shell comments inside the fenced install
    // snippets start with a single "#" and must not end the section slice.
    const guide = repoText("docs/GUIDE.md");
    const firstLaunch = guide.split(/^#{2,} .*First Launch.*$/m)[1]?.split(/^#{2,} /m)[0] ?? "";
    expect(firstLaunch, "docs/GUIDE.md must keep a First Launch section").not.toBe("");
    expect(firstLaunch).toMatch(/asks only for decisions/i);
    expect(firstLaunch).toMatch(/offline route/i);
  });

  it("uses only documented commands", () => {
    const guide = repoText("docs/GUIDE.md");
    // docs/FLEET.md keeps its filename (published links, receipts) while
    // leading with the Fleet noun; see its naming/compatibility section.
    const fleetDoc = repoText("docs/FLEET.md");
    const install = GETTING_STARTED_STEPS.find((s) => s.id === "install")!;
    expect(install.commands).toContain("npm install -g codewhale");
    expect(guide).toContain("codewhale doctor");
    const provider = GETTING_STARTED_STEPS.find((s) => s.id === "connect-provider")!;
    expect(provider.commands).toContain("codewhale auth set --provider deepseek");
    expect(guide).toContain("codewhale auth set --provider deepseek");
    const fleet = GETTING_STARTED_STEPS.find((s) => s.id === "fleet-workflow")!;
    for (const command of fleet.commands) {
      expect(`${fleetDoc}\n${guide}`, command).toContain(command);
    }
  });

  it("keeps repo source documents resolvable", () => {
    for (const path of ["docs/GUIDE.md", "docs/KEYBINDINGS.md", "docs/FLEET.md", "docs/MODES.md"]) {
      expect(existsSync(new URL(path, root)), path).toBe(true);
    }
  });
});
