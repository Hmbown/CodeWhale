import { revalidatePath } from "next/cache";
import { NextResponse } from "next/server";

export const dynamic = "force-dynamic";

/**
 * The uncached half of the feed page's "Try again": the page itself is ISR,
 * so its client retry must invalidate the cached entry before refreshing,
 * or the click serves the same unavailable record for up to ten minutes.
 * Idempotent and side-effect-free beyond the revalidation; no GitHub token
 * is spent on behalf of the caller.
 */
export async function POST() {
  revalidatePath("/[locale]/feed", "page");
  revalidatePath("/feed", "page");
  return NextResponse.json({ revalidated: true, at: new Date().toISOString() });
}
