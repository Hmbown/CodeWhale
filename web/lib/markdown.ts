/**
 * markdown.ts — build-time Markdown → HTML rendering for repo docs.
 *
 * Used by app/[locale]/docs/[slug]/page.tsx (statically generated) and the
 * /llms-full.txt route. Runs only at build/prerender time on the machine that
 * has the full repo checkout — never on the deployed worker.
 *
 * Responsibilities:
 *   - GitHub-flavored Markdown via `marked` (tables, fenced code, task lists).
 *   - Stable heading ids (GitHub-style slugs) + heading extraction for the
 *     "On this page" outline.
 *   - Headings demoted one level (# → h2) so the page <h1> stays unique.
 *   - Relative links between repo docs resolve to rendered site pages when the
 *     target is registered in docs-map with hasPage, otherwise to GitHub.
 *   - Relative images resolve to raw.githubusercontent.com.
 */
import { Marked } from "marked";
import { DOC_TOPICS, REPO_DOCS_BASE } from "./docs-map";

/** Raw-content base for images referenced from repo markdown. */
export const REPO_RAW_BASE =
  "https://raw.githubusercontent.com/Hmbown/CodeWhale/main";

export interface DocHeading {
  /** Anchor id (GitHub-style slug, deduplicated). */
  id: string;
  /** Plain heading text. */
  text: string;
  /** Original markdown depth (1 = `#`). */
  depth: number;
}

export interface RenderedDoc {
  html: string;
  headings: DocHeading[];
}

/* ------------------------------------------------------------------ */
/*  Slugs                                                              */
/* ------------------------------------------------------------------ */

/** GitHub-compatible heading slug (close enough for repo docs). */
export function githubSlug(text: string): string {
  return text
    .trim()
    .toLowerCase()
    .replace(/<[^>]+>/g, "")
    .replace(/[^\p{L}\p{N} _-]/gu, "")
    // GitHub maps each space to a hyphen without collapsing runs.
    .replace(/ /g, "-");
}

/** Deduplicating slug generator. Share one instance across all sources
 *  rendered on the same page so heading ids never collide. */
export class Slugger {
  private seen = new Map<string, number>();
  slug(text: string): string {
    const base = githubSlug(text) || "section";
    const n = this.seen.get(base) ?? 0;
    this.seen.set(base, n + 1);
    return n === 0 ? base : `${base}-${n}`;
  }
}

/* ------------------------------------------------------------------ */
/*  Link resolution                                                    */
/* ------------------------------------------------------------------ */

/** repo path (e.g. "docs/MODES.md") → docs slug, for topics with pages. */
const SOURCE_TO_SLUG: Map<string, string> = (() => {
  const m = new Map<string, string>();
  for (const t of DOC_TOPICS) {
    if (!t.hasPage) continue;
    const sources = Array.isArray(t.repoSource) ? t.repoSource : [t.repoSource];
    for (const s of sources) m.set(s, t.slug);
  }
  return m;
})();

/** Normalize "docs/../README.md" style relative segments (posix only). */
function normalizePath(path: string): string {
  const out: string[] = [];
  for (const seg of path.split("/")) {
    if (seg === "" || seg === ".") continue;
    if (seg === "..") {
      out.pop();
      continue;
    }
    out.push(seg);
  }
  return out.join("/");
}

function isAbsoluteUrl(href: string): boolean {
  return /^[a-z][a-z0-9+.-]*:/i.test(href) || href.startsWith("//");
}

/**
 * Resolve a link found in a repo markdown file.
 *
 * @param href           Raw href from the markdown.
 * @param sourceRepoPath Repo-relative path of the file being rendered
 *                       (e.g. "docs/GUIDE.md").
 * @param locale         Active site locale for internal links.
 */
export function resolveDocHref(
  href: string,
  sourceRepoPath: string,
  locale: string,
): string {
  if (!href || isAbsoluteUrl(href) || href.startsWith("#")) return href;

  const [pathPart, ...anchorParts] = href.split("#");
  const anchor = anchorParts.length > 0 ? `#${anchorParts.join("#")}` : "";

  const sourceDir = sourceRepoPath.includes("/")
    ? sourceRepoPath.slice(0, sourceRepoPath.lastIndexOf("/"))
    : "";
  const repoPath = normalizePath(
    pathPart.startsWith("/") ? pathPart.slice(1) : `${sourceDir}/${pathPart}`,
  );

  // Link into another doc that we render as a first-class page.
  const slug = SOURCE_TO_SLUG.get(repoPath);
  if (slug) return `/${locale}/docs/${slug}${anchor}`;

  // Anything else: point at the file on GitHub.
  return `${REPO_DOCS_BASE}/${repoPath}${anchor}`;
}

/** Resolve a relative image src to raw.githubusercontent.com. */
export function resolveDocImageSrc(src: string, sourceRepoPath: string): string {
  if (!src || isAbsoluteUrl(src)) return src;
  const sourceDir = sourceRepoPath.includes("/")
    ? sourceRepoPath.slice(0, sourceRepoPath.lastIndexOf("/"))
    : "";
  const repoPath = normalizePath(
    src.startsWith("/") ? src.slice(1) : `${sourceDir}/${src}`,
  );
  return `${REPO_RAW_BASE}/${repoPath}`;
}

/* ------------------------------------------------------------------ */
/*  Rendering                                                          */
/* ------------------------------------------------------------------ */

/**
 * Render one repo markdown document to HTML.
 *
 * Headings are demoted one level (`#` → `<h2>`) so the surrounding page keeps
 * a single `<h1>`. The returned `headings` list carries the *original* depth.
 */
export function renderDocMarkdown(
  markdown: string,
  opts: { sourceRepoPath: string; locale: string; slugger?: Slugger },
): RenderedDoc {
  const { sourceRepoPath, locale } = opts;
  const slugger = opts.slugger ?? new Slugger();
  const headings: DocHeading[] = [];

  const marked = new Marked({ gfm: true });
  marked.use({
    walkTokens(token) {
      if (token.type === "link") {
        token.href = resolveDocHref(token.href, sourceRepoPath, locale);
      } else if (token.type === "image") {
        token.href = resolveDocImageSrc(token.href, sourceRepoPath);
      }
    },
    renderer: {
      heading(token) {
        // Plain text for the slug + outline: strip HTML and inline md syntax.
        const text = token.text.replace(/<[^>]+>/g, "").replace(/[`*_]/g, "");
        const id = slugger.slug(text);
        headings.push({ id, text, depth: token.depth });
        const level = Math.min(token.depth + 1, 6);
        const inner = this.parser.parseInline(token.tokens);
        return `<h${level} id="${id}" class="scroll-mt-32"><a href="#${id}" class="doc-anchor" aria-label="Link to section">${inner}</a></h${level}>\n`;
      },
    },
  });

  const html = marked.parse(markdown, { async: false });
  return { html, headings };
}
