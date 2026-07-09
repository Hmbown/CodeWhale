"use client";

/**
 * <ThemeToggle> — a compact Auto / Light / Dark control for the date strip.
 *
 * Dark mode is scoped to the /docs routes (see globals.css `.docs-theme`),
 * so the toggle only renders while on a docs route — showing it site-wide
 * would be a control that appears to do nothing on the marketing pages.
 *
 * "auto" removes the attribute and follows prefers-color-scheme; "light" and
 * "dark" force the choice via `data-theme` on <html>. The choice persists to
 * localStorage and is re-applied before paint by the inline script in the
 * locale layout, so there is no theme flash on reload.
 */

import { useEffect, useState } from "react";
import { usePathname } from "next/navigation";

type Mode = "auto" | "light" | "dark";
const ORDER: Mode[] = ["auto", "light", "dark"];
const KEY = "cw-theme";

function apply(mode: Mode) {
  const el = document.documentElement;
  if (mode === "auto") el.removeAttribute("data-theme");
  else el.setAttribute("data-theme", mode);
}

export function ThemeToggle({ isZh = false }: { isZh?: boolean }) {
  const pathname = usePathname();
  const [mode, setMode] = useState<Mode>("auto");
  const [mounted, setMounted] = useState(false);

  useEffect(() => {
    setMounted(true);
    const stored = (typeof localStorage !== "undefined" && localStorage.getItem(KEY)) as Mode | null;
    if (stored && ORDER.includes(stored)) setMode(stored);
  }, []);

  const onDocs = /^\/[a-z]{2}\/docs(\/|$)/.test(pathname) || pathname.includes("/docs");
  if (!onDocs) return null;

  const cycle = () => {
    const next = ORDER[(ORDER.indexOf(mode) + 1) % ORDER.length];
    setMode(next);
    try {
      localStorage.setItem(KEY, next);
    } catch {
      /* private mode / storage disabled — the choice just won't persist */
    }
    apply(next);
  };

  const labels: Record<Mode, string> = isZh
    ? { auto: "自动", light: "浅色", dark: "深色" }
    : { auto: "auto", light: "light", dark: "dark" };
  const glyph: Record<Mode, string> = { auto: "◐", light: "☀", dark: "☾" };

  return (
    <button
      type="button"
      onClick={cycle}
      className="inline-flex items-center gap-1.5 px-1.5 py-0.5 hairline-l hairline-r hairline-t hairline-b hover:text-indigo transition-colors"
      aria-label={isZh ? `文档主题：${labels[mode]}（点击切换）` : `Docs theme: ${labels[mode]} (click to cycle)`}
      title={isZh ? "文档主题 · 自动 / 浅色 / 深色" : "Docs theme · auto / light / dark"}
      suppressHydrationWarning
    >
      <span aria-hidden>{mounted ? glyph[mode] : glyph.auto}</span>
      <span className="hidden sm:inline" suppressHydrationWarning>
        {mounted ? labels[mode] : labels.auto}
      </span>
    </button>
  );
}
