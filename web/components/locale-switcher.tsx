"use client";

import { useRouter, usePathname } from "next/navigation";
import { ALL_LOCALES, locales } from "@/lib/i18n/config";
import { fill, getChrome } from "@/lib/i18n/dictionaries";

/** Labels for the dropdown. Keyed by locale code, displayed in native script. */
const LOCALE_LABELS: Record<string, string> = {};
for (const l of ALL_LOCALES) {
  LOCALE_LABELS[l.code] = l.label;
}

/** Routed locales that appear in the switcher (shipped + partial). */
const ROUTED = ALL_LOCALES.filter((l) => l.status === "shipped" || l.status === "partial");

export function LocaleSwitcher({ current }: { current: string }) {
  const router = useRouter();
  const pathname = usePathname();
  const chrome = getChrome(current);

  const switchLocale = (code: string) => {
    if (code === current) return;
    const segments = pathname.split("/");
    if ((locales as readonly string[]).includes(segments[1])) {
      segments[1] = code;
    } else {
      segments.splice(1, 0, code);
    }
    const newPath = segments.join("/") || `/${code}`;
    document.cookie = `NEXT_LOCALE=${code};path=/;max-age=${60 * 60 * 24 * 365}`;
    router.push(newPath);
  };

  // If only 1 routed locale, no switcher needed.
  if (ROUTED.length <= 1) return null;

  // If exactly 2 routed locales, show a simple toggle.
  if (ROUTED.length === 2) {
    const other = ROUTED.find((l) => l.code !== current);
    if (!other) return null;
    return (
      <button
        onClick={() => switchLocale(other.code)}
        className="font-mono text-[0.72rem] uppercase text-ink-mute hover:text-indigo transition-colors px-2 py-1"
        aria-label={fill(chrome.switcherSwitchTo, { label: other.label })}
      >
        {other.label}
      </button>
    );
  }

  // 3+ routed locales: show a dropdown. Partial packs carry a visible
  // badge so the incomplete scope is honest at the point of selection.
  return (
    <select
      value={current}
      onChange={(e) => switchLocale(e.target.value)}
      className="font-mono text-[0.72rem] uppercase text-ink-mute bg-transparent hairline-t hairline-b hairline-l hairline-r px-2 py-1 cursor-pointer hover:text-indigo transition-colors"
      aria-label={chrome.switcherLabel}
    >
      {ROUTED.map((l) => (
        <option key={l.code} value={l.code}>
          {l.status === "partial" ? `${l.label} ${chrome.partialBadge}` : l.label}
        </option>
      ))}
    </select>
  );
}
