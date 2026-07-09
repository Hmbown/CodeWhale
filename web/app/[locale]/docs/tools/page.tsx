import Link from "next/link";
import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/docs/tools",
    locale,
    title: isZh ? "工具 · CodeWhale 文档" : "Tools · CodeWhale Docs",
    description: isZh
      ? "类型化工具集、工具生命周期和精选工具目录。"
      : "Typed tool surface, tool lifecycle, and the curated tool catalog.",
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
              group: isZh ? "文件操作" : "File ops",
              tools: "read_file · list_dir · write_file · edit_file · apply_patch",
            },
            {
              group: isZh ? "搜索" : "Search",
              tools: "grep_files · file_search · web_search · fetch_url",
            },
            {
              group: "Shell",
              tools: "exec_shell · exec_shell_wait · exec_shell_interact",
            },
            {
              group: isZh ? "Git / 诊断 / 测试" : "Git / diag / test",
              tools: "git_status · git_diff · diagnostics · run_tests",
            },
            {
              group: isZh ? "子 Agent" : "Sub-agents",
              tools: isZh
                ? "agent —— 持久会话，并行执行；详见 docs/SUBAGENTS.md"
                : "agent — persistent sessions, parallel execution; see docs/SUBAGENTS.md",
            },
            {
              group: isZh ? "递归 LM (RLM)" : "Recursive LM (RLM)",
              tools: isZh
                ? "rlm_open · rlm_eval · rlm_configure · rlm_close —— 沙箱 Python REPL，内置 peek/search/chunk/sub_query_batch 等辅助函数"
                : "rlm_open · rlm_eval · rlm_configure · rlm_close — sandboxed Python REPL with peek/search/chunk/sub_query_batch helpers",
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

      <section id="lifecycle" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">
          {isZh ? "工具生命周期" : "Tool Lifecycle"}
        </h2>
        <p className={`text-ink-soft mt-3 ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
          {isZh
            ? "工具注册、发现、弃用和稳定的完整流程详见仓库文档。"
            : "Full lifecycle for tool registration, discovery, deprecation, and stabilization is documented in the repo."}
        </p>
        <Link
          href="https://github.com/Hmbown/CodeWhale/blob/main/docs/TOOL_LIFECYCLE.md"
          className="inline-block mt-3 font-mono text-xs uppercase tracking-wider text-indigo hover:underline"
        >
          docs/TOOL_LIFECYCLE.md →
        </Link>
      </section>

      <section id="source" className="hairline-t pt-8">
        <p className="text-sm text-ink-mute">
          {isZh
            ? "来源文档：docs/TOOL_SURFACE.md, docs/TOOL_LIFECYCLE.md · 更新时请同步修改 docs-map.ts。"
            : "Source documents: docs/TOOL_SURFACE.md, docs/TOOL_LIFECYCLE.md · Update docs-map.ts when changing."}
        </p>
      </section>
    </section>
  );
}
