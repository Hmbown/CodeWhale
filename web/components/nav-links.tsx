"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { currentNavHref, type ChromeLink } from "@/lib/i18n/links";

/**
 * Desktop primary navigation. Labels come from the locale's chrome
 * dictionary — no locale branch here, and no Han characters leaking into
 * locales that never asked for them.
 *
 * Companion labels stay on the compact sheet (mobile-menu.tsx). On the
 * 76rem desktop strip they do not fit: at 2xl, de/pt-BR/id secondaries
 * collapsed the home wordmark to a zero-width hit target (#5290).
 *
 * Wrapping is the strip's escape valve — the container is capped at 76rem,
 * so the six longest translated labels (de on a docs route, which also
 * carries the theme control) can still exceed it. The row gap is tight so a
 * wrapped second row does not double the height of the sticky header.
 */
export function NavLinks({
  links,
  primaryAria,
}: {
  links: ChromeLink[];
  primaryAria: string;
}) {
  const pathname = usePathname();
  // One link is the page; ancestors are not. See currentNavHref.
  const current = currentNavHref(links, pathname);

  return (
    <nav className="hidden xl:flex min-w-0 shrink items-center gap-x-5 gap-y-1 flex-wrap" aria-label={primaryAria}>
      {links.map((l) => {
        const isActive = l.href === current;
        return (
          <Link key={l.href} href={l.href} className="nav-link group inline-flex items-baseline" aria-current={isActive ? "page" : undefined}>
            <span className="leading-none">{l.label}</span>
          </Link>
        );
      })}
    </nav>
  );
}
