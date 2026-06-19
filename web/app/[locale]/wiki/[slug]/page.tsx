import { readFile } from "node:fs/promises";
import path from "node:path";
import type { ReactNode } from "react";
import Link from "next/link";
import { notFound } from "next/navigation";
import { Seal } from "@/components/seal";
import { buildPageMetadata } from "@/lib/page-meta";
import {
  getWikiPage,
  WIKI_PAGES,
  wikiHref,
  wikiStatusClass,
  wikiStatusLabel,
  type WikiPage,
} from "@/lib/wiki";

const wikiRoot = path.join(process.cwd(), "..", "wiki");

export function generateStaticParams() {
  return WIKI_PAGES.map((page) => ({ slug: page.slug }));
}

export async function generateMetadata({
  params,
}: {
  params: Promise<{ locale: string; slug: string }>;
}) {
  const { locale, slug } = await params;
  const page = getWikiPage(slug);
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: `/wiki/${slug}`,
    locale,
    title: page ? `${page.title} · CodeWhale Wiki` : "Wiki · CodeWhale",
    description: page
      ? isZh
        ? page.cn
        : page.summary
      : isZh
        ? "CodeWhale 源码地图章节。"
        : "A CodeWhale source-map chapter.",
  });
}

async function readWikiMarkdown(page: WikiPage): Promise<string> {
  return readFile(path.join(wikiRoot, page.file), "utf8");
}

function normalizeWikiHref(href: string, locale: string): string | null {
  if (href.startsWith("#") || href.startsWith("http://") || href.startsWith("https://")) {
    return href;
  }

  const [rawPath, hash] = href.split("#", 2);
  const filename = rawPath.split("/").filter(Boolean).pop();
  const page = filename ? getWikiPage(filename) : undefined;
  if (!page) return null;

  return `${wikiHref(locale, page)}${hash ? `#${hash}` : ""}`;
}

