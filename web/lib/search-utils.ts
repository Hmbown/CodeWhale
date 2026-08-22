/**
 * search-utils.ts — shared keyword-search utilities for docs and FAQ.
 *
 * Pure functions extracted from the client components so they can be unit-tested
 * without a DOM. Used by DocsSearch and FaqSearch.
 */

import type { DocTopic } from "./docs-map";

const CATEGORY_LABELS: Record<string, { en: string; zh: string }> = {
  "getting-started": { en: "Getting started", zh: "入门" },
  "core-concepts": { en: "Core concepts", zh: "核心概念" },
  reference: { en: "Reference", zh: "参考" },
  extending: { en: "Extending", zh: "扩展" },
  operations: { en: "Operations & community", zh: "运维与社区" },
};

/**
 * Build a lowercase haystack string for a DocTopic, searching across both
 * locales, source files, category name, and id/slug.
 */
export function docTopicHaystack(t: DocTopic): string {
  const sources = Array.isArray(t.repoSource) ? t.repoSource : [t.repoSource];
  const parts = [
    t.id,
    t.slug,
    t.label.en,
    t.label.zh,
    t.description.en,
    t.description.zh,
    ...sources,
    t.category,
    CATEGORY_LABELS[t.category]?.en ?? "",
    CATEGORY_LABELS[t.category]?.zh ?? "",
  ];
  return parts.join(" ").toLowerCase();
}

/**
 * Filter DocTopics by keyword query. Returns indices into the input array.
 * Empty/whitespace query returns all indices.
 */
export function filterDocTopics(topics: DocTopic[], query: string): number[] {
  const q = query.trim().toLowerCase();
  if (!q) return topics.map((_, i) => i);
  return topics
    .map((t, i) => ({ i, hay: docTopicHaystack(t) }))
    .filter(({ hay }) => hay.includes(q))
    .map(({ i }) => i);
}

/**
 * Normalize a query for matching.
 */
export function normalizeQuery(query: string): string {
  return query.trim().toLowerCase();
}

/**
 * Check whether a query matches a haystack (case-insensitive substring).
 */
export function matches(haystack: string, query: string): boolean {
  const q = normalizeQuery(query);
  if (!q) return true;
  return haystack.toLowerCase().includes(q);
}

/** The three pieces a highlighted match splits a string into. */
export interface HighlightSpan {
  before: string;
  match: string;
  after: string;
}

/**
 * Locate `query` inside `text`, case-insensitively, in `text`'s own indices.
 *
 * The obvious form — `text.toLowerCase().indexOf(q)`, then slicing `text`
 * with that index — assumes lowercasing preserves length. It does not:
 * `"İ".toLowerCase()` is two code units, so every index after a dotted
 * capital I in the haystack is off by one and the highlight lands on the
 * wrong characters. Turkish is a routed locale, so this is reachable the
 * moment localized copy enters the search haystack.
 *
 * Lowercasing character by character and keeping a position map costs one
 * pass and keeps the three returned pieces exactly reassembling `text`.
 * Returns null when there is no match (including an empty query).
 */
export function highlightSpan(text: string, query: string): HighlightSpan | null {
  const q = normalizeQuery(query);
  if (!q) return null;

  let lower = "";
  // For each code unit of `lower`: where its source character starts and ends.
  const sourceStart: number[] = [];
  const sourceEnd: number[] = [];
  for (let i = 0; i < text.length; ) {
    const char = String.fromCodePoint(text.codePointAt(i)!);
    const next = i + char.length;
    const folded = char.toLowerCase();
    for (let k = 0; k < folded.length; k++) {
      sourceStart.push(i);
      sourceEnd.push(next);
    }
    lower += folded;
    i = next;
  }

  const idx = lower.indexOf(q);
  if (idx === -1) return null;

  const start = sourceStart[idx];
  const stop = idx + q.length;
  // A match ending inside one source character's expansion cannot claim half
  // of that character; take the whole character rather than nothing.
  const end = stop < lower.length ? Math.max(sourceStart[stop], sourceEnd[idx]) : text.length;

  return {
    before: text.slice(0, start),
    match: text.slice(start, end),
    after: text.slice(end),
  };
}
