"use client";

/**
 * <ThemeToggle> — a compact Auto / Light / Dark control for the date strip.
 *
 * The site ships the Tideline dark field everywhere; the toggle only renders
 * on docs routes, where it switches the docs sheet between the dark default
 * and the opt-in Blue Stage light sheet — the same preset pair the TUI
 * offers. Showing it off the docs routes would be a control that appears to
 * do nothing.
 *
 * "auto" removes the attribute and follows the site default (dark); "light"
 * and "dark" force the choice via `data-theme` on <html>. The choice persists
 * to localStorage and is re-applied before paint by the inline script in the
 * locale layout, so there is no theme flash on reload.
 */

import { useEffect, useState } from "react";
import { usePathname } from "next/navigation";
import { fill } from "@/lib/i18n/dictionaries";
import { isDocsPath } from "@/lib/i18n/path";

type Mode = "auto" | "light" | "dark";
const ORDER: Mode[] = ["auto", "light", "dark"];
const KEY = "cw-theme";

function apply(mode: Mode) {
  const el = document.documentElement;
  if (mode === "auto") el.removeAttribute("data-theme");
  else el.setAttribute("data-theme", mode);
}

export function ThemeToggle({
  autoLabel,
  lightLabel,
  darkLabel,
  ariaTemplate,
  titleLabel,
}: {
  autoLabel: string;
  lightLabel: string;
  darkLabel: string;
  /** "Docs theme: {mode} (click to cycle)" — interpolated with fill(). */
  ariaTemplate: string;
  titleLabel: string;
}) {
  const pathname = usePathname();
  const [mode, setMode] = useState<Mode>("auto");
  const [mounted, setMounted] = useState(false);

  useEffect(() => {
    setMounted(true);
    const stored = (typeof localStorage !== "undefined" && localStorage.getItem(KEY)) as Mode | null;
    if (stored && ORDER.includes(stored)) setMode(stored);
  }, []);

  if (!isDocsPath(pathname)) return null;

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

  const labels: Record<Mode, string> = {
    auto: autoLabel,
    light: lightLabel,
    dark: darkLabel,
  };
  const glyph: Record<Mode, string> = { auto: "◐", light: "☀", dark: "☾" };

  return (
    <button
      type="button"
      onClick={cycle}
      className="inline-flex items-center gap-1.5 px-1.5 py-0.5 hairline-l hairline-r hairline-t hairline-b hover:text-indigo transition-colors"
      aria-label={fill(ariaTemplate, { mode: labels[mode] })}
      title={titleLabel}
      suppressHydrationWarning
    >
      <span aria-hidden>{mounted ? glyph[mode] : glyph.auto}</span>
      <span className="hidden 2xl:inline" suppressHydrationWarning>
        {mounted ? labels[mode] : labels.auto}
      </span>
    </button>
  );
}
