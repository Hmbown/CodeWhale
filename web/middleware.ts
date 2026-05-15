import { NextRequest, NextResponse } from "next/server";
import { locales, defaultLocale } from "@/lib/i18n/config";

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

function detectLocale(req: NextRequest): string {
  // 1. Cookie
  const cookie = req.cookies.get(COOKIE)?.value;
  if (cookie && locales.includes(cookie as typeof locales[number])) return cookie;

  // 2. Accept-Language header
  const accept = req.headers.get("accept-language") ?? "";
  if (/^zh/i.test(accept.split(",")[0])) return "zh";

  return defaultLocale;
}

export function middleware(req: NextRequest) {
  const { pathname } = req.nextUrl;

  // Skip API routes, static files, _next (but still apply security headers).
  if (
    pathname.startsWith("/api/") ||
    pathname.startsWith("/_next/") ||
    pathname.includes(".")
  ) {
    return applySecurityHeaders(NextResponse.next());
  }

  // Check if locale is already in path
  const seg = pathname.split("/")[1];
  if (locales.includes(seg as typeof locales[number])) {
    const res = NextResponse.next();
    res.cookies.set(COOKIE, seg, { path: "/", maxAge: 60 * 60 * 24 * 365 });
    return applySecurityHeaders(res);
  }

  // Redirect bare paths to detected locale
  const locale = detectLocale(req);
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
