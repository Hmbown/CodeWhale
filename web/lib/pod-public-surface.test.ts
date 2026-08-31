/**
 * Pod is the canonical public noun; Fleet is the compatibility spelling.
 *
 * Codewhale 0.9.12 makes Pod the customer-facing name for the durable roster
 * across the CLI, the TUI slash command, the guide, and this website. Fleet is
 * deliberately *not* removed: it stays the serialization name (ledger file,
 * saved rosters, config tables, the `workflow --fleet` flag, control-plane
 * operation ids) and an accepted command alias.
 *
 * These are deterministic source contracts, in the style of docs-ia.test.ts:
 * they read the real registry, sitemap, routes, and repo docs, so a future
 * rename fails here instead of silently splitting discovery across two nouns.
 */
import { existsSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import buildSitemap from "../app/sitemap";
import { DOC_TOPICS, docTopicHref, getTopic } from "./docs-map";
import { filterDocTopics } from "./search-utils";
import { PRODUCT_TERMS } from "./content/vocabulary";
import { GETTING_STARTED_STEPS } from "./content/getting-started";
import { buildLlmsTxt } from "./llms-txt";
import { SITE_URL } from "./page-meta";

const webRoot = new URL("../", import.meta.url);
const repoRoot = new URL("../../", import.meta.url);

function webText(path: string): string {
  return readFileSync(new URL(path, webRoot), "utf8");
}

function repoText(path: string): string {
  return readFileSync(new URL(path, repoRoot), "utf8");
}

const sitemapEntries = buildSitemap();

describe("Pod is the canonical public surface", () => {
  it("registers exactly one roster topic, slugged /docs/pod", () => {
    const pod = getTopic("pod");
    expect(pod?.hasPage).toBe(true);
    expect(pod?.slug).toBe("pod");
    expect(pod?.label.en).toContain("Pod");
    expect(pod?.label.en).not.toContain("Fleet");
    expect(docTopicHref(pod!, "en")).toBe("/en/docs/pod");
    // No second topic may claim the same concept under the old noun.
    expect(DOC_TOPICS.filter((t) => t.slug === "fleet")).toHaveLength(0);
    expect(DOC_TOPICS.map((t) => t.id)).not.toContain("fleet");
  });

  it("indexes /docs/pod and keeps the legacy /docs/fleet URL out of the sitemap", () => {
    for (const locale of ["en", "zh"]) {
      expect(
        sitemapEntries.some((e) => e.url === `${SITE_URL}/${locale}/docs/pod`),
        locale,
      ).toBe(true);
    }
    expect(sitemapEntries.some((e) => e.url.endsWith("/docs/fleet"))).toBe(false);
  });

  it("serves the roster page at /docs/pod and a permanent redirect at /docs/fleet", () => {
    const page = webText("app/[locale]/docs/pod/page.tsx");
    expect(page).toContain('path: "/docs/pod"');
    expect(page).toContain('import { buildPageMetadata } from "@/lib/page-meta"');

    // The old URL is published; it must resolve, not 404, and must not be a
    // second copy of the page competing for the same search signal.
    const redirect = webText("app/[locale]/docs/fleet/page.tsx");
    expect(
      existsSync(new URL("app/[locale]/docs/fleet/page.tsx", webRoot)),
      "the legacy URL must keep a route",
    ).toBe(true);
    expect(redirect).toContain('import { permanentRedirect } from "next/navigation"');
    expect(redirect).toContain("permanentRedirect(`/${locale}/docs/pod`)");
    expect(redirect).not.toContain("buildPageMetadata");
  });

  it("names Pod, not Fleet, in the machine-readable index agents read", () => {
    const llms = buildLlmsTxt();
    expect(llms).toContain("/docs/pod");
    expect(llms).not.toContain("/docs/fleet");
  });

  it("resolves docs search on both nouns to the one Pod page", () => {
    // Someone who learned the product as Fleet must still land on the page.
    // Both queries have to return the same single topic — a Fleet search that
    // finds nothing is the failure this slice exists to prevent.
    for (const query of ["pod", "Pod", "fleet", "FLEET"]) {
      const hits = filterDocTopics(DOC_TOPICS, query).map((i) => DOC_TOPICS[i]);
      const roster = hits.filter((t) => t.slug === "pod");
      expect(roster, `search "${query}" must reach the roster page`).toHaveLength(1);
    }
  });

  it("uses Pod as the roster noun in shared vocabulary and the guided path", () => {
    expect(PRODUCT_TERMS.map((t) => t.term)).toContain("Pod");
    expect(PRODUCT_TERMS.map((t) => t.term)).not.toContain("Fleet");

    const step = GETTING_STARTED_STEPS.find((s) => s.id === "pod-workflow");
    expect(step, "the guided path ends on the Pod step").toBeTruthy();
    expect(step!.link.href).toBe("/docs/pod");
    expect(step!.commands).toContain("/pod setup");
    expect(step!.commands).toContain("codewhale pod status");
  });

  it("keeps durable Pod status separate from current-session workers", () => {
    const en = webText("lib/i18n/dictionaries/en/docs-fleet.ts");
    const zh = webText("lib/i18n/dictionaries/zh/docs-fleet.ts");
    const page = webText("app/[locale]/docs/pod/page.tsx");

    expect(page).toContain('podWorkers: "/pod workers"');

    for (const source of [en, zh]) {
      expect(source).toContain("{fleetStatusTui}");
      expect(source).toContain("{fleetStatusShell}");
      expect(source).toContain("{podWorkers}");
      expect(source).toContain("{subagents}");
      expect(source).not.toContain("{fleetStatusTui} (or {subagents})");
      expect(source).not.toContain("{fleetStatusTui}（或 {subagents}）");
    }
  });

  it("documents the named saved-Pod picker separately from members and workers", () => {
    const page = webText("app/[locale]/docs/pod/page.tsx");
    const en = webText("lib/i18n/dictionaries/en/docs-fleet.ts");
    const zh = webText("lib/i18n/dictionaries/zh/docs-fleet.ts");

    expect(page).toContain('podPods: "/pod pods"');
    for (const source of [en, zh]) {
      expect(source).toContain("{podPods}");
      expect(source).toContain("/pod setup");
      expect(source).toContain("/pod");
    }
    expect(repoText("docs/FLEET.md")).toContain("`/pod pods`");
  });
});

describe("Fleet stays a documented compatibility boundary", () => {
  it("documents both spellings and the artifacts that keep the Fleet name", () => {
    const doc = repoText("docs/FLEET.md");
    expect(doc).toContain("`codewhale pod …`");
    expect(doc).toContain("`codewhale fleet …`");
    expect(doc).toContain("`/pod …`");
    expect(doc).toContain("`/fleet …`");
    // The serialization names are a contract, not an oversight.
    for (const artifact of [
      ".codewhale/fleet.jsonl",
      "fleets/<name>.toml",
      "`[fleet]`",
      "--fleet",
      "fleet.status",
    ]) {
      expect(doc, artifact).toContain(artifact);
    }
  });

  it("keeps the primary English guide on /pod while naming the alias", () => {
    const guide = repoText("docs/GUIDE.md");
    expect(guide).toContain("| `/pod` |");
    expect(guide).not.toContain("| `/fleet` |");
    expect(guide).toContain("`codewhale pod …` is\nthe canonical command");
    expect(guide).toContain("remain accepted compatibility spellings");
  });

  it("keeps the roster term pinned verbatim to the repo fact matrix", () => {
    const matrix = JSON.parse(repoText("docs/public-surface-facts.json")) as {
      product: { terminology: Record<string, string> };
    };
    expect(Object.keys(matrix.product.terminology)).toContain("Pod");
    expect(Object.keys(matrix.product.terminology)).not.toContain("Fleet");
    const pod = PRODUCT_TERMS.find((t) => t.term === "Pod")!;
    expect(pod.short.en).toBe(matrix.product.terminology.Pod);
    expect(repoText("docs/FLEET.md")).toContain(`**Pod** = ${pod.short.en}`);
  });
});
