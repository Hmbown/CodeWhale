import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { CHANGELOG } from "./changelog.generated";
import { FACTS } from "./facts.generated";
import {
  changelogAnchor,
  clip,
  parseChangelog,
  plainText,
  renderChangelogModule,
} from "../scripts/changelog-lib.mjs";

const root = new URL("../../", import.meta.url);

describe("changelog derivation", () => {
  it("keeps lib/changelog.generated.ts in exact parity with CHANGELOG.md", () => {
    // The drift gate: a clean re-derivation must reproduce the tracked file
    // byte for byte, so the /changelog route can never show a stale record.
    const source = readFileSync(new URL("CHANGELOG.md", root), "utf8");
    const committed = readFileSync(new URL("../lib/changelog.generated.ts", import.meta.url), "utf8");
    expect(renderChangelogModule(parseChangelog(source))).toBe(committed);
  });

  it("puts the unreleased lane first and the published version right behind it", () => {
    expect(CHANGELOG[0].unreleased).toBe(true);
    expect(CHANGELOG[0].version).toBe("Unreleased");
    expect(CHANGELOG[0].date).toBeNull();
    // The source candidate the site advertises must be the next heading —
    // either as a dated release (published) or an "Unreleased candidate".
    expect(CHANGELOG[1].version).toBe(FACTS.version);
    expect(CHANGELOG[1].unreleased).toBe(false);
    for (const release of CHANGELOG.slice(1)) {
      expect(release.version).toMatch(/^\d+\.\d+\.\d+$/);
      expect(release.compareUrl, release.version).toMatch(
        /^https:\/\/github\.com\/Hmbown\/CodeWhale\/compare\//,
      );
      expect(release.sections.length, release.version).toBeGreaterThan(0);
      for (const section of release.sections) {
        expect(section.items.length).toBeGreaterThan(0);
        expect(section.items.length).toBeLessThanOrEqual(12);
        expect(section.itemCount).toBeGreaterThanOrEqual(section.items.length);
      }
    }
  });

  it("parses Keep-a-Changelog structure with continued bullets and compare links", () => {
    const md = `# Changelog

## [Unreleased]

### Added

- First entry that
  continues on the next line.
- Second **bold** entry with [a link](https://example.com) and \`code\`.

## [1.2.3] - 2026-01-02

### Fixed

- Only fix.

Trailing prose that is not a bullet.

[Unreleased]: https://github.com/Hmbown/CodeWhale/compare/v1.2.3...HEAD
[1.2.3]: https://github.com/Hmbown/CodeWhale/compare/v1.2.2...v1.2.3
`;
    const { releases } = parseChangelog(md);
    expect(releases).toHaveLength(2);
    expect(releases[0]).toMatchObject({
      version: "Unreleased",
      unreleased: true,
      date: null,
      compareUrl: "https://github.com/Hmbown/CodeWhale/compare/v1.2.3...HEAD",
    });
    expect(releases[0].sections[0].items).toEqual([
      "First entry that continues on the next line.",
      "Second bold entry with a link and code.",
    ]);
    expect(releases[1]).toMatchObject({ version: "1.2.3", date: "2026-01-02", unreleased: false });
    expect(releases[1].sections[0].items).toEqual(["Only fix."]);
  });

  it("keeps most of the release record readable in place", () => {
    // The web clip is a courtesy, not the record: the page must show more
    // than it hides, and the per-release link carries whatever it does hide.
    const items = CHANGELOG.flatMap((r) => r.sections.flatMap((s) => s.items));
    const clipped = items.filter((item) => item.endsWith("…")).length;
    expect(clipped / items.length).toBeLessThan(0.25);
  });

  it("names the GitHub heading anchor for every release heading", () => {
    expect(changelogAnchor({ version: "Unreleased", date: null, unreleased: true })).toBe("unreleased");
    expect(changelogAnchor({ version: "0.9.11", date: "2026-08-22", unreleased: false })).toBe(
      "0911---2026-08-22",
    );
    expect(changelogAnchor({ version: "1.0.0", date: null, unreleased: false })).toBe("100");
  });

  it("clips long entries at a word boundary and never mid-word", () => {
    const long = Array.from({ length: 60 }, (_, i) => `word${i}`).join(" ");
    const clipped = clip(long, 100);
    expect(clipped.length).toBeLessThanOrEqual(101);
    expect(clipped.endsWith("…")).toBe(true);
    expect(clipped.slice(0, -1).trimEnd()).toMatch(/word\d+$/);
    expect(clip("short", 100)).toBe("short");
    expect(plainText("  **a**   [b](c) `d`  ")).toBe("a b d");
  });
});
