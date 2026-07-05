/**
 * docs-search.ts — client-safe docs search: index types + ranking.
 *
 * The index itself is generated at build time (lib/docs-search-index.ts →
 * app/docs-search-index.json/route.ts, statically prerendered like
 * /llms-full.txt). This module is imported by the client search component,
 * so it must stay dependency-free and small — no `marked`, no `node:fs`.
 *
 * Ranking is a deliberately simple additive score, not a search library:
 * every query term must appear somewhere in the entry (AND semantics);
 * matches in headings and page titles outrank body-text matches, and
 * word-start matches outrank mid-word ones.
 */

export interface SearchEntry {
  /** Docs slug — the /[locale]/docs/<slug> route. */
  slug: string;
  /** Page label per locale (from docs-map). */
  page: { en: string; zh: string };
  /** Section anchor id on the rendered page; "" links to the page top. */
  anchor: string;
  /** Section heading text; "" for the page/source intro before any heading. */
  heading: string;
  /** Plain-text section content (markdown stripped, length-capped). */
  text: string;
}

export interface SearchResult {
  entry: SearchEntry;
  score: number;
  /** Short plain-text excerpt centered on the first body-text match. */
  excerpt: string;
}

/** Split a query into lowercase terms. */
export function queryTerms(query: string): string[] {
  return query.toLowerCase().split(/\s+/).filter((t) => t.length > 0);
}

/** Score one term against a haystack: 0 = no match, 1 = substring, 2 = word start. */
function termScore(haystack: string, term: string): number {
  const idx = haystack.indexOf(term);
  if (idx === -1) return 0;
  if (idx === 0 || /[^\p{L}\p{N}]/u.test(haystack[idx - 1])) return 2;
  return 1;
}

/** Build a short excerpt around the first occurrence of any term. */
function makeExcerpt(text: string, terms: string[], width = 140): string {
  const lower = text.toLowerCase();
  let at = -1;
  for (const t of terms) {
    const idx = lower.indexOf(t);
    if (idx !== -1 && (at === -1 || idx < at)) at = idx;
  }
  if (at === -1) return text.slice(0, width);
  const start = Math.max(0, at - Math.floor(width / 3));
  const slice = text.slice(start, start + width);
  return (start > 0 ? "…" : "") + slice + (start + width < text.length ? "…" : "");
}

/**
 * Rank index entries against a query. Returns at most `limit` results,
 * best first. Empty/short queries return no results.
 */
export function searchDocs(
  index: SearchEntry[],
  query: string,
  limit = 8,
): SearchResult[] {
  const terms = queryTerms(query);
  if (terms.length === 0 || query.trim().length < 2) return [];
  const phrase = query.trim().toLowerCase();

  const results: SearchResult[] = [];
  for (const entry of index) {
    const heading = entry.heading.toLowerCase();
    const page = `${entry.page.en} ${entry.page.zh}`.toLowerCase();
    const text = entry.text.toLowerCase();

    let score = 0;
    let allMatch = true;
    for (const term of terms) {
      const h = termScore(heading, term);
      const p = termScore(page, term);
      const b = termScore(text, term);
      if (h === 0 && p === 0 && b === 0) {
        allMatch = false;
        break;
      }
      score += h * 4 + p * 3 + b;
    }
    if (!allMatch) continue;

    // Phrase bonuses for multi-word queries.
    if (terms.length > 1) {
      if (heading.includes(phrase)) score += 10;
      else if (text.includes(phrase)) score += 4;
    }
    // Slight preference for section hits over page-top intros.
    if (entry.anchor !== "") score += 1;

    results.push({ entry, score, excerpt: makeExcerpt(entry.text, terms) });
  }

  results.sort(
    (a, b) => b.score - a.score || a.entry.slug.localeCompare(b.entry.slug),
  );
  return results.slice(0, limit);
}
