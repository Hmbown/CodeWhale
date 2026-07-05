import Link from "next/link";
import { Seal } from "@/components/seal";
import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/runtime",
    locale,
    title: isZh ? "Runtime & 集成 · CodeWhale" : "Runtime & Integrations · CodeWhale",
    description: isZh
      ? "CodeWhale 的本地 Runtime API、HTTP/SSE、ACP 协议、MCP 服务器、VS Code 扩展、Telegram / Feishu 桥接以及实验性微信集成。"
      : "CodeWhale local Runtime API, HTTP/SSE, ACP protocol, MCP servers, VS Code extension, Telegram and Feishu bridges, and experimental Weixin integration.",
  });
}

interface Integration {
  name: string;
  desc: string;
  descZh: string;
  href?: string;
  docsHref?: string;
}

const INTEGRATIONS: Integration[] = [
  {
    name: "HTTP / SSE Runtime API",
    desc: "Full local HTTP + Server-Sent Events runtime API on 127.0.0.1:7878. Create threads, stream turns, manage background jobs, and control approval decisions — all from any HTTP client or the bundled mobile page.",
    descZh: "完整的本地 HTTP + Server-Sent Events Runtime API，监听 127.0.0.1:7878。创建线程、流式对话、管理后台任务、控制审批决策——任意 HTTP 客户端或内置手机页面皆可调用。",
    docsHref: "/en/docs/runtime-api",
  },
  {
    name: "ACP (Agent Client Protocol)",
    desc: "Open protocol for connecting coding agents to editor clients. CodeWhale speaks ACP natively over stdio (codewhale serve --acp), so ACP-capable editors can host running sessions.",
    descZh: "用于连接编码 Agent 与编辑器客户端的开放协议。CodeWhale 通过 stdio 原生支持 ACP（codewhale serve --acp），兼容 ACP 的编辑器可直接托管会话。",
    docsHref: "https://github.com/Hmbown/CodeWhale/blob/main/docs/ACP_REGISTRY_SUBMISSION.md",
  },
  {
    name: "MCP (Model Context Protocol)",
    desc: "Connect CodeWhale to external tools and services via MCP servers over stdio or HTTP/SSE. Pre-configured servers include filesystem, Git, SQLite, and popular SaaS platforms.",
    descZh: "通过 MCP 服务器（stdio 或 HTTP/SSE）将 CodeWhale 连接到外部工具和服务。预配置的服务器包括文件系统、Git、SQLite 和常用 SaaS 平台。",
    docsHref: "/en/docs/mcp",
  },
  {
    name: "VS Code Extension",
    desc: "Open-source VS Code extension that embeds CodeWhale as a side-panel agent. Run codewhale inside your editor with full workspace context.",
    descZh: "开源 VS Code 扩展，将 CodeWhale 嵌入编辑器的侧边面板。在编辑器内利用完整工作区上下文运行 CodeWhale。",
    href: "https://github.com/Hmbown/CodeWhale/tree/main/extensions/vscode",
  },
  {
    name: "Telegram Bridge",
    desc: "First-party Telegram bot bridge. Start a headless CodeWhale session, then chat with it from any Telegram client — approvals, tool results, and completions surface inline.",
    descZh: "官方 Telegram 机器人桥接。启动无头 CodeWhale 会话，在任何 Telegram 客户端中与之对话——审批、工具结果和完成状态内联展示。",
    href: "https://github.com/Hmbown/CodeWhale/tree/main/integrations/telegram-bridge",
  },
  {
    name: "Feishu / Lark Bridge",
    desc: "First-party Feishu / Lark bot bridge. Chat-native agent loop inside your Feishu workspace with approval cards, session linking, and audit trail.",
    descZh: "官方飞书 / Lark 机器人桥接。在飞书工作区内实现聊天原生 Agent 循环，支持审批卡片、会话关联和审计日志。",
    href: "https://github.com/Hmbown/CodeWhale/tree/main/integrations/feishu-bridge",
  },
  {
    name: "Weixin Bridge (实验性)",
    desc: "Experimental Weixin / WeChat bridge. Receive agent completions and approvals inside WeChat; early-stage and not recommended for production deployments.",
    descZh: "实验性微信桥接。在微信中接收 Agent 完成通知和审批；早期阶段，不建议用于生产环境。",
    href: "https://github.com/Hmbown/CodeWhale/tree/main/integrations/weixin-bridge",
  },
];

