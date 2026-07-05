/**
 * docs-content.ts — build-time access to the parent repo's Markdown docs.
 *
 * The website lives inside the CodeWhale monorepo (web/), so the repo checkout
 * is available at build time — the same assumption scripts/derive-facts.mjs
 * already relies on. All pages that call these helpers must be statically
 * generated (`dynamic = "force-static"` / `dynamicParams = false`) so that no
 * filesystem access ever happens on the deployed worker.
 */
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { DOC_TOPICS, type DocTopic } from "./docs-map";

/** Repo root — web/ is a direct child of the repo checkout. */
const REPO_ROOT = resolve(process.cwd(), "..");

export interface DocSource {
  /** Repo-relative path, e.g. "docs/GUIDE.md". */
  repoPath: string;
  /** Raw markdown content. */
  markdown: string;
}

/** Normalize a topic's repoSource to an array. */
export function topicSources(topic: DocTopic): string[] {
  return Array.isArray(topic.repoSource) ? topic.repoSource : [topic.repoSource];
}

/**
 * Read every markdown source for a topic from the repo checkout.
 * Throws at build time if a registered source is missing — that is the
 * desired behavior (drift must fail the build, mirroring check-docs.mjs).
 */
export function readTopicSources(topic: DocTopic): DocSource[] {
  return topicSources(topic).map((repoPath) => ({
    repoPath,
    markdown: readFileSync(resolve(REPO_ROOT, repoPath), "utf-8"),
  }));
}

/** Topics rendered from repo Markdown by the dynamic [slug] route. */
export function markdownRenderedTopics(customSlugs: readonly string[]): DocTopic[] {
  return DOC_TOPICS.filter((t) => t.hasPage && !customSlugs.includes(t.slug));
}
