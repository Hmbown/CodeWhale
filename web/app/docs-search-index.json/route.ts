/**
 * /docs-search-index.json — static search index for the docs sidebar search.
 *
 * Generated at build time from the repo checkout (same assumption as
 * /llms-full.txt and scripts/derive-facts.mjs); the deployed worker serves the
 * prerendered JSON and never reads disk. Fetched lazily by the client search
 * component the first time the search input is focused.
 */
import { buildSearchIndex } from "@/lib/docs-search-index";

export const dynamic = "force-static";

export async function GET() {
  return Response.json(buildSearchIndex(), {
    headers: { "content-type": "application/json; charset=utf-8" },
  });
}
