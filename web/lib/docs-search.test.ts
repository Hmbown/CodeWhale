import { describe, expect, it } from "vitest";
import { searchDocs, queryTerms, type SearchEntry } from "./docs-search";
import { extractSections, stripMarkdown } from "./docs-search-index";
import { Slugger, renderDocMarkdown } from "./markdown";

/* ------------------------------------------------------------------ */
/*  stripMarkdown                                                      */
/* ------------------------------------------------------------------ */

describe("stripMarkdown", () => {
  it("strips links, code ticks, emphasis, tables, and collapses whitespace", () => {
    expect(stripMarkdown("See [MODES.md](MODES.md) for `/*bold*` **rules**.")).toBe(
      "See MODES.md for /bold rules.",
    );
    expect(stripMarkdown("| a | b |\n|:--|:--|\n| 1 | 2 |")).toBe("a b 1 2");
  });

  it("keeps fenced code content but drops the fence lines", () => {
    expect(stripMarkdown("```bash\ncodewhale --resume\n```")).toBe(
      "codewhale --resume",
    );
  });
});

/* ------------------------------------------------------------------ */
/*  extractSections                                                    */
/* ------------------------------------------------------------------ */

const SAMPLE = `# Rollback and Restore

Intro paragraph about snapshots.

## How snapshots work

Every workspace gets a side git repository.

## \`/restore\` — revert files

Run \`/restore 3\` to roll back.
`;

describe("extractSections", () => {
  it("splits at headings with anchors matching the renderer", () => {
    const sections = extractSections(SAMPLE, new Slugger());
    const rendered = renderDocMarkdown(SAMPLE, {
      sourceRepoPath: "docs/RESTORE.md",
      locale: "en",
    });
    // Every section anchor must exist as a rendered heading id.
    const ids = new Set(rendered.headings.map((h) => h.id));
    for (const s of sections) {
      expect(ids.has(s.anchor)).toBe(true);
    }
    expect(sections.map((s) => s.heading)).toEqual([
      "Rollback and Restore",
      "How snapshots work",
      "/restore — revert files",
    ]);
    expect(sections[1].text).toContain("side git repository");
  });

  it("captures intro text before the first heading with an empty anchor", () => {
    const sections = extractSections("plain intro\n\n## First\n\nbody", new Slugger());
    expect(sections[0]).toMatchObject({ anchor: "", heading: "" });
    expect(sections[0].text).toBe("plain intro");
  });
});

/* ------------------------------------------------------------------ */
/*  searchDocs                                                         */
/* ------------------------------------------------------------------ */

const INDEX: SearchEntry[] = [
  {
    slug: "restore",
    page: { en: "Rollback & Restore", zh: "回滚与恢复" },
    anchor: "how-snapshots-work",
    heading: "How snapshots work",
    text: "Every workspace gets a side git repository stored under snapshots.",
  },
  {
    slug: "guide",
    page: { en: "User Guide", zh: "使用指南" },
    anchor: "first-launch",
    heading: "First launch",
    text: "Run codewhale in a repo. Snapshots protect every turn.",
  },
  {
    slug: "mcp",
    page: { en: "MCP", zh: "MCP" },
    anchor: "",
    heading: "",
    text: "Model Context Protocol servers over stdio and HTTP.",
  },
];

describe("queryTerms", () => {
  it("lowercases and splits on whitespace", () => {
    expect(queryTerms("  Fleet  Ledger ")).toEqual(["fleet", "ledger"]);
  });
});

describe("searchDocs", () => {
  it("returns nothing for empty or single-char queries", () => {
    expect(searchDocs(INDEX, "")).toEqual([]);
    expect(searchDocs(INDEX, "s")).toEqual([]);
  });

  it("ranks heading matches above body matches", () => {
    const results = searchDocs(INDEX, "snapshots");
    expect(results.length).toBe(2);
    expect(results[0].entry.slug).toBe("restore"); // heading hit
    expect(results[1].entry.slug).toBe("guide"); // body-only hit
  });

  it("requires every term to match (AND semantics)", () => {
    expect(searchDocs(INDEX, "snapshots nonexistentterm")).toEqual([]);
  });

  it("matches page titles and zh labels", () => {
    expect(searchDocs(INDEX, "mcp")[0].entry.slug).toBe("mcp");
    expect(searchDocs(INDEX, "回滚")[0].entry.slug).toBe("restore");
  });

  it("produces an excerpt around the first match", () => {
    const [r] = searchDocs(INDEX, "side git");
    expect(r.excerpt).toContain("side git repository");
  });

  it("respects the limit", () => {
    expect(searchDocs(INDEX, "snapshots", 1).length).toBe(1);
  });
});
