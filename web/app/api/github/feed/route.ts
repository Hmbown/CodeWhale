import { NextResponse } from "next/server";
import { fetchFeed } from "@/lib/github";
import { getEnv } from "@/lib/kv";

export const revalidate = 600;
export const dynamic = "force-dynamic";

export async function GET() {
  const env = await getEnv();
  const items = await fetchFeed(env.GITHUB_TOKEN, 50);
  return NextResponse.json({ items, fetchedAt: new Date().toISOString() });
}