export default async function RuntimePage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";

  return (
    <>
      {/* Hero */}
      <section className="mx-auto max-w-[1100px] px-6 pt-12 pb-10">
        <div className="flex items-baseline gap-4 mb-3">
          <Seal char="接" />
          <div className="eyebrow">{isZh ? "Runtime & 集成" : "Runtime & Integrations"}</div>
        </div>
        <h1 className="font-display tracking-crisp mb-6">
          {isZh ? (
            <>Runtime & <span className="font-cjk text-indigo text-5xl ml-2">集成 Integrations</span></>
          ) : (
            <>Runtime & <span className="font-cjk text-indigo text-5xl ml-2">集成 Integrations</span></>
          )}
        </h1>

        <p className="max-w-2xl text-ink-soft leading-relaxed">
          {isZh
            ? "CodeWhale 不仅是一个终端 Agent——它还是一个可通过多种协议和集成方式嵌入到你现有工作流中的本地控制平面。"
            : "CodeWhale is more than a terminal agent — it is a local control plane you can embed into your existing workflow through multiple protocols and integrations."}
        </p>
      </section>

      {/* Trust boundary */}
      <section className="mx-auto max-w-[1100px] px-6 py-8 hairline-t">
        <div className="flex items-baseline gap-4 mb-4">
          <Seal char="信" />
          <div className="eyebrow">{isZh ? "信任边界" : "Trust boundary"}</div>
        </div>
        <div className="grid sm:grid-cols-2 gap-6 text-sm text-ink-soft leading-relaxed">
          <div>
            <strong className="text-ink">{isZh ? "本地优先" : "Local-first"}</strong>
            <p className="mt-1">
              {isZh
                ? "Runtime API 默认仅监听 127.0.0.1，无托管中继，无云依赖。你的数据、你的模型、你的控制权。"
                : "The Runtime API binds 127.0.0.1 by default. No hosted relay. No cloud dependency. Your data, your model, your control."}
            </p>
          </div>
          <div>
            <strong className="text-ink">{isZh ? "认证必需" : "Auth required"}</strong>
            <p className="mt-1">
              {isZh
                ? "所有 Runtime API 路由（/v1/*）需要 Bearer Token。配置 CODEWHALE_RUNTIME_TOKEN 环境变量或 config.toml 中的 auth_token。"
                : "All Runtime API routes (/v1/*) require a Bearer token. Set CODEWHALE_RUNTIME_TOKEN env var or auth_token in config.toml."}
            </p>
          </div>
          <div>
            <strong className="text-ink">{isZh ? "权限用户控制" : "Permissions user-controlled"}</strong>
            <p className="mt-1">
              {isZh
                ? "工具审批、shell 授权、沙箱策略和网络访问均由用户控制，不可从远程绕过。"
                : "Tool approvals, shell authorization, sandbox policies, and network access are all user-controlled and cannot be bypassed remotely."}
            </p>
          </div>
          <div>
            <strong className="text-ink">{isZh ? "开放协议" : "Open protocols"}</strong>
            <p className="mt-1">
              {isZh
                ? "CodeWhale 使用标准 HTTP/SSE、JSON-RPC 和 ACP，可与任何兼容客户端或平台集成。"
                : "CodeWhale speaks standard HTTP/SSE, JSON-RPC, and ACP — compatible with any client or platform."}
            </p>
          </div>
        </div>
      </section>

      {/* Integration cards */}
      <section className="mx-auto max-w-[1100px] px-6 py-10 hairline-t">
        <div className="flex items-baseline gap-4 mb-6">
          <Seal char="集" />
          <div className="eyebrow">{isZh ? "集成方式" : "Integration surfaces"}</div>
        </div>

        <div className="grid sm:grid-cols-2 gap-6">
          {INTEGRATIONS.map((item) => (
            <div key={item.name} className="hairline rounded-lg p-5 bg-panel-1 hover:bg-panel-2 transition-colors">
              <h3 className="font-semibold text-base mb-2">
                {item.href ? (
                  <a href={item.href} target="_blank" rel="noopener noreferrer" className="body-link">
                    {item.name} ↗
                  </a>
                ) : (
                  <Link href={item.docsHref ?? "/en/docs"} className="body-link">
                    {item.name}
                  </Link>
                )}
              </h3>
              <p className="text-sm text-ink-soft leading-relaxed">
                {isZh ? item.descZh : item.desc}
              </p>
            </div>
          ))}
        </div>
      </section>

      {/* Read more */}
      <section className="mx-auto max-w-[1100px] px-6 py-8 hairline-t">
        <p className="text-sm text-ink-soft">
          {isZh ? (
            <>
              详细实现文档：{" "}
              <Link href="/zh/docs/runtime-api" className="body-link">docs/RUNTIME_API.md</Link>
              {" · "}
              <Link href="https://github.com/Hmbown/CodeWhale/blob/main/docs/ACP_REGISTRY_SUBMISSION.md" className="body-link">ACP Registry</Link>
            </>
          ) : (
            <>
              Detailed implementation docs:{" "}
              <Link href="/en/docs/runtime-api" className="body-link">docs/RUNTIME_API.md</Link>
              {" · "}
              <Link href="https://github.com/Hmbown/CodeWhale/blob/main/docs/ACP_REGISTRY_SUBMISSION.md" className="body-link">ACP Registry</Link>
            </>
          )}
        </p>
      </section>
    </>
  );
}
