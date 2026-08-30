"use client";

import { useState, useMemo, useRef, useCallback, useEffect } from "react";
import Link from "next/link";
import {
  DOC_CATEGORY_LABELS,
  DOC_TOPICS,
  docTopicHref,
  docTopicIsExternal,
  type DocTopic,
} from "@/lib/docs-map";
import { DOC_TASKS, docTaskHaystack, type DocTask } from "@/lib/docs-tasks";
import { fill, getDocsShell, pickText } from "@/lib/i18n/dictionaries";
import { docTopicHaystack, highlightSpan } from "@/lib/search-utils";
import { EmptyState } from "./surface-state";

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

function topicSources(topic: DocTopic): string[] {
  return Array.isArray(topic.repoSource) ? topic.repoSource : [topic.repoSource];
}

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
/*  Rows                                                               */
/* ------------------------------------------------------------------ */

function TaskRow({ task, locale, query }: { task: DocTask; locale: string; query: string }) {
  return (
    <Link href={`/${locale}${task.href}`} className="docs-topic-row docs-task-row">
      <div className="docs-topic-main">
        <div className="docs-topic-title">{highlight(pickText(task.label, locale), query)}</div>
        <p>{highlight(pickText(task.description, locale), query)}</p>
      </div>
      <div className="docs-topic-source">{task.href}</div>
      <span className="docs-topic-arrow" aria-hidden="true">→</span>
    </Link>
  );
}

function TopicRow({
  topic,
  locale,
  query,
  webGuideTag,
  sourceDocTag,
}: {
  topic: DocTopic;
  locale: string;
  query: string;
  webGuideTag: string;
  sourceDocTag: string;
}) {
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
          {highlight(pickText(topic.label, locale), query)}
          <span>{isExternal ? sourceDocTag : webGuideTag}</span>
        </div>
        <p>{highlight(pickText(topic.description, locale), query)}</p>
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

/**
 * The docs hub: one search box over two registries — tasks
 * (`lib/docs-tasks.ts`, "I want to…") and topics (`lib/docs-map.ts`).
 * Searching matches English and Chinese text regardless of the active
 * locale. Every string is dictionary-driven; no locale branch here.
 */
export function DocsSearch({ locale }: { locale: string }) {
  const t = getDocsShell(locale);
  const [query, setQuery] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  const topicHaystacks = useMemo(() => DOC_TOPICS.map(docTopicHaystack), []);
  const taskHaystacks = useMemo(() => DOC_TASKS.map(docTaskHaystack), []);

  const q = query.trim().toLowerCase();
  const filteredTasks = useMemo(
    () => (q ? DOC_TASKS.filter((_, i) => taskHaystacks[i].includes(q)) : DOC_TASKS),
    [q, taskHaystacks],
  );
  const filteredTopics = useMemo(
    () => (q ? DOC_TOPICS.filter((_, i) => topicHaystacks[i].includes(q)) : DOC_TOPICS),
    [q, topicHaystacks],
  );

  // Group filtered topics by category (preserve DOC_TOPICS order).
  const grouped = useMemo(() => {
    const map = new Map<DocTopic["category"], DocTopic[]>();
    for (const topic of filteredTopics) {
      const group = map.get(topic.category) ?? [];
      group.push(topic);
      map.set(topic.category, group);
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

  const total = DOC_TOPICS.length + DOC_TASKS.length;
  const matched = filteredTopics.length + filteredTasks.length;
  const hasQuery = q.length > 0;

  return (
    <div className="docs-index">
      {/* Search bar */}
      <div className="docs-search-block">
        <label htmlFor="docs-search" className="docs-search-label">
          {t.searchLabel}
        </label>
        <div className="relative">
          <input
            id="docs-search"
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t.searchPlaceholder}
            className="search-input docs-search-input w-full"
            aria-label={t.searchLabel}
            autoComplete="off"
          />
          {hasQuery && (
            <button
              type="button"
              onClick={() => setQuery("")}
              className="docs-search-clear"
              aria-label={t.searchClear}
            >
              ✕
            </button>
          )}
        </div>
        {hasQuery && (
          <div className="docs-search-count" aria-live="polite">
            {matched > 0
              ? fill(t.searchMatches, { matched, total, query: query.trim() })
              : fill(t.searchNoMatches, { query: query.trim() })}
          </div>
        )}
      </div>

      {matched > 0 ? (
        <div className="docs-result-groups">
          {/* Tasks — "I am trying to…" */}
          {filteredTasks.length > 0 && (
            <section id="tasks" className="docs-result-group docs-task-group">
              <div className="docs-result-heading">
                <h2>{t.tasksHeading}</h2>
                <span>{filteredTasks.length}</span>
              </div>
              {!hasQuery && <p className="docs-result-lead">{t.tasksLead}</p>}
              <div className="docs-topic-list">
                {filteredTasks.map((task) => (
                  <TaskRow key={task.id} task={task} locale={locale} query={query} />
                ))}
              </div>
            </section>
          )}

          {/* Topics by category */}
          {grouped.size > 0 && (
            <div className="docs-result-topics">
              {!hasQuery && (
                <div className="docs-result-heading docs-result-heading-topics">
                  <h2>{t.topicsHeading}</h2>
                  <span>{filteredTopics.length}</span>
                </div>
              )}
              {[...grouped.entries()].map(([category, topics]) => (
                <section key={category} id={category} className="docs-result-group">
                  <div className="docs-result-heading">
                    <h3>{pickText(DOC_CATEGORY_LABELS[category], locale)}</h3>
                    <span>{topics.length}</span>
                  </div>
                  <div className="docs-topic-list">
                    {topics.map((topic) => (
                      <TopicRow
                        key={topic.id}
                        topic={topic}
                        locale={locale}
                        query={query}
                        webGuideTag={t.webGuideTag}
                        sourceDocTag={t.sourceDocTag}
                      />
                    ))}
                  </div>
                </section>
              ))}
            </div>
          )}
        </div>
      ) : (
        <EmptyState
          locale={locale}
          title={t.emptyTitle}
          body={t.emptyBody}
          action={
            <a
              href="https://github.com/Hmbown/CodeWhale/tree/main/docs"
              target="_blank"
              rel="noreferrer"
              className="portal-button portal-button-secondary"
            >
              {t.emptyCta}
            </a>
          }
        />
      )}

      {/* Registry note (only when not searching) */}
      {!hasQuery && (
        <section className="docs-source-note">
          <p>{t.indexNote}</p>
        </section>
      )}
    </div>
  );
}
