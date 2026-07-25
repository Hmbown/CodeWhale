import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/docs/runtime-api",
    locale,
    title: isZh ? "运行时 API · Codewhale 文档" : "Runtime API · Codewhale Docs",
    description: isZh
      ? "面向集成、桥接和自动化的本地 HTTP/SSE、JSON-RPC stdio 与 ACP 入口。"
      : "Local HTTP/SSE, JSON-RPC stdio, and ACP entrypoints for integrations, bridges, and automation.",
  });
}

export default async function RuntimeApiPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const bodyClass = isZh
    ? "text-ink-soft leading-[1.9] tracking-wide"
    : "text-ink-soft leading-relaxed";
  const entries = isZh
    ? [
        { cmd: "codewhale app-server --http", detail: "完整 /v1/* HTTP/SSE 运行时 API（canonical 入口），默认 127.0.0.1:7878。" },
        { cmd: "codewhale app-server --mobile", detail: "运行时 API 加 /mobile 手机控制页。" },
        { cmd: "codewhale app-server --stdio", detail: "换行分隔的 JSON-RPC 2.0 控制传输，无监听端口，适合本地 SDK 和探针。" },
        { cmd: "codewhale web [--port 7878]", detail: "仅回环的浏览器客户端，内嵌于二进制并打开默认浏览器。" },
        { cmd: "codewhale doctor --json", detail: "机器可读的健康与能力报告。" },
        { cmd: "codewhale serve --acp", detail: "面向 Zed 等编辑器的 ACP（Agent Client Protocol）stdio 适配器。" },
        { cmd: "codewhale exec [args]", detail: "一次性无头 worker（stream-json、Fleet 子进程、CI 原语）——不属于本 API，但共享同一运行时与事件词汇。" },
      ]
    : [
        { cmd: "codewhale app-server --http", detail: "The full /v1/* HTTP/SSE runtime API (canonical entry), default 127.0.0.1:7878." },
        { cmd: "codewhale app-server --mobile", detail: "The runtime API plus the /mobile phone control page." },
        { cmd: "codewhale app-server --stdio", detail: "Newline-delimited JSON-RPC 2.0 control transport with no listener, for local SDKs and probes." },
        { cmd: "codewhale web [--port 7878]", detail: "The loopback-only browser client, embedded in the binary and opened in the default browser." },
        { cmd: "codewhale doctor --json", detail: "Machine-readable health and capability report." },
        { cmd: "codewhale serve --acp", detail: "ACP (Agent Client Protocol) stdio adapter for editors such as Zed." },
        { cmd: "codewhale exec [args]", detail: "The one-shot headless worker (stream-json, fleet subprocess, CI primitive) — not part of this API, but it shares the same runtime and event vocabulary." },
      ];

  return (
    <section className="space-y-10">
      <section id="overview" className="scroll-mt-32">
        <h2 className="font-display text-3xl mb-1">{isZh ? "运行时 API" : "Runtime API"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "codewhale app-server 是 canonical 的本地运行时 API 与控制平面。本地 SDK、移动/远控客户端和编辑器集成直接与它对话，而不是抓终端输出。引擎只作为本地进程运行：所有 API 默认绑定 localhost——没有托管中继，不托管 provider 令牌，不泄露秘密。codewhale serve --http / --mobile 保留为 app-server --http / --mobile 的兼容别名，启动的是同一个服务器；新集成应面向 app-server。"
            : "codewhale app-server is the canonical local runtime API and control plane. Local SDKs, mobile/remote-control clients, and editor integrations talk to it instead of screen-scraping terminal output. The engine runs as a local-only process: every API binds to localhost by default — no hosted relay, no provider-token custody, no secret leakage. codewhale serve --http / --mobile remain compatibility aliases for app-server --http / --mobile and launch the identical server; new integrations should target app-server."}
        </p>
        <div className="hairline-t mt-6">
          {entries.map((row) => (
            <section key={row.cmd} className="py-4 hairline-b">
              <h3 className="font-mono text-sm font-semibold">{row.cmd}</h3>
              <p className={`${bodyClass} mt-1 text-sm`}>{row.detail}</p>
            </section>
          ))}
        </div>
      </section>

      <section id="stdio" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{isZh ? "零成本探测" : "Probe without model tokens"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "stdio 控制传输可以不花模型 token 地探测。capabilities 返回声明的方法族（thread/*、app/*、prompt/*）和完整方法列表；方法集由 crates/app-server/src/lib.rs 中的漂移测试固定，SDK 和本地集成可以放心依赖它不会悄悄变化。"
            : "The stdio control transport can be probed without spending model tokens. capabilities returns the advertised method families (thread/*, app/*, prompt/*) and the full method list; the method set is pinned by a drift test in crates/app-server/src/lib.rs, so SDK and local integration clients can rely on it not changing silently."}
        </p>
        <pre className="code-block mt-4">{`printf '%s\n' \\
  '{"jsonrpc":"2.0","id":1,"method":"healthz"}' \\
  '{"jsonrpc":"2.0","id":2,"method":"capabilities"}' \\
  '{"jsonrpc":"2.0","id":3,"method":"shutdown"}' \\
  | codewhale app-server --stdio`}</pre>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "进行中的回合可以用 thread/interrupt（或 HTTP 的 POST /v1/threads/{id}/turns/{turn_id}/interrupt）请求中断；没有正在流式输出的回合时返回 interrupted: false——这不是错误，只是没有可停的东西。"
            : "A live turn can be asked to stop with thread/interrupt (or POST /v1/threads/{id}/turns/{turn_id}/interrupt over HTTP); when no turn is streaming the reply carries interrupted: false — not an error, just nothing to stop."}
        </p>
      </section>

      <section id="security" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{isZh ? "安全边界" : "Security boundary"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh ? (
            <>
              运行时 API 令牌按 <code className="inline">--auth-token</code>、
              <code className="inline">CODEWHALE_RUNTIME_TOKEN</code>、
              <code className="inline">DEEPSEEK_RUNTIME_TOKEN</code> 的顺序读取；
              <code className="inline">--insecure-no-auth</code> 只允许与回环绑定一起使用。浏览器侧的跨源请求会被
              CORS 允许列表拒绝。选择非回环绑定（尤其是{" "}
              <code className="inline">app-server --mobile</code>）之前，请阅读 docs/RUNTIME_API.md
              的完整部署与认证约定。
            </>
          ) : (
            <>
              The runtime API token is read from <code className="inline">--auth-token</code>, then{" "}
              <code className="inline">CODEWHALE_RUNTIME_TOKEN</code>, then{" "}
              <code className="inline">DEEPSEEK_RUNTIME_TOKEN</code>;{" "}
              <code className="inline">--insecure-no-auth</code> is only accepted with a loopback bind.
              Cross-origin browser requests are rejected by the CORS allow-list. Before selecting a
              non-loopback bind — especially <code className="inline">app-server --mobile</code> — read
              the full deployment and authentication contract in docs/RUNTIME_API.md.
            </>
          )}
        </p>
      </section>

      <section id="source" className="hairline-t pt-8">
        <p className="text-sm text-ink-mute">
          {isZh
            ? "来源文档：docs/RUNTIME_API.md · 更新时请同步修改 docs-map.ts。"
            : "Source document: docs/RUNTIME_API.md · Update docs-map.ts when changing."}
        </p>
      </section>
    </section>
  );
}
