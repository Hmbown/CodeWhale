"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import type { ChromeLink } from "@/lib/i18n/links";

/**
 * Desktop primary navigation. Both the landmark label and the small
 * bilingual companion label come from the locale's chrome dictionary — no
 * locale branch here, and no Han characters leaking into locales that never
 * asked for them.
 */
export function NavLinks({
  links,
  primaryAria,
}: {
  links: ChromeLink[];
  primaryAria: string;
}) {
  const pathname = usePathname();

  return (
    <nav className="hidden md:flex items-center gap-7" aria-label={primaryAria}>
      {links.map((l) => {
        const isActive = pathname === l.href || pathname.startsWith(`${l.href}/`);
        return (
          <Link key={l.href} href={l.href} className="nav-link group inline-flex items-baseline" aria-current={isActive ? "page" : undefined}>
            <span className="leading-none">{l.label}</span>
            {l.secondary && (
              <span className="font-cjk text-[0.66rem] leading-none ml-1.5 text-ink-mute">{l.secondary}</span>
            )}
          </Link>
        );
      })}
    </nav>
  );
}
