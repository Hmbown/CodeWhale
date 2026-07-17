/**
 * release-credits.ts — community credit data for the website.
 *
 * CANONICAL SOURCE: docs/CONTRIBUTORS.md
 *   That file is the full per-PR contributor record in chronological order.
 *   This module is a snapshot of the latest release's merged/harvested
 *   contributors and issue helpers, kept in sync by the release process.
 *   When cutting a release, update both docs/CONTRIBUTORS.md AND this file.
 *
 * EXTENSION PATH FOR NEW LOCALES:
 *   The arrays below are locale-agnostic (GitHub handles). Locale-specific
 *   section labels and descriptions live in the page component. To add a
 *   new locale, update the page copy — no data changes needed here.
 *
 * See also:
 *   - .github/AUTHOR_MAP for identity mapping
 *   - CHANGELOG.md for the full release narrative
 *   - https://github.com/Hmbown/CodeWhale/graphs/contributors for the live list
 */

/** Contributors whose PRs were merged or harvested into this release. */
export const RELEASE_CONTRIBUTORS: string[] = [
  "@h3c-hexin",
  "@shenjackyuanjie",
  "@shenyongqing",
  "@sternelee",
  "@nightt5879",
  "@zhangweiii",
];

/** Contributors who helped with reports, reproductions, and verification. */
export const RELEASE_HELPERS: string[] = [
  "@AiurArtanis",
  "@seanthefuturegorilla",
];
