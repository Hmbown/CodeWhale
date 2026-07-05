/**
 * docs-search-index.ts — build-time generation of the docs search index.
 *
 * Mirrors how /llms-full.txt aggregates every rendered docs page's Markdown:
 * reads the registered repo sources (lib/docs-content.ts) at build time and
 * splits each into sections at headings. Served as static JSON by
 * app/docs-search-index.json/route.ts and consumed by the client search
 * component via lib/docs-search.ts (which stays dependency-free).
 *
 * Anchor fidelity: section ids are computed with the same Slugger + heading
 * text cleanup as renderDocMarkdown (lib/markdown.ts), sharing one Slugger
 * across all sources of a page, so anchors always match the rendered page.
 * Hand-authored pages (CUSTOM_DOC_PAGES) do not use the markdown renderer,
 * so their entries link to the page top instead of guessing anchors.
 */
import { Marked } from "marked";
import { CUSTOM_DOC_PAGES, DOC_TOPICS, type DocTopic } from "./docs-map";
import { readTopicSources } from "./docs-content";
import { Slugger } from "./markdown";
import type { SearchEntry } from "./docs-search";

/** Max stored plain-text length per section — keeps the JSON index small. */
const SECTION_TEXT_CAP = 500;

/** Strip markdown syntax from a block-level raw snippet to plain text. */
export function stripMarkdown(raw: string): string {
  return (
    raw
      // fence lines (keep the code content itself — commands are searchable)
      .replace(/^ {0,3}(?:```|~~~)[^\n]*$/gm, " ")
      // images then links → keep the label text
      .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
      .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
      // html tags, inline code ticks, emphasis
      .replace(/<[^>]+>/g, " ")
      .replace(/[`*_]/g, "")
      // table rules and pipes, blockquote markers, heading hashes
      .replace(/^\s*[|:\s-]+\s*$/gm, " ")
      .replace(/\|/g, " ")
      .replace(/^\s*>+\s?/gm, "")
      .replace(/^#{1,6}\s+/gm, "")
      .replace(/\s+/g, " ")
      .trim()
  );
}

/** Same heading-text cleanup the renderer uses for slugs and the outline. */
function headingText(rawInline: string): string {
  return rawInline.replace(/<[^>]+>/g, "").replace(/[`*_]/g, "").trim();
}

interface Section {
  anchor: string;
  heading: string;
  text: string;
}

/**
 * Split one markdown source into sections at headings, computing anchors
 * with the shared per-page slugger (matching renderDocMarkdown exactly).
 */
export function extractSections(markdown: string, slugger: Slugger): Section[] {
  const marked = new Marked({ gfm: true });
  const tokens = marked.lexer(markdown);

  const sections: Section[] = [];
  let current: Section = { anchor: "", heading: "", text: "" };

  const flush = () => {
    current.text = current.text.trim().slice(0, SECTION_TEXT_CAP);
    if (current.heading !== "" || current.text !== "") sections.push(current);
  };

  for (const token of tokens) {
    if (token.type === "heading") {
      flush();
      const text = headingText(token.text);
      current = { anchor: slugger.slug(text), heading: text, text: "" };
    } else if (token.raw) {
      const plain = stripMarkdown(token.raw);
      if (plain) current.text += (current.text ? " " : "") + plain;
    }
  }
  flush();
  return sections;
}

/** Build search entries for one registered docs topic. */
function topicEntries(topic: DocTopic): SearchEntry[] {
  const sources = readTopicSources(topic);
  const isCustom = (CUSTOM_DOC_PAGES as readonly string[]).includes(topic.slug);

  if (isCustom) {
    // Hand-authored TSX page: markdown anchors don't exist there. One
    // page-top entry, still matching against the source text.
    const text = stripMarkdown(sources.map((s) => s.markdown).join("\n\n"));
    return [
      {
        slug: topic.slug,
        page: topic.label,
        anchor: "",
        heading: "",
        text: `${topic.description.en} ${text}`.slice(0, SECTION_TEXT_CAP),
      },
    ];
  }

  const slugger = new Slugger();
  const entries: SearchEntry[] = [];
  for (const src of sources) {
    for (const section of extractSections(src.markdown, slugger)) {
      entries.push({
        slug: topic.slug,
        page: topic.label,
        anchor: section.anchor,
        heading: section.heading,
        text:
          section.anchor === ""
            ? `${topic.description.en} ${section.text}`.slice(0, SECTION_TEXT_CAP)
            : section.text,
      });
    }
  }
  return entries;
}

/** Build the full index: every topic rendered as a site page. */
export function buildSearchIndex(): SearchEntry[] {
  return DOC_TOPICS.filter((t) => t.hasPage).flatMap(topicEntries);
}
