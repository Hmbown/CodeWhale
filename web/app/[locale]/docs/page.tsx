import Link from "next/link";
import {
  getTopicsByCategory,
  REPO_DOCS_BASE,
  type DocTopic,
} from "@/lib/docs-map";
import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/docs",
    locale,
    title: isZh ? "文档 · CodeWhale" : "Docs · CodeWhale",
    description: isZh
      ? "CodeWhale 文档：安装、使用指南、配置、提供商、核心概念、工具、MCP、技能、沙箱、运行时 API、排障。"
      : "CodeWhale documentation: install, user guide, configuration, providers, core concepts, tools, MCP, skills, sandbox, runtime API, troubleshooting.",
  });
}

/* ------------------------------------------------------------------ */
/*  Locale-aware strings                                              */
/* ------------------------------------------------------------------ */

const CATEGORY_LABELS: Record<string, { en: string; zh: string }> = {
  "getting-started": { en: "Getting started", zh: "入门" },
  workflows: { en: "Workflows", zh: "工作流" },
  "core-concepts": { en: "Core concepts", zh: "核心概念" },
  reference: { en: "Reference", zh: "参考" },
  extending: { en: "Extending", zh: "扩展" },
  operations: { en: "Operations & community", zh: "运维与社区" },
};

/* ------------------------------------------------------------------ */

function topicHref(topic: DocTopic, locale: string): string {
  if (topic.hasPage) {
    return `/${locale}/docs/${topic.slug}`;
  }
  const src = Array.isArray(topic.repoSource) ? topic.repoSource[0] : topic.repoSource;
  return `${REPO_DOCS_BASE}/${src}`;
}

function topicSources(topic: DocTopic): string[] {
  return Array.isArray(topic.repoSource) ? topic.repoSource : [topic.repoSource];
}

/* ------------------------------------------------------------------ */

export default async function DocsHubPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const byCategory = getTopicsByCategory();

  const body = (
    <div className="space-y-12">
      {[...byCategory.entries()].map(([cat, topics]) => (
        <section key={cat} id={cat}>
          <h2 className="font-display text-2xl mb-1">
            {isZh ? CATEGORY_LABELS[cat]?.zh ?? cat : CATEGORY_LABELS[cat]?.en ?? cat}
          </h2>
          <div className="grid sm:grid-cols-2 gap-4 mt-4">
            {topics.map((t) => {
              const href = topicHref(t, locale);
              const sources = topicSources(t);
              const isExternal = !t.hasPage;
              return (
                <Link
                  key={t.id}
                  id={t.id}
                  href={href}
                  target={isExternal ? "_blank" : undefined}
                  className="hairline-t hairline-b hairline-l hairline-r p-4 hover:bg-paper-deep transition-colors group block scroll-mt-32"
                >
                  <div className="flex items-center gap-2 mb-1.5">
                    <span className="font-mono text-[0.62rem] uppercase tracking-widest text-ink-mute">
                      {isZh ? t.label.zh : t.label.en}
                    </span>
                    {isExternal && (
                      <span className="font-mono text-[0.6rem] text-ink-mute">↗</span>
                    )}
                  </div>
                  <p className="text-sm text-ink-soft leading-relaxed">
                    {isZh ? t.description.zh : t.description.en}
                  </p>
                  <div className="mt-2 font-mono text-[0.62rem] text-ink-mute truncate">
                    {sources.map((s, i) => (
                      <span key={s}>
                        {i > 0 && ", "}
                        {s}
                      </span>
                    ))}
                  </div>
                </Link>
              );
            })}
          </div>
        </section>
      ))}

      {/* Parapgraph about the docs approach */}
      <section className="hairline-t pt-8">
        <p className="text-sm text-ink-mute leading-relaxed max-w-2xl">
          {isZh
            ? "§ 标记的条目在 CodeWhale 网站上有独立页面；↗ 标记的条目链接到 GitHub 仓库中的源文档。所有内容来源于 docs/ 目录下的 40+ 篇 Markdown 文档，通过 docs-map.ts 注册表维护。"
            : "Entries marked § have dedicated pages on codewhale.net; entries marked ↗ link to source documents in the GitHub repository. All content is sourced from 40+ Markdown documents in the docs/ directory, maintained through the docs-map.ts registry."}
        </p>
      </section>
    </div>
  );

  return body;
}
