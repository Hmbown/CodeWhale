import Link from "next/link";
import { Seal } from "@/components/seal";
import { getTopicsByCategory, REPO_DOCS_BASE, type DocTopic } from "@/lib/docs-map";

/* ------------------------------------------------------------------ */
/*  Locale-aware category heading labels                               */
/* ------------------------------------------------------------------ */

const CATEGORY_LABELS: Record<string, { en: string; zh: string }> = {
  "getting-started": { en: "Getting started", zh: "入门" },
  "core-concepts": { en: "Core concepts", zh: "核心概念" },
  reference: { en: "Reference", zh: "参考" },
  extending: { en: "Extending", zh: "扩展" },
  operations: { en: "Operations", zh: "运维" },
};

/* ------------------------------------------------------------------ */
/*  Link resolution helpers                                            */
/* ------------------------------------------------------------------ */

function topicHref(topic: DocTopic, locale: string): string {
  if (topic.hasPage) {
    return `/${locale}/docs/${topic.slug}`;
  }
  const src = Array.isArray(topic.repoSource) ? topic.repoSource[0] : topic.repoSource;
  return `${REPO_DOCS_BASE}/${src}`;
}

/* ------------------------------------------------------------------ */
/*  Sidebar                                                            */
/* ------------------------------------------------------------------ */

function DocsSidebar({ locale, currentId }: { locale: string; currentId?: string }) {
  const isZh = locale === "zh";
  const byCategory = getTopicsByCategory();

  return (
    <aside className="lg:col-span-3 min-w-0">
      <div className="lg:sticky lg:top-32">
        <div className="eyebrow mb-3">{isZh ? "文档目录" : "Docs index"}</div>
        <nav className="hairline-t hairline-b py-3 space-y-4">
          {[...byCategory.entries()].map(([cat, topics]) => (
            <div key={cat}>
              <div className="font-mono text-[0.62rem] uppercase tracking-widest text-ink-mute mb-1.5 px-0.5">
                {isZh ? CATEGORY_LABELS[cat]?.zh ?? cat : CATEGORY_LABELS[cat]?.en ?? cat}
              </div>
              <ul className="space-y-0.5">
                {topics.map((t) => {
                  const href = topicHref(t, locale);
                  const isCurrent = t.id === currentId;
                  return (
                    <li key={t.id}>
                      <Link
                        href={href}
                        target={t.hasPage ? undefined : "_blank"}
                        className={`block py-0.5 px-0.5 text-sm transition-colors ${
                          isCurrent ? "text-indigo font-semibold" : "text-ink-soft hover:text-indigo"
                        }`}
                      >
                        <span className="font-mono text-[0.7rem] text-ink-mute mr-1.5 tabular">
                          {t.hasPage ? "§" : "↗"}
                        </span>
                        {isZh ? t.label.zh : t.label.en}
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

/* ------------------------------------------------------------------ */
/*  Layout (Next.js App Router)                                        */
/* ------------------------------------------------------------------ */

export default async function DocsLayout({
  children,
  params,
}: {
  children: React.ReactNode;
  params: Promise<{ locale: string }>;
}) {
  const { locale } = await params;
  const isZh = locale === "zh";

  return (
    <div className="docs-theme min-h-screen">
    <section className="mx-auto max-w-[1400px] px-6 pt-12 pb-8">
      <div className="flex items-baseline gap-4 mb-3">
        <Seal char="文" />
        <div className="eyebrow">{isZh ? "Section 02 · 文档" : "Section 02 · Docs"}</div>
      </div>
      <h1 className="font-display tracking-crisp">
        {isZh ? (
          <>
            文档 <span className="font-cjk text-indigo text-5xl ml-2">Documentation</span>
          </>
        ) : (
          <>
            Documentation <span className="font-cjk text-indigo text-5xl ml-2">文档</span>
          </>
        )}
      </h1>
      <p className="mt-5 max-w-3xl text-ink-soft text-lg leading-[1.9] tracking-wide">
        {isZh
          ? "工作原理简述：先有 Agent 自我模型，再有嵌套权威系统，最后才是模式、工具和 provider。完整的架构讲解请参阅仓库中的 "
          : "How CodeWhale works: ego, conflict law, evidence, modes, tools, sandbox, MCP, config, hooks. The full architecture walkthrough is in "}
        <Link
          href="https://github.com/Hmbown/CodeWhale/blob/main/docs/ARCHITECTURE.md"
          className="body-link mx-1"
        >
          docs/ARCHITECTURE.md
        </Link>
        。
      </p>

      <div className="mt-10 grid lg:grid-cols-12 gap-10 min-w-0">
        <DocsSidebar locale={locale} />
        <article className="lg:col-span-9 min-w-0">{children}</article>
      </div>
    </section>
    </div>
  );
}
