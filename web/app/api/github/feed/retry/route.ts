import { revalidatePath } from "next/cache";
import { NextResponse } from "next/server";

export const dynamic = "force-dynamic";

/**
 * The uncached half of the feed page's "Try again": the page itself is ISR,
 * so its client retry must invalidate the cached entry before refreshing,
 * or the click serves the same unavailable record for up to ten minutes.
 * Same-origin only: a third-party page must not drive the visitor's
 * regeneration. Direct HTTP clients can still revalidate, bounded by the
 * same ISR cost as fetching /feed itself; no GitHub token is spent on
 * behalf of the caller.
 */
export async function POST(req: Request) {
  const site = req.headers.get("sec-fetch-site");
  if (site && site !== "same-origin") {
    return NextResponse.json({ error: "cross-site" }, { status: 403 });
  }
  if (!site) {
    const origin = req.headers.get("origin");
    if (origin && new URL(origin).host !== req.headers.get("host")) {
      return NextResponse.json({ error: "cross-site" }, { status: 403 });
    }
  }
  revalidatePath("/[locale]/feed", "page");
  revalidatePath("/feed", "page");
  return NextResponse.json({ revalidated: true, at: new Date().toISOString() });
}
