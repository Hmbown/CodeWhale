import Link from "next/link";
import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/docs/tools",
    locale,
    title: isZh ? "工具 · Codewhale 文档" : "Tools · Codewhale Docs",
    description: isZh
      ? "六个小型核心工具、按需搜索、会话工具箱缓存与精确回放兼容边界。"
      : "Six small core tools, on-demand discovery, a conversation toolbox cache, and exact replay compatibility.",
  });
}

export default async function ToolsPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";

  return (
    <section className="space-y-10">
      <section id="overview" className="scroll-mt-32">
        <h2 className="font-display text-3xl mb-1">
          {isZh ? "工具" : "Tools"}{" "}
          <span className="font-cjk text-indigo text-2xl ml-2">
            {isZh ? "Tools" : "工具"}
          </span>
        </h2>
        <p className={`text-ink-soft mt-3 ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
          {isZh
            ? "精选工具集——设计思路详见 "
            : "Curated surface — see "}
          <Link
            href="https://github.com/Hmbown/CodeWhale/blob/main/docs/TOOL_SURFACE.md"
            className="body-link"
          >
            docs/TOOL_SURFACE.md
          </Link>
          {isZh ? "。" : " for design rationale."}
        </p>
        <div className="hairline-t hairline-b mt-6">
          {[
            {
              group: "read",
              tools: "path · offset? · limit?",
            },
            {
              group: "write",
              tools: "path · content",
            },
            {
              group: "edit",
              tools: "path · edits",
            },
            {
              group: "bash",
              tools: "command · timeout?",
            },
            {
              group: isZh ? "协调" : "Coordination",
              tools: isZh
                ? "agent · tool_search（始终启用；子 Agent 也有自己的搜索）"
                : "agent · tool_search (always active; every child has its own search)",
            },
            {
              group: isZh ? "延迟加载" : "Deferred",
              tools: isZh
                ? "Git · Run · tasks · todo_write · remember · Web · MCP · plugins；只在策略允许时由 tool_search 加载"
                : "Git · Run · tasks · todo_write · remember · Web · MCP · plugins; loaded by tool_search only when policy permits",
            },
            {
              group: isZh ? "会话工具箱" : "Conversation toolbox",
              tools: isZh
                ? "最多 8 个名称 / 16 KiB schema；每个子 Agent 独立、每轮重新校验"
                : "8 names / 16 KiB of schemas; independent per child and revalidated every turn",
            },
            {
              group: isZh ? "只读研究" : "Read-only research",
              tools: isZh
                ? "侦察与审查子 Agent 可搜索 Web search/fetch，但不能获得写入或任意网络权限"
                : "Scout and Reviewer children can discover Web search/fetch without gaining mutation or arbitrary network authority",
            },
            {
              group: "MCP",
              tools: isZh
                ? "mcp_<server>_<tool>——从 ~/.codewhale/mcp.json 自动注册"
                : "mcp_<server>_<tool> — auto-registered from ~/.codewhale/mcp.json",
            },
          ].map((row) => (
            <div
              key={row.group}
              className="grid md:grid-cols-12 gap-0 hairline-t py-3 px-4 hover:bg-paper-deep transition-colors min-w-0"
            >
              <div className="md:col-span-3 font-display text-sm font-semibold">
                {row.group}
              </div>
              <div className="md:col-span-9 font-mono text-[0.78rem] text-ink-soft leading-relaxed break-words min-w-0">
                {row.tools}
              </div>
            </div>
          ))}
        </div>
      </section>

      <section id="compatibility" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">
          {isZh ? "回放兼容" : "Replay compatibility"}
        </h2>
        <p className={`text-ink-soft mt-3 ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
          {isZh
            ? "旧名称只为已保存的 transcript 与协议客户端保留。精确旧调用仍使用旧 schema 的处理器，但不会出现在新模型目录或 tool_search 中；未知名称不会被猜测或模糊改写。"
            : "Legacy names remain only for saved transcripts and protocol clients. An exact old call still reaches the handler for its old schema, but stays out of new catalogs and tool_search; unknown names are never guessed or fuzzily rewritten."}
        </p>
        <Link
          href="https://github.com/Hmbown/CodeWhale/blob/main/docs/RUNTIME_SIMPLIFICATION_DESIGN.md"
          className="inline-block mt-3 font-mono text-xs uppercase tracking-wider text-indigo hover:underline"
        >
          docs/RUNTIME_SIMPLIFICATION_DESIGN.md →
        </Link>
      </section>

      <section id="source" className="hairline-t pt-8">
        <p className="text-sm text-ink-mute">
          {isZh
            ? "来源文档：docs/TOOL_SURFACE.md, docs/RUNTIME_SIMPLIFICATION_DESIGN.md · 更新时请同步修改 docs-map.ts。"
            : "Source documents: docs/TOOL_SURFACE.md, docs/RUNTIME_SIMPLIFICATION_DESIGN.md · Update docs-map.ts when changing."}
        </p>
      </section>
    </section>
  );
}
