import Link from "next/link";
import { DocsBreadcrumb } from "@/components/docs-breadcrumb";
import { DocsHelp } from "@/components/docs-help";
import { DocsSidebar } from "@/components/docs-sidebar";
import { ReleaseTruth } from "@/components/release-truth";
import { Whale } from "@/components/whale";
import { BUILD_FACTS, getFactsWithProvenance } from "@/lib/facts";
import { getDocsShell } from "@/lib/i18n/dictionaries";

/* ------------------------------------------------------------------ */
/*  Layout (Next.js App Router)                                        */
/* ------------------------------------------------------------------ */

/**
 * The docs shell every documentation URL shares: the portal hero with the
 * version-aware release-truth line, the breadcrumb + content + sidebar
 * grid, and the contextual help band that closes each page. All copy is
 * dictionary-driven; the release line reads the facts layer.
 */
export default async function DocsLayout({
  children,
  params,
}: {
  children: React.ReactNode;
  params: Promise<{ locale: string }>;
}) {
  const { locale } = await params;
  const t = getDocsShell(locale);
  // These pages describe THIS build: pin the documented facts to the build
  // snapshot even when the KV snapshot was written by a newer source (a
  // rollback, or any deployment sharing it). Only the latest published
  // release is the KV snapshot's to speak for.
  const resolution = await getFactsWithProvenance();
  const facts = {
    ...BUILD_FACTS,
    latestPublishedRelease: resolution.facts.latestPublishedRelease,
  };

  return (
    <div className="docs-theme docs-portal min-h-screen">
      <section className="hero">
        <div className="portal-current" aria-hidden="true" />
        <div className="portal-container docs-portal-hero-inner">
          <div className="portal-mark">
            <Whale size={28} />
            <span>{t.portalMark}</span>
          </div>
          {/* Shell chrome, not the page heading: this line is identical on all
              docs URLs, so each page owns its own <h1> (its topic) and this
              keeps the hero's display size without claiming the heading rank. */}
          <p className="docs-hero-title">{t.heroTitle}</p>
          <p>{t.heroLead}</p>
          <div className="portal-actions">
            <Link href={`/${locale}/install`} className="portal-button portal-button-primary">
              {t.installCta}
            </Link>
            <Link
              href="https://github.com/Hmbown/CodeWhale/tree/main/docs"
              target="_blank"
              rel="noreferrer"
              className="portal-button portal-button-secondary"
            >
              {t.sourceDocsCta}
            </Link>
          </div>
          <ReleaseTruth locale={locale} facts={facts} />
        </div>
      </section>

      <div className="portal-container docs-shell min-w-0">
        <article className="docs-content min-w-0">
          <DocsBreadcrumb locale={locale} />
          {children}
          <DocsHelp locale={locale} />
        </article>
        <DocsSidebar locale={locale} />
      </div>
    </div>
  );
}
