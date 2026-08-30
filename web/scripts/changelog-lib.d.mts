/**
 * Types for changelog-lib.mjs so `lib/changelog.test.ts` can import the
 * parser under `allowJs: false`. Keep in step with the runtime module.
 */
export interface ParsedChangelogSection {
  heading: string;
  items: string[];
  itemCount: number;
}

export interface ParsedChangelogRelease {
  version: string;
  date: string | null;
  unreleased: boolean;
  compareUrl: string | null;
  sections: ParsedChangelogSection[];
}

export interface ParsedChangelog {
  releases: ParsedChangelogRelease[];
}

export interface ParseChangelogOptions {
  limit?: number;
  itemsPerSection?: number;
  itemChars?: number;
}

export function plainText(markdown: string): string;
export function clip(text: string, max?: number): string;
export function parseChangelog(markdown: string, options?: ParseChangelogOptions): ParsedChangelog;
export function changelogAnchor(release: {
  version: string;
  date: string | null;
  unreleased: boolean;
}): string;
export function renderChangelogModule(parsed: ParsedChangelog, sourcePath?: string): string;
