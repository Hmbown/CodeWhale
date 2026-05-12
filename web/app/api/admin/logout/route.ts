/**
 * @ApiRoute POST /api/admin/logout
 *
 * Maintainer logout endpoint. Clears the mt_sid session cookie and
 * redirects to the admin page.
 */
import { NextResponse } from "next/server";
import { getAgentEnv, deleteSession } from "@/lib/community-agent";

export const dynamic = "force-dynamic";

const ALLOWED_LOCALES = new Set(["en", "zh"]);

function pickLocale(value: string | null | undefined): string {
  if (!value) return "en";
  return ALLOWED_LOCALES.has(value) ? value : "en";
}

export async function POST(req: Request) {
  const env = await getAgentEnv();
  const url = new URL(req.url);
  const locale = pickLocale(url.searchParams.get("locale"));

  const cookieHeader = req.headers.get("cookie") ?? "";
  let sid: string | undefined;
  for (const c of cookieHeader.split(";")) {
    const [name, ...rest] = c.trim().split("=");
    if (name === "mt_sid") {
      sid = rest.join("=");
      break;
    }
  }

  await deleteSession(env.CURATED_KV, sid);

  const res = NextResponse.redirect(new URL(`/${locale}/admin`, req.url), {
    status: 303,
    headers: { "Cache-Control": "no-store" },
  });
  res.cookies.set("mt_sid", "", {
    path: "/",
    httpOnly: true,
    secure: true,
    sameSite: "strict",
    maxAge: 0,
  });
  return res;
}
