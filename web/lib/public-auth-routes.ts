/**
 * Public-site account-entry aliases (#5767).
 *
 * codewhale.net is not the signed-in CWC app. `/signin`, `/signup`, and
 * `/auth/callback` used to locale-prefix into `/en/...` and 404. This module
 * is the one mapping: callback hops to the CWC app immediately (query
 * preserved); sign-in / create-account are locale-aware public pages that
 * send the person to app.codewhale.net without implying a local CLI account
 * is required.
 */
import { APP_URL } from "./i18n/links";
import { pathLocale } from "./i18n/path";

/**
 * App-icon raster (1254×1254) used on public account-entry pages: the canonical
 * mark in white on a navy field, generated from `public/brand/mark.svg`. The
 * vector is the master — never hand-edit this PNG, regenerate it. Pinned by
 * hash so a redrawn or stale substitute fails the test rather than shipping.
 */
export const CANONICAL_MARK_SRC = "/brand/codewhale-mark.png";
export const CANONICAL_MARK_SHA256 =
  "8b4c25460cb2a913bc42e4b97a06a09e4fd3bb31a93dc2da169a2858bbb34f01";

export const APP_AUTH_CALLBACK_URL = `${APP_URL}/auth/callback`;

const SIGN_IN_SLUGS = new Set(["signin"]);
const SIGN_IN_ALIASES = new Set(["login"]);
const SIGN_UP_SLUGS = new Set(["signup"]);
const SIGN_UP_ALIASES = new Set(["register", "create-account"]);

export type PublicAuthKind = "sign-in" | "sign-up" | "callback";

/** Path after a routed locale prefix, with no leading slash. */
export function publicAuthRemainder(pathname: string): string {
  const segments = pathname.split("/").filter(Boolean);
  if (pathLocale(pathname)) segments.shift();
  return segments.join("/");
}

export function publicAuthKind(pathname: string): PublicAuthKind | null {
  const remainder = publicAuthRemainder(pathname);
  if (remainder === "auth/callback") return "callback";
  if (SIGN_IN_SLUGS.has(remainder) || SIGN_IN_ALIASES.has(remainder)) return "sign-in";
  if (SIGN_UP_SLUGS.has(remainder) || SIGN_UP_ALIASES.has(remainder)) return "sign-up";
  return null;
}

/**
 * Fold `/login` onto `/signin` and `/register` onto `/signup`, keeping the
 * locale prefix when one is already present. Bare aliases still go through
 * the locale-prefix branch after this rewrite so `/login` becomes `/en/signin`
 * rather than a 404.
 */
export function canonicalPublicAuthPath(pathname: string): string | null {
  const remainder = publicAuthRemainder(pathname);
  const canonical =
    SIGN_IN_ALIASES.has(remainder) ? "signin"
    : SIGN_UP_ALIASES.has(remainder) ? "signup"
    : null;
  if (!canonical) return null;
  const locale = pathLocale(pathname);
  return locale ? `/${locale}/${canonical}` : `/${canonical}`;
}

/** CWC app destination. Chinese public pages keep the `/zh/` app entry. */
export function publicAuthAppDestination(
  kind: Exclude<PublicAuthKind, "callback">,
  locale: string | null,
): string {
  const localized = locale === "zh" ? "/zh" : "";
  const path = kind === "sign-in" ? "/login" : "/signup";
  return `${APP_URL}${localized}${path}`;
}

/** OAuth/email callbacks belong on the CWC app, never a localized 404. */
export function publicAuthCallbackDestination(url: URL): string | null {
  if (publicAuthKind(url.pathname) !== "callback") return null;
  const destination = new URL(APP_AUTH_CALLBACK_URL);
  destination.search = url.search;
  return destination.toString();
}
