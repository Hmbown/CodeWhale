/**
 * truncate.ts — code-point-safe truncation for text the repository owns
 * rather than the site.
 *
 * `String.prototype.slice` counts UTF-16 code units, so a cut can land
 * between the two halves of a surrogate pair. GitHub issue, pull request,
 * and release titles routinely carry emoji, and the resulting lone surrogate
 * is not a character: it renders as U+FFFD (the black-diamond question
 * mark) in every browser, immediately before the ellipsis that says the text
 * was shortened.
 */

/**
 * `value` unchanged when it is at most `limit` characters long; otherwise its
 * first `keep` characters (default: `limit`) followed by `ellipsis`.
 *
 * Characters are Unicode code points, so an astral character is either kept
 * whole or dropped whole.
 */
export function truncateChars(
  value: string,
  limit: number,
  keep: number = limit,
  ellipsis = "…",
): string {
  const chars = Array.from(value);
  if (chars.length <= limit) return value;
  return chars.slice(0, Math.max(0, keep)).join("") + ellipsis;
}
