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

import { useEffect, useMemo, useState } from "react";
import { SCENES } from "./thinking-trace";

const TICK_MS = 24;
const CHARS_PER_TICK = 2;

function Caret() {
  return <span className="tp-caret" aria-hidden="true" />;
}

export function TerminalPlayer({ locale = "en" }: { locale?: string }) {
  const isZh = locale === "zh";
  const [active, setActive] = useState(0);
  const scene = SCENES[active];

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

  const slice = (t: string, start: number) => t.slice(0, Math.max(0, shown - start));
  const typing = (start: number, len: number) => shown > start && shown < start + len;
  const done = shown >= total;

  return (
    <div className="hairline-t hairline-b hairline-l hairline-r bg-ink overflow-hidden">
      {/* title bar */}
      <div className="px-4 py-2.5 flex items-center justify-between border-b border-white/10">
        <div className="flex items-center gap-1.5">
          <span className="w-2.5 h-2.5 rounded-full bg-jade inline-block" />
          <span className="w-2.5 h-2.5 rounded-full bg-ochre inline-block" />
          <span className="w-2.5 h-2.5 rounded-full bg-indigo inline-block" />
          <span className="ml-2.5 font-mono text-[0.66rem] uppercase tracking-widest text-paper-deep">
            codewhale — thinking
          </span>
        </div>
        <span className="font-cjk text-[0.6rem] text-paper-deep/70">
          {isZh ? "推理痕迹" : "reasoning trace"}
        </span>
      </div>

      {/* scene tabs */}
      <div
        className="flex border-b border-white/10 overflow-x-auto"
        role="tablist"
        aria-label={isZh ? "会话片段" : "Session excerpts"}
      >
        {SCENES.map((s, i) => (
          <button
            key={i}
            type="button"
            role="tab"
            aria-selected={i === active}
            onClick={() => setActive(i)}
            className={`shrink-0 px-3 py-2 font-mono text-[0.62rem] uppercase tracking-widest transition-colors ${
              i === active
                ? "text-paper bg-white/10 border-b border-indigo"
                : "text-paper-deep/60 hover:text-paper"
            }`}
          >
            {String(i + 1).padStart(2, "0")} · {isZh ? s.tab.zh : s.tab.en}
          </button>
        ))}
      </div>

      {/* body */}
      <div className="px-4 py-4 min-h-[15rem] font-mono text-[0.8rem] leading-relaxed">
        {/* context */}
        <div className="text-white/45">
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
