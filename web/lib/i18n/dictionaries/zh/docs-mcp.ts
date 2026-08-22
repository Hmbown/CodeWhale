import type { DocsMcpDict } from "../types";

/** 中文对照见 `en/docs-mcp.ts`,文案自页面的 `isZh` 三元逐字迁入。 */
export const docsMcp: DocsMcpDict = {
  metaTitle: "MCP · Codewhale 文档",
  metaDescription:
    "通过 Model Context Protocol 消费外部工具服务器，或把 Codewhale 作为 MCP 服务器暴露。",
  bodyClassName: "text-ink-soft leading-[1.9] tracking-wide",
  overviewLead:
    "Codewhale 可以通过 MCP（Model Context Protocol）加载额外的工具。MCP 服务器可以是由 TUI 启动的本地 stdio 进程，也可以是远程 URL 服务器（Streamable HTTP，带旧版 SSE 回退）。连接成功的服务器会把工具注册进模型目录；失败或被禁用的服务器不会作为可用工具呈现给模型。",
  overviewConfig:
    "配置文件默认在 {configPath}（新文件缺失时仍读取旧版 {legacyConfigPath}），可用 {configPathOption} 或 {configEnvVar} 覆盖。也兼容其他客户端使用的 {serversKey} 键名。",
  setupTitle: "配置与管理",
  setupLead:
    "用 {initCommand} 生成初始配置；TUI 内的 {mcpCommand} 打开紧凑管理器，显示每个服务器的启用状态、传输方式、命令或 URL、超时和连接错误。常用命令：",
  setupReload:
    "在 TUI 里做的配置编辑会立即写盘，但模型可见的 MCP 工具池不会热加载——管理器会把它标记为需要重启。/mcp validate 和 /mcp reload 会重新连接以刷新界面快照。",
  authTitle: "远程认证",
  authLead:
    "URL 服务器可以使用静态 headers、从环境变量派生的 env_headers、bearer_token_env_var 或 OAuth。优先级是保守的：先应用 headers 和 env_headers；bearer_token_env_var 只在尚未设置 Authorization 时添加；OAuth 登录获取的令牌同样不会覆盖已有的显式 header。应避免提交字面量 Authorization header——优先用 env_headers、bearer_token_env_var 或 OAuth 登录，让秘密留在 MCP 文件之外。",
  toolsTitle: "工具命名与安全",
  toolsLead:
    "发现的 MCP 工具以 {toolNamePattern} 的形式暴露给模型——例如名为 {gitServer} 的服务器的 {statusTool} 工具会变成 {gitStatusTool}。MCP 工具和内置工具走同一套审批框架：只读的 MCP 辅助工具在策略允许时可免提示运行，有副作用的 MCP 工具需要审批，Full Access 也不会绕过硬策略拦截。",
  toolsTrust:
    "只配置你信任的 MCP 服务器，并把 MCP 服务器配置视为等同于在本机运行代码。经过审查的本地插件包也可以贡献 MCP 服务器：它们复用同一个 MCP 管理器、审批和网络策略路径，以 <plugin>-<server> 的命名空间身份出现，边界比手写的 mcp.json 更严格。",
  serverTitle: "把 Codewhale 作为 MCP 服务器",
  serverLead:
    "{serveMcp} 会把 Codewhale 作为 stdio MCP 服务器运行，让其他会话（或任何 MCP 客户端）调用它的工具；{mcpServerCommand} 是 dispatcher 暴露的等价入口。{addSelfCommand} 会自动解析当前二进制路径并把服务器写进你的 MCP 配置。注意区分：{serveHttp} 是运行时 HTTP/SSE API，是另一种模式。",
  sourceNote: "来源文档：docs/MCP.md · 更新时请同步修改 docs-map.ts。",
};
