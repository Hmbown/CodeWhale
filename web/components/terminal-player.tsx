"use client";

/**
 * <TerminalPlayer> — a terminal chrome around the real reasoning traces in
 * thinking-trace.tsx. Lines type in progressively with a blinking caret;
 * scene tabs switch between the excerpts.
 *
 * Pure React + CSS, no media assets. SSG-safe: the server render (and any
 * no-JS render) shows the complete static trace; the typing animation only
 * starts inside a client effect. Users with prefers-reduced-motion get the
 * full static text with no animation.
 */

import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { SCENES } from "./thinking-trace";

const TICK_MS = 24;
const CHARS_PER_TICK = 2;

function Caret() {
  return <span className="tp-caret" aria-hidden="true" />;
}

export function TerminalPlayer({
  locale = "en",
  traceLabel,
  tabsAria,
}: {
  locale?: string;
  /** Chrome copy from the locale dictionary (getChrome(locale)). */
  traceLabel: string;
  tabsAria: string;
}) {
  // Scene bodies are faithful excerpts of a real session and live in
  // components/thinking-trace.tsx as {en, zh} content pairs — the same
  // shared-content pattern as web/lib/content/. Locales beyond zh fall back
  // to the English excerpt until that content module gains more pairs
  // (FINISH-0.9.4 §0A Phase 2); the surrounding chrome is dictionary-driven.
  const isZh = locale === "zh";
  const [active, setActive] = useState(0);
  const scene = SCENES[active];
  const tabRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const panelRef = useRef<HTMLDivElement>(null);
  const firstScene = useRef(true);

  // ARIA tabs pattern: the tablist owns arrow-key/Home/End movement with
  // automatic activation — focus and selection stay on the same tab, so the
  // roving tabindex below never strands keyboard users on a deselected tab.
  const onTablistKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const last = SCENES.length - 1;
    let next: number | null = null;
    if (e.key === "ArrowRight") next = active === last ? 0 : active + 1;
    else if (e.key === "ArrowLeft") next = active === 0 ? last : active - 1;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = last;
    if (next === null) return;
    e.preventDefault();
    setActive(next);
    tabRefs.current[next]?.focus();
  };

  const text = useMemo(
    () => ({
      context: `# ${isZh ? scene.context.zh : scene.context.en}`,
      trace: scene.trace,
      decision: isZh ? scene.decision.zh : scene.decision.en,
    }),
    [scene, isZh]
  );

  // Character offsets across the four "lines" of a scene. Cites reveal as
  // whole pills once the animation reaches their offset.
  const contextStart = 0;
  const traceStart = text.context.length;
  const citesStart = traceStart + text.trace.length;
  const citeStarts: number[] = [];
  let acc = citesStart;
  for (const c of scene.cites) {
    citeStarts.push(acc);
    acc += c.length;
  }
  const decisionStart = acc;
  const total = decisionStart + text.decision.length;

  // Server render shows the full trace; the effect rewinds and types it in
  // when motion is allowed. No Date.now in render paths.
  const [shown, setShown] = useState(Number.MAX_SAFE_INTEGER);

  useEffect(() => {
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      setShown(Number.MAX_SAFE_INTEGER);
      return;
    }
    setShown(0);
    const id = window.setInterval(() => {
      setShown((n) => {
        if (n + CHARS_PER_TICK >= total) {
          window.clearInterval(id);
          return total;
        }
        return n + CHARS_PER_TICK;
      });
    }, TICK_MS);
    return () => window.clearInterval(id);
  }, [active, total, isZh]);

  // Tab switches get a beat of physicality: the new scene settles in as the
  // typing restarts. A WAAPI one-shot, transform/opacity only — skipped on
  // first mount (the section's own reveal covers arrival) and for reduced
  // motion (the trace simply appears complete, as ever).
  useEffect(() => {
    if (firstScene.current) {
      firstScene.current = false;
      return;
    }
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    panelRef.current?.animate(
      [
        { opacity: 0, transform: "translateY(5px)" },
        { opacity: 1, transform: "translateY(0)" },
      ],
      { duration: 190, easing: "cubic-bezier(0.25, 0.46, 0.45, 0.94)" }
    );
  }, [active]);

  const slice = (t: string, start: number) => t.slice(0, Math.max(0, shown - start));
  const typing = (start: number, len: number) => shown > start && shown < start + len;
  const done = shown >= total;

  return (
    <div className="terminal-frame hairline-t hairline-b hairline-l hairline-r bg-paper overflow-hidden">
      {/* title bar */}
      <div className="px-4 py-2.5 flex items-center justify-between border-b border-white/10">
        <div className="flex items-center gap-1.5">
          <span className="w-2.5 h-2.5 rounded-full bg-jade inline-block" />
          <span className="w-2.5 h-2.5 rounded-full bg-ochre inline-block" />
          <span className="w-2.5 h-2.5 rounded-full bg-indigo inline-block" />
          <span className="ml-2.5 font-mono text-[0.66rem] uppercase tracking-widest text-ink-mute">
            codewhale — thinking
          </span>
        </div>
        <span className="font-cjk text-[0.6rem] text-ink-mute">{traceLabel}</span>
      </div>

      {/* scene tabs */}
      <div
        className="flex border-b border-white/10 overflow-x-auto"
        role="tablist"
        aria-label={tabsAria}
        onKeyDown={onTablistKeyDown}
      >
        {SCENES.map((s, i) => (
          <button
            key={i}
            ref={(el) => {
              tabRefs.current[i] = el;
            }}
            type="button"
            role="tab"
            id={`tp-tab-${i}`}
            aria-selected={i === active}
            aria-controls="tp-panel"
            tabIndex={i === active ? 0 : -1}
            onClick={() => setActive(i)}
            className={`shrink-0 px-3 py-2 font-mono text-[0.62rem] uppercase tracking-widest transition-colors ${
              i === active
                ? "text-ink bg-white/10 border-b border-indigo"
                : "text-ink-mute hover:text-ink"
            }`}
          >
            {String(i + 1).padStart(2, "0")} · {isZh ? s.tab.zh : s.tab.en}
          </button>
        ))}
      </div>

      {/* body */}
      <div
        role="tabpanel"
        id="tp-panel"
        ref={panelRef}
        aria-labelledby={`tp-tab-${active}`}
        tabIndex={0}
        className="px-4 py-4 min-h-[15rem] font-mono text-[0.8rem] leading-relaxed"
      >
        {/* context — white/55 on ink clears WCAG AA (5.75:1) at this size */}
        <div className="text-white/55">
          {slice(text.context, contextStart)}
          {typing(contextStart, text.context.length) && <Caret />}
        </div>

        {/* the trace */}
        {shown > traceStart && (
          <div className="mt-3 whitespace-pre-wrap">
            <span className="text-indigo">›</span>{" "}
            <span className="text-white/85">{slice(text.trace, traceStart)}</span>
            {typing(traceStart, text.trace.length) && <Caret />}
          </div>
        )}

        {/* cited authority */}
        {shown > citesStart && (
          <div className="mt-3 flex flex-wrap gap-1.5">
            {scene.cites.map(
              (c, i) =>
                shown > citeStarts[i] && (
                  <span
                    key={c}
                    className="px-1.5 py-0.5 border border-white/25 text-white/75 text-[0.6rem] uppercase tracking-wider"
                  >
                    {c}
                  </span>
                )
            )}
          </div>
        )}

        {/* the decision it produced */}
        {shown > decisionStart && (
          <div className="mt-3">
            <span className="text-indigo font-semibold">→</span>{" "}
            <span className="text-white/90">{slice(text.decision, decisionStart)}</span>
            {typing(decisionStart, text.decision.length) && <Caret />}
          </div>
        )}

        {/* resting prompt */}
        {done && (
          <div className="mt-3 text-indigo">
            › <Caret />
          </div>
        )}
      </div>
    </div>
  );
}
