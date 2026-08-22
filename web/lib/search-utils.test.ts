import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import {
  docTopicHaystack,
  filterDocTopics,
  highlightSpan,
  normalizeQuery,
  matches,
} from "./search-utils";
import { DOC_TOPICS } from "./docs-map";

describe("normalizeQuery", () => {
  it("trims and lowercases", () => {
    expect(normalizeQuery("  Hello World  ")).toBe("hello world");
  });

  it("returns empty string for whitespace-only input", () => {
    expect(normalizeQuery("   ")).toBe("");
  });
});

describe("matches", () => {
  it("returns true for empty query (shows everything)", () => {
    expect(matches("anything", "")).toBe(true);
    expect(matches("anything", "   ")).toBe(true);
  });

  it("does case-insensitive substring matching", () => {
    expect(matches("Install Guide", "install")).toBe(true);
    expect(matches("install guide", "INSTALL")).toBe(true);
    expect(matches("Configuration", "config")).toBe(true);
  });

  it("returns false when no match", () => {
    expect(matches("Install", "docker")).toBe(false);
  });
});

describe("docTopicHaystack", () => {
  it("includes the topic id and slug", () => {
    const install = DOC_TOPICS.find((t) => t.id === "install")!;
    const hay = docTopicHaystack(install);
    expect(hay).toContain("install");
  });

  it("includes both EN and ZH labels", () => {
    const mcp = DOC_TOPICS.find((t) => t.id === "mcp")!;
    const hay = docTopicHaystack(mcp);
    expect(hay).toContain("mcp");
    // ZH label is also "MCP" but description has Chinese
    expect(hay).toContain("stdio");
    expect(hay).toContain("工具"); // tools in Chinese description
  });

  it("includes source file paths", () => {
    const config = DOC_TOPICS.find((t) => t.id === "configuration")!;
    const hay = docTopicHaystack(config);
    expect(hay).toContain("docs/configuration.md");
    expect(hay).toContain("docs/legacy_paths.md");
  });

  it("includes category name in both locales", () => {
    const install = DOC_TOPICS.find((t) => t.id === "install")!;
    const hay = docTopicHaystack(install);
    expect(hay).toContain("getting-started");
    expect(hay).toContain("入门"); // ZH for getting-started
  });
});

describe("filterDocTopics", () => {
  it("returns all indices when query is empty", () => {
    const result = filterDocTopics(DOC_TOPICS, "");
    expect(result.length).toBe(DOC_TOPICS.length);
  });

  it("returns all indices when query is whitespace", () => {
    const result = filterDocTopics(DOC_TOPICS, "   ");
    expect(result.length).toBe(DOC_TOPICS.length);
  });

  it("filters by English keyword", () => {
    const result = filterDocTopics(DOC_TOPICS, "install");
    const ids = result.map((i) => DOC_TOPICS[i].id);
    expect(ids).toContain("install");
    expect(ids.length).toBeGreaterThanOrEqual(1);
  });

  it("filters by Chinese keyword", () => {
    const result = filterDocTopics(DOC_TOPICS, "沙箱"); // sandbox
    const ids = result.map((i) => DOC_TOPICS[i].id);
    expect(ids).toContain("sandbox");
  });

  it("filters by source file name", () => {
    const result = filterDocTopics(DOC_TOPICS, "configuration.md");
    const ids = result.map((i) => DOC_TOPICS[i].id);
    expect(ids).toContain("configuration");
  });

  it("filters by category", () => {
    const result = filterDocTopics(DOC_TOPICS, "extending");
    const ids = result.map((i) => DOC_TOPICS[i].id);
    // extending category includes mcp, hooks, runtime-api
    expect(ids).toContain("mcp");
    expect(ids).toContain("hooks");
    expect(ids).toContain("runtime-api");
  });

  it("is case-insensitive", () => {
    const lower = filterDocTopics(DOC_TOPICS, "mcp");
    const upper = filterDocTopics(DOC_TOPICS, "MCP");
    expect(lower).toEqual(upper);
  });

  it("returns empty array for gibberish query", () => {
    const result = filterDocTopics(DOC_TOPICS, "zzzzzzz_nonexistent");
    expect(result).toEqual([]);
  });

  it("matches partial keywords", () => {
    const result = filterDocTopics(DOC_TOPICS, "tool");
    const ids = result.map((i) => DOC_TOPICS[i].id);
    // "tool" should match "tools" topic (id contains "tool")
    expect(ids).toContain("tools");
  });

  it("matches across locales (EN query matches ZH content)", () => {
    // "安装" is the ZH label for "install"
    const result = filterDocTopics(DOC_TOPICS, "安装");
    const ids = result.map((i) => DOC_TOPICS[i].id);
    expect(ids).toContain("install");
  });
});

describe("highlightSpan", () => {
  const webRoot = new URL("../", import.meta.url);
  const read = (p: string) => readFileSync(new URL(p, webRoot), "utf8");

  it("splits a plain match into three reassembling pieces", () => {
    const span = highlightSpan("Install Guide", "install")!;
    expect(span).toEqual({ before: "", match: "Install", after: " Guide" });
    expect(span.before + span.match + span.after).toBe("Install Guide");
  });

  it("returns null for an empty query or no match", () => {
    expect(highlightSpan("Install", "")).toBeNull();
    expect(highlightSpan("Install", "   ")).toBeNull();
    expect(highlightSpan("Install", "docker")).toBeNull();
    expect(highlightSpan("", "install")).toBeNull();
  });

  it("keeps indices in the source string when lowercasing changes length", () => {
    // "İ".toLowerCase() is two code units, so a lowercased copy of this
    // string is one longer than the string itself. Index arithmetic done on
    // the copy highlighted "tanbul " instead of "stanbul".
    const text = "İstanbul kurulumu";
    expect(text.toLowerCase().length).toBe(text.length + 1);
    const span = highlightSpan(text, "stanbul")!;
    expect(span.match).toBe("stanbul");
    expect(span.before).toBe("İ");
    expect(span.after).toBe(" kurulumu");
    expect(span.before + span.match + span.after).toBe(text);
  });

  it("never claims half of a source character", () => {
    const span = highlightSpan("İstanbul", "i")!;
    expect(span.match).toBe("İ");
    expect(span.before + span.match + span.after).toBe("İstanbul");
  });

  it("is the one match rule both search surfaces use", () => {
    for (const file of ["components/docs-search.tsx", "components/faq-search.tsx"]) {
      const source = read(file);
      expect(source, file).toContain("highlightSpan(text, query)");
      expect(source, file).not.toContain("text.toLowerCase()");
      expect(source, file).not.toContain("text.slice(");
    }
  });
});
