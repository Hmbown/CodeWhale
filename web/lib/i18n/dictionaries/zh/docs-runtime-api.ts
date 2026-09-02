import type { DocsRuntimeApiDict } from "../types";

/** 中文对照见 `en/docs-runtime-api.ts`,文案自页面的 `isZh` 三元逐字迁入。 */
export const docsRuntimeApi: DocsRuntimeApiDict = {
  metaTitle: "运行时 API · Codewhale 文档",
  metaDescription: "面向集成、桥接和自动化的本地 HTTP/SSE、JSON-RPC stdio 与 ACP 入口。",
  bodyClassName: "text-ink-soft leading-[1.9] tracking-wide",
  overviewTitle: "运行时 API",
  overviewLead:
    "codewhale app-server 是 canonical 的本地运行时 API 与控制平面。本地 SDK、移动/远控客户端和编辑器集成直接与它对话，而不是抓终端输出。引擎只作为本地进程运行：所有 API 默认绑定 localhost——没有托管中继，不托管 provider 令牌，不泄露秘密。codewhale serve --http / --mobile 保留为 app-server --http / --mobile 的兼容别名，启动的是同一个服务器；新集成应面向 app-server。",
  entries: [
    ["http", "完整 /v1/* HTTP/SSE 运行时 API（canonical 入口），默认 127.0.0.1:7878。"],
    ["mobile", "运行时 API 加 /mobile 手机控制页。"],
    ["stdio", "换行分隔的 JSON-RPC 2.0 控制传输，无监听端口，适合本地 SDK 和探针。"],
    ["web", "仅回环的浏览器客户端，内嵌于二进制并打开默认浏览器。"],
    ["doctor", "机器可读的健康与能力报告。"],
    ["acp", "面向 Zed 等编辑器的 ACP（Agent Client Protocol）stdio 适配器。"],
    [
      "exec",
      "一次性无头 worker（stream-json、fleet 子进程、CI 原语）——不属于本 API，但共享同一运行时与事件词汇。",
    ],
  ],
  stdioTitle: "零成本探测",
  stdioLead:
    "stdio 控制传输可以不花模型 token 地探测。capabilities 返回声明的方法族（thread/*、app/*、prompt/*）和完整方法列表；方法集由 crates/app-server/src/lib.rs 中的漂移测试固定，SDK 和本地集成可以放心依赖它不会悄悄变化。",
  interruptNote:
    "进行中的回合可以用 thread/interrupt（或 HTTP 的 POST /v1/threads/{id}/turns/{turn_id}/interrupt）请求中断；没有正在流式输出的回合时返回 interrupted: false——这不是错误，只是没有可停的东西。",
  securityTitle: "安全边界",
  securityLead:
    "运行时 API 令牌按 {authToken}、{runtimeTokenEnv}、{legacyTokenEnv} 的顺序读取；{insecureFlag} 只允许与回环绑定一起使用。浏览器侧的跨源请求会被 CORS 允许列表拒绝。选择非回环绑定（尤其是 {mobileFlag}）之前，请阅读 docs/RUNTIME_API.md 的完整部署与认证约定。",
  sourceNote: "来源文档：docs/RUNTIME_API.md · 更新时请同步修改 docs-map.ts。",
};
