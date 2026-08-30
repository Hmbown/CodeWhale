"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { defaultLocale } from "@/lib/i18n/config";
import { getStates } from "@/lib/i18n/dictionaries";
import { pathLocale } from "@/lib/i18n/path";
import { RetryAction } from "./retry-action";
import { EmptyState, ErrorState, LoadingState } from "./surface-state";

/**
 * Route boundaries — `loading.tsx`, `error.tsx`, `not-found.tsx` — receive
 * no params, so these thin client shims read the locale from the pathname
 * and render the shared surface states with dictionary copy.
 */
function useRouteLocale(): string {
  const pathname = usePathname();
  return pathLocale(pathname ?? "") ?? defaultLocale;
}

export function LoadingRoute() {
  const locale = useRouteLocale();
  return <LoadingState locale={locale} lines={4} />;
}

export function ErrorRoute({ reset, digest }: { reset: () => void; digest?: string }) {
  const locale = useRouteLocale();
  const t = getStates(locale);
  return (
    <ErrorState
      locale={locale}
      body={digest ? `${t.errorBody} (${digest})` : t.errorBody}
      action={
        <>
          <RetryAction label={t.retry} onRetry={reset} />
          <Link href={`/${locale}`} className="portal-button portal-button-secondary">
            {t.homeLink}
          </Link>
        </>
      }
    />
  );
}

export function NotFoundRoute() {
  const locale = useRouteLocale();
  const t = getStates(locale);
  return (
    <EmptyState
      locale={locale}
      title={t.notFoundTitle}
      body={t.notFoundBody}
      action={
        <>
          <Link href={`/${locale}/docs`} className="portal-button portal-button-primary">
            {t.homeLink.replace(/\.$/, "")}
          </Link>
        </>
      }
    />
  );
}
