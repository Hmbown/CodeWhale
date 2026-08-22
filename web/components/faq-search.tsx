"use client";

import { useState, useMemo, useRef, useCallback, useEffect } from "react";
import { faqSourceHref } from "@/lib/faq-source";
import { extractText } from "@/lib/react-text";
import { highlightSpan } from "@/lib/search-utils";

export interface FaqSearchItem {
  q: string;
  a: React.ReactNode;
  sources?: string[];
}

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
/*  Component                                                           */
/* ------------------------------------------------------------------ */

export function FaqSearch({
  items,
  locale,
}: {
  items: FaqSearchItem[];
  locale: string;
}) {
  const isZh = locale === "zh";
  const [query, setQuery] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  // Precompute haystack for each item (question + answer text + sources).
  const haystacks = useMemo(
    () =>
      items.map((item) => {
        const parts = [
          item.q,
          extractText(item.a),
          ...(item.sources ?? []),
        ];
        return parts.join(" ").toLowerCase();
      }),
    [items],
  );

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return items.map((item, i) => ({ item, i }));
    return items
      .map((item, i) => ({ item, i }))
      .filter(({ i }) => haystacks[i].includes(q));
  }, [query, haystacks, items]);

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

  const total = items.length;
  const matched = filtered.length;
  const hasQuery = query.trim().length > 0;

  return (
    <>
      {/* Search bar */}
      <div className="mb-6">
        <div className="relative">
          <input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={
              isZh
                ? "搜索常见问题…（按 / 快速聚焦）"
                : "Search FAQ… (press / to focus)"
            }
            className="search-input w-full"
            aria-label={isZh ? "搜索常见问题" : "Search FAQ"}
          />
          {hasQuery && (
            <button
              onClick={() => setQuery("")}
              className="absolute right-3 top-1/2 -translate-y-1/2 font-mono text-sm text-ink-mute hover:text-indigo transition-colors"
              aria-label={isZh ? "清除" : "Clear"}
            >
              ✕
            </button>
          )}
        </div>
        {hasQuery && (
          <div className="mt-2 font-mono text-[0.7rem] text-ink-mute" aria-live="polite">
            {matched > 0
              ? isZh
                ? `${matched} / ${total} 个问题匹配 "${query.trim()}"`
                : `${matched} of ${total} questions match "${query.trim()}"`
              : isZh
                ? `未找到匹配 "${query.trim()}" 的问题`
                : `No questions match "${query.trim()}"`}
          </div>
        )}
      </div>

      {/* FAQ list */}
      {matched > 0 ? (
        <div className="space-y-0 hairline-t hairline-b">
          {filtered.map(({ item, i }) => (
            <details key={i} className="group hairline-b last:border-b-0">
              <summary className="px-0 py-5 cursor-pointer flex items-start gap-4 hover:text-indigo transition-colors">
                <span className="font-mono text-indigo tabular text-sm pt-0.5 shrink-0">
                  {String(i + 1).padStart(2, "0")}
                </span>
                <span className="font-display text-lg leading-snug flex-1">
                  {highlight(item.q, query)}
                </span>
                <span className="font-mono text-ink-mute text-sm group-open:rotate-45 transition-transform shrink-0">+</span>
              </summary>
              <div className="pb-5 pl-10 pr-4">
                <div className={`text-ink-soft leading-relaxed ${isZh ? "leading-[1.9] tracking-wide" : ""}`}>
                  {item.a}
                </div>
                {item.sources && item.sources.length > 0 && (
                  <div className="mt-3 flex items-center gap-2 flex-wrap">
                    <span className="font-mono text-[0.66rem] text-ink-mute uppercase tracking-wider">
                      {isZh ? "来源" : "Sources"}:
                    </span>
                    {item.sources.map((s) => {
                      const href = faqSourceHref(s);
                      return href ? (
                        <a
                          key={s}
                          href={href}
                          target="_blank"
                          rel="noreferrer"
                          className="font-mono text-[0.7rem] text-indigo hover:underline"
                        >
                          {s}
                        </a>
                      ) : (
                        <span key={s} className="font-mono text-[0.7rem] text-indigo">{s}</span>
                      );
                    })}
                  </div>
                )}
              </div>
            </details>
          ))}
        </div>
      ) : (
        <div className="text-center py-16 hairline-t hairline-b">
          <p className="font-display text-lg text-ink-mute mb-2">
            {isZh ? "未找到结果" : "No results found"}
          </p>
          <p className="text-sm text-ink-mute">
            {isZh
              ? "尝试使用不同的关键字。"
              : "Try a different keyword."}
          </p>
        </div>
      )}
    </>
  );
}
