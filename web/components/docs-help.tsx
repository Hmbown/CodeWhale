"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { resolveDocsTopic } from "@/lib/docs-breadcrumbs";
import { REPO_DOCS_BASE } from "@/lib/docs-map";
import { getDocsShell, splitToken } from "@/lib/i18n/dictionaries";
import { DISCORD_URL, REPO_ISSUES_URL } from "@/lib/i18n/links";

/**
 * Contextual help band under every docs page. The source-document link is
 * resolved from the current route through the docs-map registry, so the
 * band always points at the document this page was drawn from; the rest
 * are the fixed escalation paths — troubleshooting, FAQ, Discord, and a
 * pre-labelled docs issue.
 */
export function DocsHelp({ locale }: { locale: string }) {
  const t = getDocsShell(locale);
  const pathname = usePathname() ?? `/${locale}/docs`;
  const topic = resolveDocsTopic(locale, pathname);
  const sources = topic
    ? Array.isArray(topic.repoSource)
      ? topic.repoSource
      : [topic.repoSource]
    : [];
  // "Source: {name}" — the links are typeset between the template's halves
  // so a locale may place the token anywhere.
  const sourceParts = splitToken(t.helpSource, "name");
  const issueUrl = `${REPO_ISSUES_URL}/new?labels=documentation&title=${encodeURIComponent(
    `docs: ${topic ? topic.label.en : "hub"} (${pathname})`,
  )}`;

  return (
    <aside className="docs-help" aria-labelledby="docs-help-title">
      <div className="docs-help-copy">
        <h2 id="docs-help-title">{t.helpTitle}</h2>
        <p>{t.helpLead}</p>
        {sources.length > 0 && (
          <p className="docs-help-source">
            {sourceParts[0]}
            {sources.map((source, i) => (
              <span key={source}>
                {i > 0 && ", "}
                <a href={`${REPO_DOCS_BASE}/${source}`} target="_blank" rel="noreferrer">
                  {source}
                </a>
              </span>
            ))}
            {sourceParts[1]}
          </p>
        )}
      </div>
      <nav className="docs-help-links" aria-label={t.helpTitle}>
        {topic?.id !== "troubleshooting" && (
          <Link href={`/${locale}/docs/troubleshooting`}>{t.helpTroubleshooting} →</Link>
        )}
        <Link href={`/${locale}/faq`}>{t.helpFaq} →</Link>
        <a href={DISCORD_URL} target="_blank" rel="noreferrer">
          {t.helpDiscord}
        </a>
        <a href="mailto:help@codewhale.net">help@codewhale.net</a>
        <a href={issueUrl} target="_blank" rel="noreferrer">
          {t.helpIssue}
        </a>
      </nav>
    </aside>
  );
}
