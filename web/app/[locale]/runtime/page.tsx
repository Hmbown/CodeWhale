import { Seal } from "@/components/seal";
import { buildPageMetadata } from "@/lib/page-meta";

const REPO_BLOB_BASE = "https://github.com/Hmbown/CodeWhale/blob/main";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/runtime",
    locale,
    title: isZh ? "Runtime & 集成 · Codewhale" : "Runtime & Integrations · Codewhale",
    description: isZh
      ? "Codewhale 的本地 Runtime API、HTTP/SSE、ACP stdio 适配器、MCP 服务器、VS Code 配套扩展与消息桥接。"
      : "Codewhale's local Runtime API, HTTP/SSE, baseline ACP stdio adapter, MCP servers, Phase 0 VS Code companion, and messaging bridges.",
  });
}

interface Integration {
  name: string;
  desc: string;
  descZh: string;
  href: string;
}

const INTEGRATIONS: Integration[] = [
  {
    name: "HTTP / SSE Runtime API",
    desc: "Full local HTTP + Server-Sent Events runtime API on 127.0.0.1:7878. Create threads, stream turns, manage background jobs, and control approval decisions — all from any HTTP client or the bundled mobile page.",
    descZh: "完整的本地 HTTP + Server-Sent Events Runtime API，监听 127.0.0.1:7878。创建线程、流式对话、管理后台任务、控制审批决策——任意 HTTP 客户端或内置手机页面皆可调用。",
    href: `${REPO_BLOB_BASE}/docs/RUNTIME_API.md`,
  },
  {
    name: "ACP (Agent Client Protocol)",
    desc: "Baseline JSON-RPC adapter over stdio for compatible editor clients such as Zed. It supports initialize, new session, prompt, and cancel with text responses; shell and file tools, checkpoint replay, and session loading remain on the full Runtime API.",
    descZh: "面向 Zed 等兼容编辑器客户端的基础 JSON-RPC stdio 适配器。它支持初始化、新建会话、提示和取消，并返回文本响应；shell 与文件工具、检查点回放和会话加载仍由完整 Runtime API 提供。",
    href: `${REPO_BLOB_BASE}/docs/RUNTIME_API.md`,
  },
  {
    name: "MCP (Model Context Protocol)",
    desc: "Connect Codewhale to external tools and services through configured MCP servers over stdio or HTTP/SSE, or expose Codewhale's own tools to another MCP client.",
    descZh: "通过已配置的 MCP 服务器（stdio 或 HTTP/SSE）将 Codewhale 连接到外部工具和服务，或把 Codewhale 自身工具暴露给其他 MCP 客户端。",
    href: `${REPO_BLOB_BASE}/docs/MCP.md`,
  },
  {
    name: "VS Code Extension",
    desc: "Phase 0 companion for the local runtime. It can open Codewhale in a terminal, start and check the Runtime API, and show read-only thread summaries and restore points. It does not yet provide full chat, inline edits, or editor actions.",
    descZh: "本地 Runtime 的 Phase 0 配套扩展。它可以在终端中打开 Codewhale、启动并检查 Runtime API，以及显示只读线程摘要和还原点；目前尚不提供完整聊天、内联编辑或编辑器操作。",
    href: "https://github.com/Hmbown/CodeWhale/tree/main/extensions/vscode",
  },
  {
    name: "Telegram Bridge",
    desc: "First-party Telegram bot bridge. Start a headless Codewhale session, then chat with it from any Telegram client — approvals, tool results, and completions surface inline.",
    descZh: "官方 Telegram 机器人桥接。启动无头 Codewhale 会话，在任何 Telegram 客户端中与之对话——审批、工具结果和完成状态内联展示。",
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
            <>运行时与集成<span className="font-cjk text-indigo text-5xl ml-2">Runtime &amp; Integrations</span></>
          ) : (
            <>Runtime &amp; Integrations<span className="font-cjk text-indigo text-5xl ml-2">运行时与集成</span></>
          )}
        </h1>

        <p className="max-w-2xl text-ink-soft leading-relaxed">
          {isZh
            ? "Codewhale 不仅是一个终端 Agent——它还是一个可通过多种协议和集成方式嵌入到你现有工作流中的本地控制平面。"
            : "Codewhale is more than a terminal agent — it is a local control plane you can embed into your existing workflow through multiple protocols and integrations."}
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
            <strong className="text-ink">{isZh ? "本机运行" : "Runs on your machine"}</strong>
            <p className="mt-1">
              {isZh
                ? "Runtime API 默认仅监听 127.0.0.1。本地运行时不需要 Codewhale 账户或托管中继。"
                : "The Runtime API binds 127.0.0.1 by default. The local runtime does not require a Codewhale account or hosted relay."}
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
                ? "远程客户端通过经过认证的 Runtime API 提交请求与审批决定。本地模式、权限姿态和沙箱策略仍然生效。"
                : "Remote clients submit requests and approval decisions through the authenticated Runtime API. Local mode, permission posture, and sandbox policy still apply."}
            </p>
          </div>
          <div>
            <strong className="text-ink">{isZh ? "开放协议" : "Open protocols"}</strong>
            <p className="mt-1">
              {isZh
                ? "HTTP/SSE Runtime API、MCP 和基础 ACP stdio 适配器分别服务于不同集成场景；请根据客户端需要选择对应接口。"
                : "The HTTP/SSE Runtime API, MCP surface, and baseline ACP stdio adapter serve different integration needs; choose the interface your compatible client supports."}
            </p>
          </div>
        </div>
      </section>

      {/* Integration cards */}
      <section className="mx-auto max-w-[1100px] px-6 py-10 hairline-t">
        <div className="flex items-baseline gap-4 mb-6">
          <Seal char="集" />
          <h2 className="eyebrow">{isZh ? "集成方式" : "Integration surfaces"}</h2>
        </div>

        <div className="grid sm:grid-cols-2 gap-6">
          {INTEGRATIONS.map((item) => (
            <div key={item.name} className="hairline rounded-lg p-5 bg-paper hover:bg-paper-deep transition-colors">
              <h3 className="font-semibold text-base mb-2">
                <a href={item.href} target="_blank" rel="noopener noreferrer" className="body-link">
                  {item.name} ↗
                </a>
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
              <a
                href={`${REPO_BLOB_BASE}/docs/RUNTIME_API.md`}
                target="_blank"
                rel="noopener noreferrer"
                className="body-link"
              >
                Runtime API 与 ACP stdio 适配器 ↗
              </a>
              {" · "}
              <a
                href={`${REPO_BLOB_BASE}/docs/MCP.md`}
                target="_blank"
                rel="noopener noreferrer"
                className="body-link"
              >
                MCP 集成 ↗
              </a>
            </>
          ) : (
            <>
              Detailed implementation docs:{" "}
              <a
                href={`${REPO_BLOB_BASE}/docs/RUNTIME_API.md`}
                target="_blank"
                rel="noopener noreferrer"
                className="body-link"
              >
                Runtime API and ACP stdio adapter ↗
              </a>
              {" · "}
              <a
                href={`${REPO_BLOB_BASE}/docs/MCP.md`}
                target="_blank"
                rel="noopener noreferrer"
                className="body-link"
              >
                MCP integration ↗
              </a>
            </>
          )}
        </p>
      </section>
    </>
  );
}
