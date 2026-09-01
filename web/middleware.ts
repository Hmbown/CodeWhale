import { NextRequest, NextResponse } from "next/server";
import { detectLocaleFromHeaders } from "@/lib/i18n/detect";
import { pathLocale, replacePathLocale } from "@/lib/i18n/path";
import {
  canonicalPublicAuthPath,
  publicAuthCallbackDestination,
} from "@/lib/public-auth-routes";

const COOKIE = "NEXT_LOCALE";

const SECURITY_HEADERS: Record<string, string> = {
  "X-Frame-Options": "DENY",
  "X-Content-Type-Options": "nosniff",
  "Referrer-Policy": "strict-origin-when-cross-origin",
  "Permissions-Policy": "camera=(), microphone=(), geolocation=(), interest-cohort=()",
  "Strict-Transport-Security": "max-age=63072000; includeSubDomains; preload",
};

function applySecurityHeaders(res: NextResponse): NextResponse {
  for (const [k, v] of Object.entries(SECURITY_HEADERS)) res.headers.set(k, v);
  return res;
}

/**
 * The one host this site is indexed under. `www` is also bound to this worker
 * as a custom domain, so without this guard the entire site answers on two
 * hosts and every page has a duplicate URL a crawler can reach.
 *
 * This runs before the locale and static-asset branches on purpose: the
 * canonical host has to win for assets and API routes too, or a `www` page
 * keeps pulling subresources from `www` after the document moved.
 */
const CANONICAL_HOST = "codewhale.net";

function canonicalHostRedirect(req: NextRequest): NextResponse | null {
  const host = req.headers.get("host");
  if (!host) return null;
  // Compare without the port so local and preview hosts are untouched.
  const bare = host.split(":")[0].toLowerCase();
  if (bare !== `www.${CANONICAL_HOST}`) return null;
  const url = req.nextUrl.clone();
  url.host = CANONICAL_HOST;
  url.port = "";
  return NextResponse.redirect(url, 301);
}

export function middleware(req: NextRequest) {
  const { pathname } = req.nextUrl;

  const canonical = canonicalHostRedirect(req);
  if (canonical) return applySecurityHeaders(canonical);

  // Skip API routes, static files, _next, and the dot-less metadata route
  // for the shared OG image (but still apply security headers).
  if (
    pathname.startsWith("/api/") ||
    pathname.startsWith("/_next/") ||
    pathname === "/opengraph-image" ||
    pathname.includes(".")
  ) {
    return applySecurityHeaders(NextResponse.next());
  }

  // Auth callbacks belong on the CWC app. Locale-prefixing them produced
  // `/en/auth/callback` 404s (#5767). Preserve the query string.
  const callback = publicAuthCallbackDestination(req.nextUrl);
  if (callback) {
    return applySecurityHeaders(NextResponse.redirect(callback, 307));
  }

  // `/login` and `/register` are aliases for the public sign-in / create-account
  // pages. Fold them before locale detection so `/login` becomes `/en/signin`
  // instead of `/en/login` (which has no page).
  const canonicalAuth = canonicalPublicAuthPath(pathname);
  if (canonicalAuth && canonicalAuth !== pathname) {
    const url = req.nextUrl.clone();
    url.pathname = canonicalAuth;
    return applySecurityHeaders(NextResponse.redirect(url, 308));
  }

  // Check if locale is already in path (`pt-BR` is one segment).
  const existing = pathLocale(pathname);
  if (existing) {
    // A miscased prefix names the same route, so fold `/pt-br/install` onto
    // `/pt-BR/install` instead of letting it reach the bare-path branch
    // below, which would redirect to `/en/pt-br/install` — a 404. One
    // canonical spelling also keeps a single URL in the index.
    const canonicalPath = replacePathLocale(pathname, existing);
    let res: NextResponse;
    if (canonicalPath === pathname) {
      res = NextResponse.next();
    } else {
      const url = req.nextUrl.clone();
      url.pathname = canonicalPath;
      res = NextResponse.redirect(url, 308);
    }
    res.cookies.set(COOKIE, existing, { path: "/", maxAge: 60 * 60 * 24 * 365 });
    return applySecurityHeaders(res);
  }

  // Redirect bare paths to the detected locale (deterministic: cookie, then
  // Accept-Language full-tag/primary-subtag matching, then the default).
  const locale = detectLocaleFromHeaders(
    req.cookies.get(COOKIE)?.value,
    req.headers.get("accept-language"),
  );
  const url = req.nextUrl.clone();
  url.pathname = `/${locale}${pathname}`;
  const res = NextResponse.redirect(url);
  res.cookies.set(COOKIE, locale, { path: "/", maxAge: 60 * 60 * 24 * 365 });
  return applySecurityHeaders(res);
}

export const config = {
  // Match everything so security headers apply globally; the function
  // bypasses redirect/locale logic for /_next, /api, and dotted paths.
  matcher: ["/:path*"],
};
