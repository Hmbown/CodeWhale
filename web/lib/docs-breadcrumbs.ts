import {
  DOC_CATEGORY_LABELS,
  DOC_TOPICS,
  docTopicHref,
  type DocTopic,
} from "./docs-map";
import { docsTopicIsCurrent } from "./docs-navigation";
import { getDocsShell, pickText } from "./i18n/dictionaries";
import { SITE_URL } from "./page-meta";

export type DocsCrumb = {
  name: string;
  /** Omitted for the current page and for category groupings that have no URL. */
  href?: string;
};

export function resolveDocsTopic(locale: string, pathname: string): DocTopic | undefined {
  return DOC_TOPICS.find((topic) => docsTopicIsCurrent(topic, locale, pathname));
}

/**
 * Visible trail: Home → Docs → category → topic. The hub stops at Docs.
 * Chrome names come from the docs-shell dictionary; topic and category
 * names are the docs-map pairs resolved through `pickText`, so no locale
 * branch lives here.
 */
export function resolveDocsBreadcrumbs(locale: string, pathname: string): DocsCrumb[] {
  const t = getDocsShell(locale);
  const home: DocsCrumb = { name: t.breadcrumbHome, href: `/${locale}` };
  const docsHref = `/${locale}/docs`;
  const topic = resolveDocsTopic(locale, pathname);
  const normalized = pathname.split(/[?#]/)[0].replace(/\/+$/, "");

  if (!topic || normalized === docsHref) {
    return [home, { name: t.breadcrumbDocs }];
  }

  return [
    home,
    { name: t.breadcrumbDocs, href: docsHref },
    { name: pickText(DOC_CATEGORY_LABELS[topic.category], locale) },
    { name: pickText(topic.label, locale) },
  ];
}

/**
 * BreadcrumbList for the current docs URL.
 * Category groupings have no unique URL, so the machine trail is Home → Docs → topic.
 */
export function buildBreadcrumbListJsonLd(locale: string, pathname: string) {
  const t = getDocsShell(locale);
  const topic = resolveDocsTopic(locale, pathname);
  const elements: { name: string; item: string }[] = [
    { name: t.breadcrumbHome, item: `${SITE_URL}/${locale}` },
    { name: t.breadcrumbDocs, item: `${SITE_URL}/${locale}/docs` },
  ];
  if (topic) {
    elements.push({
      name: pickText(topic.label, locale),
      item: `${SITE_URL}${docTopicHref(topic, locale)}`,
    });
  }

  return {
    "@context": "https://schema.org",
    "@type": "BreadcrumbList",
    itemListElement: elements.map((element, index) => ({
      "@type": "ListItem",
      position: index + 1,
      name: element.name,
      item: element.item,
    })),
  };
}
