import Link from "next/link";
import { EmptyState } from "@/components/surface-state";
import { CHANGELOG, type ChangelogRelease } from "@/lib/changelog.generated";
import { changelogAnchor } from "@/scripts/changelog-lib.mjs";
import { getFacts } from "@/lib/facts";
import { fill, getChangelog, getChrome } from "@/lib/i18n/dictionaries";
import { REPO_RELEASES_URL, REPO_URL } from "@/lib/i18n/links";
import { buildPageMetadata } from "@/lib/page-meta";

// The two headline facts come from the facts layer, which may be refreshed
// from the KV snapshot after deploy; the notes are build-time from CHANGELOG.md.
export const revalidate = 300;

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const t = getChangelog(locale);
  return buildPageMetadata({
    path: "/changelog",
    locale,
    title: t.metaTitle,
    description: t.metaDescription,
  });
}

function formatDate(iso: string, dateLocale: string): string {
  return new Date(iso).toLocaleDateString(dateLocale, {
    year: "numeric",
    month: "short",
    day: "numeric",
    // Dates come from ISO timestamps and YYYY-MM-DD headings; render them
    // as the UTC day they name, not the server's local day.
    timeZone: "UTC",
  });
}

export default async function ChangelogPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const t = getChangelog(locale);
  const chrome = getChrome(locale);
  const facts = await getFacts();
  const published = facts.latestPublishedRelease;
  const candidate = facts.version;
  const candidateIsPublished = published !== null && candidate !== null && published.version === candidate;

  return (
    <div className="portal-home">
      <section className="hero">
        <div className="portal-current" aria-hidden="true" />
        <div className="portal-container community-welcome-inner">
          <div className="eyebrow">{t.kicker}</div>
          <h1>{t.title}</h1>
          <p>{t.lead}</p>

          <div className="changelog-facts">
            <div data-published={published ? "true" : "false"}>
              <span>{t.publishedLabel}</span>
              {published ? (
                <>
                  <strong>
                    {fill(t.publishedValue, {
                      tag: published.tag,
                      date: formatDate(published.publishedAt, chrome.dateLocale),
                    })}
                  </strong>
                  <a href={published.url} target="_blank" rel="noreferrer">
                    {t.releasePageLink}
                  </a>
                </>
              ) : (
                <a href={REPO_RELEASES_URL} target="_blank" rel="noreferrer">
                  {t.releasesLink}
                </a>
              )}
            </div>
            <div data-candidate={candidateIsPublished ? "published" : "unreleased"}>
              <span>{t.candidateLabel}</span>
              {candidate && (
                <strong>
                  {candidateIsPublished
                    ? fill(t.candidateMatches, { version: `v${candidate}` })
                    : fill(t.candidateValue, { version: `v${candidate}` })}
                </strong>
              )}
              <a href={`${REPO_URL}/blob/main/CHANGELOG.md`} target="_blank" rel="noreferrer">
                {t.fullNotes}
              </a>
            </div>
          </div>
        </div>
      </section>

      <section className="portal-section">
        <div className="portal-container" style={{ maxWidth: "56rem" }}>
          {CHANGELOG.length === 0 ? (
            <EmptyState
              locale={locale}
              title={t.emptyTitle}
              body={t.emptyBody}
              action={
                <a className="portal-button portal-button-secondary" href={REPO_RELEASES_URL}>
                  {t.releasesLink}
                </a>
              }
            />
          ) : (
            CHANGELOG.map((release) => (
              <Release
                key={release.version}
                release={release}
                locale={locale}
                publishedUrl={
                  published && published.version === release.version ? published.url : null
                }
              />
            ))
          )}
          <p className="mt-8 text-sm text-ink-mute">
            <Link href={`/${locale}/docs`} className="body-link">
              {chrome.footerDocs}
            </Link>
            {" · "}
            <a href={REPO_RELEASES_URL} target="_blank" rel="noreferrer" className="body-link">
              {t.releasesLink}
            </a>
          </p>
        </div>
      </section>
    </div>
  );
}

function Release({
  release,
  locale,
  publishedUrl,
}: {
  release: ChangelogRelease;
  locale: string;
  publishedUrl: string | null;
}) {
  const t = getChangelog(locale);
  const chrome = getChrome(locale);
  const heading = release.unreleased ? t.unreleasedHeading : `v${release.version}`;
  const anchor = release.unreleased ? "unreleased" : `v${release.version}`;
  // Every entry here is clipped or capped for the web; this is the in-page
  // path to the unabridged notes for exactly this version.
  const notesUrl = `${REPO_URL}/blob/main/CHANGELOG.md#${changelogAnchor(release)}`;

  return (
    <article id={anchor} className="changelog-release scroll-mt-32">
      <div className="changelog-release-head">
        <h2>{heading}</h2>
        <div className="dotline">
          {release.date && <span className="tabular">{formatDate(release.date, chrome.dateLocale)}</span>}
          {publishedUrl && (
            <a href={publishedUrl} target="_blank" rel="noreferrer">
              {t.releasePageLink}
            </a>
          )}
          {release.compareUrl && (
            <a href={release.compareUrl} target="_blank" rel="noreferrer">
              {t.compareLink}
            </a>
          )}
          <a href={notesUrl} target="_blank" rel="noreferrer">
            {fill(t.releaseNotesLink, { version: heading })}
          </a>
        </div>
      </div>
      {release.unreleased && <p className="changelog-unreleased-note">{t.unreleasedNote}</p>}
      <div className="changelog-sections">
        {release.sections.map((section, i) => (
          <section key={`${section.heading}-${i}`}>
            <h3>
              {section.heading}
              {section.itemCount > section.items.length && (
                <a href={notesUrl} target="_blank" rel="noreferrer">
                  {fill(t.moreEntries, { shown: section.items.length, total: section.itemCount })}
                </a>
              )}
            </h3>
            <ul>
              {section.items.map((item, j) => (
                <li key={j}>{item}</li>
              ))}
            </ul>
          </section>
        ))}
      </div>
    </article>
  );
}
