"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { docsTopicIsCurrent } from "@/lib/docs-navigation";
import {
  DOC_CATEGORY_LABELS,
  docTopicHref,
  docTopicIsExternal,
  getTopicsByCategory,
} from "@/lib/docs-map";
import { getDocsShell, pickText } from "@/lib/i18n/dictionaries";

/**
 * The one docs navigation: every registered topic, grouped by category,
 * with the current route marked. Chrome strings come from the docs-shell
 * dictionary; topic names are docs-map pairs resolved through `pickText`.
 */
export function DocsSidebar({ locale }: { locale: string }) {
  const t = getDocsShell(locale);
  const pathname = usePathname();
  const byCategory = getTopicsByCategory();

  return (
    <aside className="docs-sidebar min-w-0">
      <div className="lg:sticky lg:top-24">
        <div className="docs-sidebar-heading">
          <Link href={`/${locale}/docs`}>
            <span>{t.sidebarHeading}</span>
            <span aria-hidden="true">→</span>
          </Link>
        </div>
        <nav aria-label={t.sidebarAria}>
          {[...byCategory.entries()].map(([category, topics]) => (
            <div key={category} className="docs-sidebar-group">
              <div className="docs-sidebar-category">
                {pickText(DOC_CATEGORY_LABELS[category], locale)}
              </div>
              <ul>
                {topics.map((topic) => {
                  const isCurrent = docsTopicIsCurrent(topic, locale, pathname);
                  const isExternal = docTopicIsExternal(topic);
                  return (
                    <li key={topic.id}>
                      <Link
                        href={docTopicHref(topic, locale)}
                        target={isExternal ? "_blank" : undefined}
                        rel={isExternal ? "noreferrer" : undefined}
                        aria-current={isCurrent ? "page" : undefined}
                        className={
                          isCurrent
                            ? "docs-sidebar-link docs-sidebar-link-current"
                            : "docs-sidebar-link"
                        }
                      >
                        <span>{pickText(topic.label, locale)}</span>
                        {isExternal && <span aria-hidden="true">↗</span>}
                      </Link>
                    </li>
                  );
                })}
              </ul>
            </div>
          ))}
        </nav>
      </div>
    </aside>
  );
}
