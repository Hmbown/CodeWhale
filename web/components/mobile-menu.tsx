"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { useEffect, useState } from "react";

type MobileLink = { href: string; label: string; cn?: string };

export function MobileMenu({
  links,
  installHref,
  installLabel,
}: {
  links: MobileLink[];
  installHref: string;
  installLabel: string;
}) {
  const [open, setOpen] = useState(false);
  const pathname = usePathname();

  useEffect(() => {
    if (!open) return;
    const prev = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => {
      document.body.style.overflow = prev;
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <>
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="md:hidden inline-flex items-center justify-center w-9 h-9 hairline-t hairline-b hairline-l hairline-r hover:bg-paper-deep transition-colors"
        aria-label={open ? "Close menu" : "Open menu"}
        aria-expanded={open}
        aria-controls="mobile-menu"
      >
        {open ? (
          <svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden>
            <path d="M2 2L12 12M12 2L2 12" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
          </svg>
        ) : (
          <svg width="16" height="12" viewBox="0 0 16 12" fill="none" aria-hidden>
            <path d="M0 1H16M0 6H16M0 11H16" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
          </svg>
        )}
      </button>

      {open && (
        <div
          id="mobile-menu"
          className="md:hidden fixed inset-x-0 top-[4.25rem] bottom-0 z-40 bg-paper hairline-t overflow-y-auto"
          role="dialog"
          aria-modal="true"
        >
          <nav className="px-6 py-4">
            <ul className="divide-y divide-[rgba(14,14,16,0.18)]">
              {links.map((l) => {
                const isActive = pathname === l.href || pathname.startsWith(`${l.href}/`);
                return (
                  <li key={l.href}>
                    <Link
                      href={l.href}
                      onClick={() => setOpen(false)}
                      className={`flex items-baseline gap-3 py-4 hover:text-indigo transition-colors ${isActive ? "text-indigo" : ""}`}
                      aria-current={isActive ? "page" : undefined}
                    >
                      <span className="font-display text-lg">{l.label}</span>
                      {l.cn && (
                        <span className="font-cjk text-sm text-ink-mute">{l.cn}</span>
                      )}
                      <span className="ml-auto font-mono text-xs text-ink-mute">→</span>
                    </Link>
                  </li>
                );
              })}
            </ul>

            <Link
              href={installHref}
              onClick={() => setOpen(false)}
              className="mt-6 block w-full text-center px-5 py-3 bg-indigo text-paper font-mono text-sm uppercase tracking-wider hover:bg-indigo-deep transition-colors"
            >
              {installLabel}
            </Link>
          </nav>
        </div>
      )}
    </>
  );
}