function renderInline(text: string, locale: string): ReactNode[] {
  const parts = text.split(/(`[^`]+`|\*\*[^*]+\*\*|\[[^\]]+\]\([^)]+\))/g);
  return parts.filter(Boolean).map((part, index) => {
    if (part.startsWith("`") && part.endsWith("`")) {
      return (
        <code key={index} className="inline">
          {part.slice(1, -1)}
        </code>
      );
    }

    if (part.startsWith("**") && part.endsWith("**")) {
      return <strong key={index}>{renderInline(part.slice(2, -2), locale)}</strong>;
    }

    const linkMatch = part.match(/^\[([^\]]+)\]\(([^)]+)\)$/);
    if (linkMatch) {
      const href = normalizeWikiHref(linkMatch[2], locale);
      if (!href) {
        return <span key={index}>{renderInline(linkMatch[1], locale)}</span>;
      }
      const external = href.startsWith("http://") || href.startsWith("https://");
      if (external) {
        return (
          <a key={index} href={href} className="body-link" rel="noreferrer" target="_blank">
            {renderInline(linkMatch[1], locale)}
          </a>
        );
      }
      return (
        <Link key={index} href={href} className="body-link">
          {renderInline(linkMatch[1], locale)}
        </Link>
      );
    }

    return part;
  });
}

function headingId(text: string): string {
  return text
    .toLowerCase()
    .replace(/`([^`]+)`/g, "$1")
    .replace(/[^a-z0-9\u4e00-\u9fff]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

function renderHeading(level: number, text: string, key: number, locale: string) {
  const id = headingId(text);
  const content = renderInline(text, locale);
  if (level <= 1) {
    return (
      <h1 key={key} id={id} className="scroll-mt-32 text-4xl">
        {content}
      </h1>
    );
  }
  if (level === 2) {
    return (
      <h2 key={key} id={id} className="scroll-mt-32 pt-8 text-3xl">
        {content}
      </h2>
    );
  }
  if (level === 3) {
    return (
      <h3 key={key} id={id} className="scroll-mt-32 pt-5 text-xl">
        {content}
      </h3>
    );
  }
  return (
    <h4 key={key} id={id} className="scroll-mt-32 pt-4 font-display text-base font-semibold">
      {content}
    </h4>
  );
}

function parseTableRow(line: string): string[] {
  const row = line.trim().replace(/^\|/, "").replace(/\|$/, "");
  const cells: string[] = [];
  let current = "";

  for (let index = 0; index < row.length; index += 1) {
    const char = row[index];
    if (char === "\\" && row[index + 1] === "|") {
      current += "|";
      index += 1;
      continue;
    }
    if (char === "|") {
      cells.push(current.trim());
      current = "";
      continue;
    }
    current += char;
  }

  cells.push(current.trim());
  return cells;
}

function isTableSeparator(line: string): boolean {
  return /^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)+\|?\s*$/.test(line);
}

function isBlockStart(line: string, nextLine = ""): boolean {
  const trimmed = line.trim();
  return (
    trimmed.startsWith("```") ||
    /^#{1,6}\s+/.test(trimmed) ||
    /^-{3,}$/.test(trimmed) ||
    trimmed.startsWith(">") ||
    /^[-*]\s+/.test(trimmed) ||
    /^\d+\.\s+/.test(trimmed) ||
    (trimmed.startsWith("|") && isTableSeparator(nextLine))
  );
}

function renderMarkdown(markdown: string, locale: string): ReactNode[] {
  const lines = markdown.replace(/\r\n/g, "\n").split("\n");
  const blocks: ReactNode[] = [];
  let i = 0;

  while (i < lines.length) {
    const line = lines[i];
    const trimmed = line.trim();
    if (!trimmed) {
      i += 1;
      continue;
    }

    if (trimmed.startsWith("```")) {
      const language = trimmed.slice(3).trim();
      const code: string[] = [];
      i += 1;
      while (i < lines.length && !lines[i].trim().startsWith("```")) {
        code.push(lines[i]);
        i += 1;
      }
      i += 1;
      blocks.push(
        <pre key={blocks.length} className="code-block mt-4 overflow-x-auto">
          <code data-language={language || undefined}>{code.join("\n")}</code>
        </pre>,
      );
      continue;
    }

    const heading = trimmed.match(/^(#{1,6})\s+(.+)$/);
    if (heading) {
      blocks.push(renderHeading(heading[1].length, heading[2], blocks.length, locale));
      i += 1;
      continue;
    }

    if (/^-{3,}$/.test(trimmed)) {
      blocks.push(<hr key={blocks.length} className="my-8 hairline-t" />);
      i += 1;
      continue;
    }

    if (trimmed.startsWith(">")) {
      const quote: string[] = [];
      while (i < lines.length && lines[i].trim().startsWith(">")) {
        quote.push(lines[i].trim().replace(/^>\s?/, ""));
        i += 1;
      }
      blocks.push(
        <blockquote key={blocks.length} className="my-5 border-l-2 border-indigo pl-4 text-ink-soft">
          <p className="leading-[1.85] tracking-wide">{renderInline(quote.join(" "), locale)}</p>
        </blockquote>,
      );
      continue;
    }

    if (trimmed.startsWith("|") && isTableSeparator(lines[i + 1] ?? "")) {
      const rows: string[][] = [parseTableRow(lines[i])];
      i += 2;
      while (i < lines.length && lines[i].trim().startsWith("|")) {
        rows.push(parseTableRow(lines[i]));
        i += 1;
      }
      const [header, ...body] = rows;
      blocks.push(
        <div key={blocks.length} className="my-6 overflow-x-auto hairline-t hairline-b">
          <table className="min-w-full border-collapse text-left text-sm">
            <thead>
              <tr>
                {header.map((cell, cellIndex) => (
                  <th key={cellIndex} className="border-b border-ink/15 px-3 py-2 font-mono text-[0.72rem] uppercase tracking-wider text-ink">
                    {renderInline(cell, locale)}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {body.map((row, rowIndex) => (
                <tr key={rowIndex} className="border-t border-ink/10">
                  {row.map((cell, cellIndex) => (
                    <td key={cellIndex} className="px-3 py-2 align-top leading-relaxed text-ink-soft">
                      {renderInline(cell, locale)}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>,
      );
      continue;
    }

    if (/^[-*]\s+/.test(trimmed)) {
      const items: string[] = [];
      while (i < lines.length && /^[-*]\s+/.test(lines[i].trim())) {
        let item = lines[i].trim().replace(/^[-*]\s+/, "");
        i += 1;
        while (i < lines.length && lines[i].trim() && !isBlockStart(lines[i], lines[i + 1] ?? "")) {
          item += ` ${lines[i].trim()}`;
          i += 1;
        }
        items.push(item);
      }
      blocks.push(
        <ul key={blocks.length} className="my-4 list-disc space-y-2 pl-6 text-ink-soft">
          {items.map((item, itemIndex) => (
            <li key={itemIndex} className="leading-[1.8] tracking-wide">
              {renderInline(item, locale)}
            </li>
          ))}
        </ul>,
      );
      continue;
    }

    if (/^\d+\.\s+/.test(trimmed)) {
      const items: string[] = [];
      while (i < lines.length && /^\d+\.\s+/.test(lines[i].trim())) {
        let item = lines[i].trim().replace(/^\d+\.\s+/, "");
        i += 1;
        while (i < lines.length && lines[i].trim() && !isBlockStart(lines[i], lines[i + 1] ?? "")) {
          item += ` ${lines[i].trim()}`;
          i += 1;
        }
        items.push(item);
      }
      blocks.push(
        <ol key={blocks.length} className="my-4 list-decimal space-y-2 pl-6 text-ink-soft">
          {items.map((item, itemIndex) => (
            <li key={itemIndex} className="leading-[1.8] tracking-wide">
              {renderInline(item, locale)}
            </li>
          ))}
        </ol>,
      );
      continue;
    }

    const paragraph: string[] = [];
    while (i < lines.length && lines[i].trim() && !isBlockStart(lines[i], lines[i + 1] ?? "")) {
      paragraph.push(lines[i].trim());
      i += 1;
    }
    blocks.push(
      <p key={blocks.length} className="my-4 leading-[1.85] tracking-wide text-ink-soft">
        {renderInline(paragraph.join(" "), locale)}
      </p>,
    );
  }

  return blocks;
}

export default async function WikiChapterPage({
  params,
}: {
  params: Promise<{ locale: string; slug: string }>;
}) {
  const { locale, slug } = await params;
  const page = getWikiPage(slug);
  if (!page) notFound();

  const markdown = await readWikiMarkdown(page);
  const pageIndex = WIKI_PAGES.findIndex((candidate) => candidate.slug === page.slug);
  const previous = WIKI_PAGES[pageIndex - 1];
  const next = WIKI_PAGES[pageIndex + 1];
  const isZh = locale === "zh";

  return (
    <>
      <section className="mx-auto max-w-[1400px] px-6 pt-12 pb-8">
        <div className="mb-3 flex items-baseline gap-4">
          <Seal char={page.id} />
          <div className="eyebrow">{isZh ? "Wiki · 章节" : "Wiki · Chapter"}</div>
        </div>
        <h1 className="font-display tracking-crisp">
          {page.title}{" "}
          <span className={`font-mono ml-3 align-middle text-sm uppercase tracking-widest ${wikiStatusClass(page.status)}`}>
            {wikiStatusLabel(page.status, isZh)}
          </span>
        </h1>
        <p className="mt-5 max-w-3xl text-lg leading-[1.9] tracking-wide text-ink-soft">
          {isZh ? page.cn : page.summary}
        </p>
        <div className="mt-6 flex flex-wrap gap-3">
          <Link
            href={isZh ? "/zh/wiki" : "/en/wiki"}
            className="inline-flex hairline-t hairline-b hairline-l hairline-r px-4 py-2 font-mono text-xs uppercase tracking-wider transition-colors hover:bg-paper-deep"
          >
            {isZh ? "返回 Wiki" : "Back to Wiki"}
          </Link>
          {previous && (
            <Link
              href={wikiHref(locale, previous)}
              className="inline-flex hairline-t hairline-b hairline-l hairline-r px-4 py-2 font-mono text-xs uppercase tracking-wider transition-colors hover:bg-paper-deep"
            >
              {isZh ? "上一章" : "Previous"} · {previous.id}
            </Link>
          )}
          {next && (
            <Link
              href={wikiHref(locale, next)}
              className="inline-flex bg-indigo px-4 py-2 font-mono text-xs uppercase tracking-wider text-paper transition-colors hover:bg-indigo-deep"
            >
              {isZh ? "下一章" : "Next"} · {next.id}
            </Link>
          )}
        </div>
      </section>

      <section className="mx-auto grid max-w-[1400px] grid-cols-1 gap-10 px-6 pb-20 lg:grid-cols-12">
        <aside className="lg:col-span-3">
          <div className="lg:sticky lg:top-32">
            <div className="eyebrow mb-3">{isZh ? "章节" : "Chapters"}</div>
            <nav className="hairline-t hairline-b py-3">
              {WIKI_PAGES.map((chapter) => (
                <Link
                  key={chapter.slug}
                  href={wikiHref(locale, chapter)}
                  className={`block py-1.5 text-sm transition-colors hover:text-indigo ${
                    chapter.slug === page.slug ? "text-indigo" : "text-ink-soft"
                  }`}
                >
                  <span className="mr-2 font-mono text-[0.72rem] text-ink-mute">{chapter.id}</span>
                  {chapter.title}
                </Link>
              ))}
            </nav>
          </div>
        </aside>
        <article className="wiki-prose min-w-0 lg:col-span-9">{renderMarkdown(markdown, locale)}</article>
      </section>
    </>
  );
}
