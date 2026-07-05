"use client";

/**
 * DocsSearch — client-side search box for the docs sidebar.
 *
 * Fetches the build-time-generated /docs-search-index.json lazily on first
 * focus, ranks entries with the dependency-free scorer in lib/docs-search,
 * and renders section-anchored results. No external services, no server
 * runtime — everything runs in the browser against static JSON.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { useRouter } from "next/navigation";
import { searchDocs, type SearchEntry, type SearchResult } from "@/lib/docs-search";

export function DocsSearch({ locale }: { locale: string }) {
  const isZh = locale === "zh";
  const router = useRouter();

  const [index, setIndex] = useState<SearchEntry[] | null>(null);
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<SearchResult[]>([]);
  const [active, setActive] = useState(0);
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const loadStarted = useRef(false);

  const loadIndex = useCallback(() => {
    if (loadStarted.current) return;
    loadStarted.current = true;
    fetch("/docs-search-index.json")
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error(`${r.status}`))))
      .then((data: SearchEntry[]) => setIndex(data))
      .catch(() => {
        // Allow a retry on next focus if the fetch failed.
        loadStarted.current = false;
      });
  }, []);

  // Re-rank whenever the query or index changes.
  useEffect(() => {
    if (!index || query.trim().length < 2) {
      setResults([]);
      setActive(0);
      return;
    }
    setResults(searchDocs(index, query));
    setActive(0);
  }, [index, query]);

  // Close on click outside.
  useEffect(() => {
    const onPointerDown = (e: PointerEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, []);

  const hrefFor = (r: SearchResult) =>
    `/${locale}/docs/${r.entry.slug}${r.entry.anchor ? `#${r.entry.anchor}` : ""}`;

  const go = (r: SearchResult) => {
    setOpen(false);
    setQuery("");
    router.push(hrefFor(r));
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((a) => Math.min(a + 1, results.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((a) => Math.max(a - 1, 0));
    } else if (e.key === "Enter" && results[active]) {
      e.preventDefault();
      go(results[active]);
    } else if (e.key === "Escape") {
      setOpen(false);
    }
  };

  const showPanel = open && query.trim().length >= 2;

  return (
    <div ref={rootRef} className="relative mb-4">
      <label className="sr-only" htmlFor="docs-search-input">
        {isZh ? "搜索文档" : "Search docs"}
      </label>
      <input
        id="docs-search-input"
        type="search"
        role="combobox"
        aria-expanded={showPanel}
        aria-controls="docs-search-results"
        aria-autocomplete="list"
        autoComplete="off"
        placeholder={isZh ? "搜索文档…" : "Search docs…"}
        value={query}
        onChange={(e) => {
          setQuery(e.target.value);
          setOpen(true);
        }}
        onFocus={() => {
          loadIndex();
          setOpen(true);
        }}
        onKeyDown={onKeyDown}
        className="w-full hairline-t hairline-b hairline-l hairline-r bg-transparent px-2.5 py-1.5 text-sm text-ink-soft placeholder:text-ink-mute focus:outline-none focus:border-indigo"
      />
      {showPanel && (
        <div
          id="docs-search-results"
          role="listbox"
          className="absolute left-0 top-full z-40 mt-1 w-full min-w-[18rem] lg:w-[26rem] max-h-[60vh] overflow-y-auto hairline-t hairline-b hairline-l hairline-r shadow-lg"
          style={{ background: "var(--paper)" }}
        >
          {index === null ? (
            <div className="px-3 py-2 text-sm text-ink-mute">
              {isZh ? "加载索引中…" : "Loading index…"}
            </div>
          ) : results.length === 0 ? (
            <div className="px-3 py-2 text-sm text-ink-mute">
              {isZh ? "没有匹配结果。" : "No matches."}
            </div>
          ) : (
            <ul>
              {results.map((r, i) => (
                <li key={`${r.entry.slug}#${r.entry.anchor}`}>
                  <a
                    href={hrefFor(r)}
                    role="option"
                    aria-selected={i === active}
                    onClick={(e) => {
                      e.preventDefault();
                      go(r);
                    }}
                    onMouseEnter={() => setActive(i)}
                    className={`block px-3 py-2 transition-colors ${
                      i === active ? "bg-[var(--indigo-pale)]" : ""
                    }`}
                  >
                    <div className="text-sm text-ink-soft">
                      <span className="font-semibold">
                        {isZh ? r.entry.page.zh : r.entry.page.en}
                      </span>
                      {r.entry.heading && (
                        <span className="text-ink-mute"> › {r.entry.heading}</span>
                      )}
                    </div>
                    {r.excerpt && (
                      <div className="mt-0.5 text-xs text-ink-mute leading-relaxed line-clamp-2">
                        {r.excerpt}
                      </div>
                    )}
                  </a>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}
