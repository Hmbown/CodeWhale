"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { currentNavHref, type ChromeLink } from "@/lib/i18n/links";

export function MobileMenu({
  links,
  installHref,
  installLabel,
  signInHref,
  signInLabel,
  registerHref,
  registerLabel,
  openLabel,
  closeLabel,
  navAria,
}: {
  links: ChromeLink[];
  installHref: string;
  installLabel: string;
  signInHref: string;
  signInLabel: string;
  registerHref: string;
  registerLabel: string;
  openLabel: string;
  closeLabel: string;
  /** Accessible name for the dialog's navigation landmark. */
  navAria: string;
}) {
  const [open, setOpen] = useState(false);
  // `closing` holds the panel mounted for a short exit fade; the unmount —
  // and the focus hand-back in the effect below — then happens on the
  // timeout, not on the click.
  const [closing, setClosing] = useState(false);
  const closeTimer = useRef<number | null>(null);
  const pathname = usePathname();
  // One link is the page; ancestors are not. See currentNavHref.
  const currentHref = currentNavHref(links, pathname);
  const toggleRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  const closeImmediately = useCallback(() => {
    if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
    closeTimer.current = null;
    setOpen(false);
    setClosing(false);
  }, []);

  const close = useCallback(() => {
    // Reduced motion keeps the original instant mount/unmount.
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      closeImmediately();
      return;
    }
    if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
    setClosing(true);
    closeTimer.current = window.setTimeout(() => {
      closeTimer.current = null;
      setOpen(false);
      setClosing(false);
    }, 180);
  }, [closeImmediately]);

  const onToggle = () => {
    if (!open) {
      setOpen(true);
      return;
    }
    if (closing) {
      // Re-open mid-exit: cancel the pending unmount and stay open.
      window.clearTimeout(closeTimer.current ?? undefined);
      closeTimer.current = null;
      setClosing(false);
      return;
    }
    close();
  };

  useEffect(() => {
    if (!open) return;
    const prev = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    // aria-modal promises the dialog owns interaction. Keep background roots
    // inert, contain keyboard focus, and hand it back to the toggle on close.
    const dialog = menuRef.current;
    const backgroundRoots = Array.from(document.body.children)
      .filter((element): element is HTMLElement =>
        element instanceof HTMLElement && element !== dialog)
      .map((element) => ({ element, wasInert: element.inert }));
    for (const { element } of backgroundRoots) element.inert = true;

    // The toggle node is captured now: reading toggleRef.current inside the
    // cleanup would race React clearing the ref.
    const toggle = toggleRef.current;
    const focusable = () => Array.from(dialog?.querySelectorAll<HTMLElement>(
      'a[href], button:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])',
    ) ?? []).filter((element) => element.getClientRects().length > 0);
    focusable()[0]?.focus();

    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        close();
        return;
      }
      if (e.key !== "Tab") return;
      const candidates = focusable();
      const first = candidates[0];
      const last = candidates[candidates.length - 1];
      if (!first || !last) {
        e.preventDefault();
        return;
      }
      const active = document.activeElement;
      if (e.shiftKey && (active === first || !dialog?.contains(active))) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && (active === last || !dialog?.contains(active))) {
        e.preventDefault();
        first.focus();
      }
    };

    // Tailwind's xl boundary hides the compact controls. Close immediately
    // when a live resize crosses it so an invisible sheet cannot retain the
    // body's scroll lock.
    const desktop = window.matchMedia("(min-width: 1280px)");
    const onDesktop = (event: MediaQueryListEvent | MediaQueryList) => {
      if (event.matches) closeImmediately();
    };
    window.addEventListener("keydown", onKey);
    desktop.addEventListener("change", onDesktop);
    if (desktop.matches) closeImmediately();

    return () => {
      document.body.style.overflow = prev;
      window.removeEventListener("keydown", onKey);
      desktop.removeEventListener("change", onDesktop);
      for (const { element, wasInert } of backgroundRoots) element.inert = wasInert;
      if (toggle?.getClientRects().length) toggle.focus();
    };
  }, [close, closeImmediately, open]);

  // A pending exit timer must not outlive the component (locale switches
  // remount the nav).
  useEffect(() => {
    return () => {
      if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
    };
  }, []);

  return (
    <>
      <button
        ref={toggleRef}
        type="button"
        onClick={onToggle}
        className="xl:hidden inline-flex items-center justify-center w-9 h-9 hairline-t hairline-b hairline-l hairline-r hover:bg-paper-deep transition-colors"
        aria-label={open ? closeLabel : openLabel}
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

      {open && typeof document !== "undefined" &&
        createPortal(<div
          ref={menuRef}
          id="mobile-menu"
          className={`mm-panel xl:hidden fixed inset-0 z-40 bg-paper overflow-y-auto${closing ? " mm-closing" : ""}`}
          role="dialog"
          aria-modal="true"
          aria-label={navAria}
        >
          <div className="flex min-h-[5.75rem] items-center justify-between px-6 hairline-b">
            <span className="font-display text-lg">{navAria}</span>
            <button
              type="button"
              onClick={close}
              className="inline-flex h-9 w-9 items-center justify-center hairline-t hairline-b hairline-l hairline-r hover:bg-paper-deep transition-colors"
              aria-label={closeLabel}
            >
              <svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden>
                <path d="M2 2L12 12M12 2L2 12" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
              </svg>
            </button>
          </div>
          {/* Only one nav landmark is exposed at a time (the desktop nav is
              display:none at these widths), so the named dialog carries the
              landmark name and the inner nav stays unlabeled — two nested
              "Primary" landmarks would read as duplication. */}
          <nav className="px-6 py-4">
            <ul className="divide-y divide-[rgba(106,174,242,0.16)]">
              {links.map((l) => {
                const isActive = l.href === currentHref;
                return (
                  <li key={l.href}>
                    <Link
                      href={l.href}
                      onClick={() => setOpen(false)}
                      className={`flex items-baseline gap-3 py-4 hover:text-indigo transition-colors ${isActive ? "text-indigo" : ""}`}
                      aria-current={isActive ? "page" : undefined}
                    >
                      <span className="font-display text-lg">{l.label}</span>
                      {l.secondary && (
                        <span className="font-cjk text-sm text-ink-mute">{l.secondary}</span>
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
            <div className="mt-3 grid grid-cols-2 gap-3">
              <Link
                href={signInHref}
                onClick={() => setOpen(false)}
                className="block text-center px-5 py-3 hairline-t hairline-b hairline-l hairline-r font-mono text-sm uppercase tracking-wider hover:bg-paper-deep transition-colors"
              >
                {signInLabel}
              </Link>
              <Link
                href={registerHref}
                onClick={() => setOpen(false)}
                className="block text-center px-5 py-3 hairline-t hairline-b hairline-l hairline-r font-mono text-sm uppercase tracking-wider text-indigo hover:bg-paper-deep transition-colors"
              >
                {registerLabel}
              </Link>
            </div>
          </nav>
        </div>, document.body)}
    </>
  );
}
