"use client";

import { useState, useMemo, useRef, useCallback, useEffect } from "react";
import Link from "next/link";
import {
  DOC_TOPICS,
  docTopicHref,
  docTopicIsExternal,
  type DocTopic,
} from "@/lib/docs-map";
import { docTopicHaystack, highlightSpan } from "@/lib/search-utils";

/* ------------------------------------------------------------------ */
/*  Locale-aware strings                                              */
/* ------------------------------------------------------------------ */

const CATEGORY_LABELS: Record<string, { en: string; zh: string }> = {
  "getting-started": { en: "Getting started", zh: "入门" },
  "core-concepts": { en: "Core concepts", zh: "核心概念" },
  reference: { en: "Reference", zh: "参考" },
  extending: { en: "Extending", zh: "扩展" },
  operations: { en: "Operations & community", zh: "运维与社区" },
};

/* ------------------------------------------------------------------ */
/*  Link / source helpers (mirrored from the original page.tsx)       */
/* ------------------------------------------------------------------ */

function topicSources(topic: DocTopic): string[] {
  return Array.isArray(topic.repoSource) ? topic.repoSource : [topic.repoSource];
}

/**
 * Build a single lowercase haystack string for fuzzy matching.
 * Delegates to the shared search-utils for testability.
 * Includes both EN and ZH text so a user can search in either language
 * regardless of the active locale.
 */
const topicHaystack = docTopicHaystack;

/* ------------------------------------------------------------------ */
/*  Highlight helper                                                   */
/* ------------------------------------------------------------------ */

function highlight(text: string, query: string): React.ReactNode {
  // Index arithmetic lives in search-utils: lowercasing can change a
  // string's length, so `text` cannot be sliced with indices taken from
  // its lowercased copy.
  const span = highlightSpan(text, query);
  if (!span) return text;
  return (
    <>
      {span.before}
      <mark className="search-highlight">{span.match}</mark>
      {span.after}
    </>
  );
}

/* ------------------------------------------------------------------ */
/*  Topic row                                                          */
/* ------------------------------------------------------------------ */

function TopicRow({
  topic,
  locale,
  query,
}: {
  topic: DocTopic;
  locale: string;
  query: string;
}) {
  const isZh = locale === "zh";
  const href = docTopicHref(topic, locale);
  const sources = topicSources(topic);
  const isExternal = docTopicIsExternal(topic);

  return (
    <Link
      href={href}
      target={isExternal ? "_blank" : undefined}
      rel={isExternal ? "noreferrer" : undefined}
      className="docs-topic-row"
    >
      <div className="docs-topic-main">
        <div className="docs-topic-title">
          {highlight(isZh ? topic.label.zh : topic.label.en, query)}
          <span>{isExternal ? (isZh ? "源文档" : "Source doc") : (isZh ? "网页" : "Web guide")}</span>
        </div>
        <p>
          {highlight(isZh ? topic.description.zh : topic.description.en, query)}
        </p>
      </div>
      <div className="docs-topic-source">
        {sources.map((s, i) => (
          <span key={s}>
            {i > 0 && ", "}
            {highlight(s, query)}
          </span>
        ))}
      </div>
      <span className="docs-topic-arrow" aria-hidden="true">{isExternal ? "↗" : "→"}</span>
    </Link>
  );
}

/* ------------------------------------------------------------------ */
/*  Main component                                                     */
/* ------------------------------------------------------------------ */

export function DocsSearch({ locale }: { locale: string }) {
  const isZh = locale === "zh";
  const [query, setQuery] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  // Precompute haystacks once.
  const haystacks = useMemo(() => DOC_TOPICS.map(topicHaystack), []);

  // Filter topics by query.
  const filteredTopics = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return DOC_TOPICS;
    return DOC_TOPICS.filter((_, i) => haystacks[i].includes(q));
  }, [query, haystacks]);

  // Group filtered topics by category (preserve DOC_TOPICS order).
  const grouped = useMemo(() => {
    const map = new Map<string, DocTopic[]>();
    for (const t of filteredTopics) {
      const group = map.get(t.category) ?? [];
      group.push(t);
      map.set(t.category, group);
    }
    return map;
  }, [filteredTopics]);

  // Keyboard shortcut: focus search on "/".
  const handleKeyDown = useCallback((e: KeyboardEvent) => {
    if (e.key === "/" && document.activeElement?.tagName !== "INPUT") {
      e.preventDefault();
      inputRef.current?.focus();
    }
  }, []);

  useEffect(() => {
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [handleKeyDown]);

  const total = DOC_TOPICS.length;
  const matched = filteredTopics.length;
  const hasQuery = query.trim().length > 0;

  return (
    <div className="docs-index">
      {/* Search bar */}
      <div className="docs-search-block">
        <label htmlFor="docs-search" className="docs-search-label">
          {isZh ? "搜索文档" : "Search documentation"}
        </label>
        <div className="relative">
          <input
            id="docs-search"
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={
              isZh
                ? "搜索文档…（按 / 快速聚焦）"
                : "Search docs… (press / to focus)"
            }
            className="search-input docs-search-input w-full"
            aria-label={isZh ? "搜索文档" : "Search documentation"}
          />
          {hasQuery && (
            <button
              type="button"
              onClick={() => setQuery("")}
              className="docs-search-clear"
              aria-label={isZh ? "清除" : "Clear"}
            >
              ✕
            </button>
          )}
        </div>
        {hasQuery && (
          <div className="docs-search-count" aria-live="polite">
            {matched > 0
              ? isZh
                ? `${matched} / ${total} 篇文档匹配 "${query.trim()}"`
                : `${matched} of ${total} docs match "${query.trim()}"`
              : isZh
                ? `未找到匹配 "${query.trim()}" 的文档`
                : `No docs match "${query.trim()}"`}
          </div>
        )}
      </div>

      {/* Results */}
      {matched > 0 ? (
        <div className="docs-result-groups">
          {[...grouped.entries()].map(([cat, topics]) => (
            <section key={cat} id={cat} className="docs-result-group">
              <div className="docs-result-heading">
                <h2>{isZh ? CATEGORY_LABELS[cat]?.zh ?? cat : CATEGORY_LABELS[cat]?.en ?? cat}</h2>
                <span>{topics.length}</span>
              </div>
              <div className="docs-topic-list">
                {topics.map((t) => (
                  <TopicRow key={t.id} topic={t} locale={locale} query={query} />
                ))}
              </div>
            </section>
          ))}
        </div>
      ) : (
        <div className="docs-empty">
          <p>
            {isZh ? "未找到结果" : "No results found"}
          </p>
          <p>
            {isZh
              ? "尝试使用不同的关键字，或浏览 GitHub 上的完整文档。"
              : "Try a different keyword, or browse the full docs on GitHub."}
          </p>
          <Link
            href="https://github.com/Hmbown/CodeWhale/tree/main/docs"
            target="_blank"
            rel="noreferrer"
            className="portal-button portal-button-secondary"
          >
            {isZh ? "GitHub 文档目录 ↗" : "GitHub docs directory ↗"}
          </Link>
        </div>
      )}

      {/* Source note (only when not searching) */}
      {!hasQuery && (
        <section className="docs-source-note">
          <p>
            {isZh
              ? "“网页”条目提供站内指南；“源文档”条目直接打开 GitHub 仓库中的完整参考资料。文档索引由仓库中的 docs-map.ts 注册表维护。"
              : "Web guides stay on codewhale.net. Source docs open the complete reference in the GitHub repository. The index is maintained from the docs-map.ts registry in the repository."}
          </p>
        </section>
      )}
    </div>
  );
}
