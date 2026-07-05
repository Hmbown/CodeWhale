import { describe, expect, it } from "vitest";
import {
  githubSlug,
  renderDocMarkdown,
  resolveDocHref,
  resolveDocImageSrc,
  Slugger,
} from "./markdown";
import { CUSTOM_DOC_PAGES, DOC_TOPICS } from "./docs-map";

describe("githubSlug", () => {
  it("lowercases, strips punctuation, hyphenates spaces", () => {
    expect(githubSlug("First Launch")).toBe("first-launch");
    expect(githubSlug("Modes & Approvals")).toBe("modes--approvals");
    expect(githubSlug("`config.toml` reference")).toBe("configtoml-reference");
  });
});

describe("Slugger", () => {
  it("deduplicates repeated headings", () => {
    const s = new Slugger();
    expect(s.slug("Overview")).toBe("overview");
    expect(s.slug("Overview")).toBe("overview-1");
  });
});

describe("resolveDocHref", () => {
  it("keeps absolute URLs and bare anchors", () => {
    expect(resolveDocHref("https://x.dev/a.md", "docs/GUIDE.md", "en")).toBe(
      "https://x.dev/a.md",
    );
    expect(resolveDocHref("#first-launch", "docs/GUIDE.md", "en")).toBe("#first-launch");
  });

  it("maps sibling docs with rendered pages to site routes", () => {
    expect(resolveDocHref("MODES.md", "docs/GUIDE.md", "en")).toBe("/en/docs/modes");
    expect(resolveDocHref("./MODES.md#tui-modes", "docs/GUIDE.md", "zh")).toBe(
      "/zh/docs/modes#tui-modes",
    );
    // workflows → parent docs dir
    expect(resolveDocHref("../FLEET.md", "docs/workflows/fleet-run.md", "en")).toBe(
      "/en/docs/fleet",
    );
  });

  it("falls back to GitHub for unregistered files", () => {
    expect(resolveDocHref("OPERATIONS_RUNBOOK.md", "docs/GUIDE.md", "en")).toBe(
      "https://github.com/Hmbown/CodeWhale/blob/main/docs/OPERATIONS_RUNBOOK.md",
    );
    expect(resolveDocHref("../CONTRIBUTING.md", "docs/GUIDE.md", "en")).toBe(
      "https://github.com/Hmbown/CodeWhale/blob/main/CONTRIBUTING.md",
    );
  });
});

describe("resolveDocImageSrc", () => {
  it("maps relative images to raw.githubusercontent.com", () => {
    expect(resolveDocImageSrc("../assets/demo.png", "docs/GUIDE.md")).toBe(
      "https://raw.githubusercontent.com/Hmbown/CodeWhale/main/assets/demo.png",
    );
    expect(resolveDocImageSrc("https://img.example/x.png", "docs/GUIDE.md")).toBe(
      "https://img.example/x.png",
    );
  });
});

describe("renderDocMarkdown", () => {
  it("demotes headings, assigns ids, and extracts the outline", () => {
    const md = "# Title\n\nIntro.\n\n## First Section\n\nBody with `code`.\n";
    const { html, headings } = renderDocMarkdown(md, {
      sourceRepoPath: "docs/GUIDE.md",
      locale: "en",
    });
    expect(html).toContain('<h2 id="title"');
    expect(html).toContain('<h3 id="first-section"');
    expect(headings).toEqual([
      { id: "title", text: "Title", depth: 1 },
      { id: "first-section", text: "First Section", depth: 2 },
    ]);
  });

  it("renders GFM tables and fenced code", () => {
    const md = "| a | b |\n|---|---|\n| 1 | 2 |\n\n```bash\nnpm i -g codewhale\n```\n";
    const { html } = renderDocMarkdown(md, {
      sourceRepoPath: "docs/INSTALL.md",
      locale: "en",
    });
    expect(html).toContain("<table>");
    expect(html).toContain('<code class="language-bash">');
  });

  it("rewrites relative doc links inside the rendered html", () => {
    const md = "See [modes](MODES.md) and [runbook](OPERATIONS_RUNBOOK.md).";
    const { html } = renderDocMarkdown(md, {
      sourceRepoPath: "docs/GUIDE.md",
      locale: "en",
    });
    expect(html).toContain('href="/en/docs/modes"');
    expect(html).toContain(
      'href="https://github.com/Hmbown/CodeWhale/blob/main/docs/OPERATIONS_RUNBOOK.md"',
    );
  });
});

describe("docs-map registry invariants", () => {
  it("custom pages are all registered with hasPage", () => {
    for (const slug of CUSTOM_DOC_PAGES) {
      const topic = DOC_TOPICS.find((t) => t.slug === slug);
      expect(topic, `missing topic for custom page ${slug}`).toBeDefined();
      expect(topic?.hasPage).toBe(true);
    }
  });

  it("slugs are unique", () => {
    const slugs = DOC_TOPICS.map((t) => t.slug);
    expect(new Set(slugs).size).toBe(slugs.length);
  });
});
