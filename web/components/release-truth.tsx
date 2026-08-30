import Link from "next/link";
import type { RepoFacts } from "@/lib/facts";
import { fill, getChrome, getDocsShell } from "@/lib/i18n/dictionaries";

/**
 * Version-aware release truth, as one line of chrome: the latest published
 * release from the facts layer, and whether the pages describe that release
 * or the newer source candidate. Both values come from
 * `lib/facts.generated.ts` (build) or the KV snapshot (deployed); nothing is
 * typed by hand.
 */
export function ReleaseTruth({ locale, facts }: { locale: string; facts: RepoFacts }) {
  const t = getDocsShell(locale);
  const chrome = getChrome(locale);
  const published = facts.latestPublishedRelease;
  const version = facts.version;
  const matches = published !== null && version !== null && published.version === version;

  return (
    <div className="release-truth dotline" data-release-state={matches ? "published" : "candidate"}>
      <span className="release-truth-label">{t.releaseLabel}</span>
      {published && (
        <a href={published.url} target="_blank" rel="noreferrer">
          {fill(t.releasePublished, {
            tag: published.tag,
            date: new Date(published.publishedAt).toLocaleDateString(chrome.dateLocale, {
              year: "numeric",
              month: "short",
              day: "numeric",
              timeZone: "UTC",
            }),
          })}
        </a>
      )}
      {version && (
        <span>
          {matches
            ? fill(t.releaseMatches, { tag: `v${version}` })
            : fill(t.releaseCandidate, { version: `v${version}` })}
        </span>
      )}
      <Link href={`/${locale}/changelog`}>{t.releaseChangelog}</Link>
    </div>
  );
}
