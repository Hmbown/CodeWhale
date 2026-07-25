import { buildPageMetadata } from "@/lib/page-meta";

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/docs/mcp",
    locale,
    title: isZh ? "MCP · Codewhale 文档" : "MCP · Codewhale Docs",
    description: isZh
      ? "通过 Model Context Protocol 消费外部工具服务器，或把 Codewhale 作为 MCP 服务器暴露。"
      : "Consume external tool servers over the Model Context Protocol, or expose Codewhale itself as an MCP server.",
  });
}

export default async function McpPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const bodyClass = isZh
    ? "text-ink-soft leading-[1.9] tracking-wide"
    : "text-ink-soft leading-relaxed";

  return (
    <section className="space-y-10">
      <section id="overview" className="scroll-mt-32">
        <h2 className="font-display text-3xl mb-1">MCP</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "Codewhale 可以通过 MCP（Model Context Protocol）加载额外的工具。MCP 服务器可以是由 TUI 启动的本地 stdio 进程，也可以是远程 URL 服务器（Streamable HTTP，带旧版 SSE 回退）。连接成功的服务器会把工具注册进模型目录；失败或被禁用的服务器不会作为可用工具呈现给模型。"
            : "Codewhale can load additional tools via MCP (Model Context Protocol). MCP servers can be local stdio processes that the TUI starts, or remote URL-based servers that speak Streamable HTTP with legacy SSE fallback. A successfully connected server registers its tools into the model catalog; a failed or disabled server is never presented as an available tool."}
        </p>
        <p className={`${bodyClass} mt-3`}>
          {isZh ? (
            <>
              配置文件默认在 <code className="inline">~/.codewhale/mcp.json</code>
              （新文件缺失时仍读取旧版 <code className="inline">~/.deepseek/mcp.json</code>），可用{" "}
              <code className="inline">mcp_config_path</code> 或{" "}
              <code className="inline">DEEPSEEK_MCP_CONFIG</code> 覆盖。也兼容其他客户端使用的{" "}
              <code className="inline">mcpServers</code> 键名。
            </>
          ) : (
            <>
              The config file defaults to <code className="inline">~/.codewhale/mcp.json</code> (the
              legacy <code className="inline">~/.deepseek/mcp.json</code> is still read when the
              Codewhale file is absent), overridable with{" "}
              <code className="inline">mcp_config_path</code> or{" "}
              <code className="inline">DEEPSEEK_MCP_CONFIG</code>. The{" "}
              <code className="inline">mcpServers</code> key used by other clients is accepted too.
            </>
          )}
        </p>
      </section>

      <section id="setup" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{isZh ? "配置与管理" : "Setup and management"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh ? (
            <>
              用 <code className="inline">codewhale-tui mcp init</code> 生成初始配置；TUI 内的{" "}
              <code className="inline">/mcp</code>{" "}
              打开紧凑管理器，显示每个服务器的启用状态、传输方式、命令或 URL、超时和连接错误。常用命令：
            </>
          ) : (
            <>
              Bootstrap a starter config with <code className="inline">codewhale-tui mcp init</code>;
              inside the TUI, <code className="inline">/mcp</code> opens a compact manager showing each
              server's enabled state, transport, command or URL, timeouts, and connection errors. Common
              commands:
            </>
          )}
        </p>
        <pre className="code-block mt-4">{`codewhale-tui mcp add <name> --command "<cmd>" --arg "<arg>"
codewhale-tui mcp add <name> --url "https://example.com/mcp" --bearer-token-env-var MCP_TOKEN
codewhale-tui mcp login <name>      # OAuth for remote servers
codewhale-tui mcp list
codewhale-tui mcp validate`}</pre>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "在 TUI 里做的配置编辑会立即写盘，但模型可见的 MCP 工具池不会热加载——管理器会把它标记为需要重启。/mcp validate 和 /mcp reload 会重新连接以刷新界面快照。"
            : "Config edits made from the TUI are written immediately, but the model-visible MCP tool pool is not hot-reloaded — the manager marks it restart-required. /mcp validate and /mcp reload reconnect to refresh the on-screen snapshot."}
        </p>
      </section>

      <section id="auth" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{isZh ? "远程认证" : "Remote authentication"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "URL 服务器可以使用静态 headers、从环境变量派生的 env_headers、bearer_token_env_var 或 OAuth。优先级是保守的：先应用 headers 和 env_headers；bearer_token_env_var 只在尚未设置 Authorization 时添加；OAuth 登录获取的令牌同样不会覆盖已有的显式 header。应避免提交字面量 Authorization header——优先用 env_headers、bearer_token_env_var 或 OAuth 登录，让秘密留在 MCP 文件之外。"
            : "URL-based servers can use static headers, env-derived env_headers, bearer_token_env_var, or OAuth. Precedence is conservative: headers and env_headers apply first; bearer_token_env_var adds an Authorization header only when one is not already set; OAuth login tokens likewise never override an explicit header. Avoid committing literal Authorization headers — prefer env_headers, bearer_token_env_var, or OAuth login so secrets stay outside the MCP file."}
        </p>
      </section>

      <section id="tools" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">{isZh ? "工具命名与安全" : "Tool naming and safety"}</h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh ? (
            <>
              发现的 MCP 工具以 <code className="inline">mcp_&lt;server&gt;_&lt;tool&gt;</code>{" "}
              的形式暴露给模型——例如名为 <code className="inline">git</code> 的服务器的{" "}
              <code className="inline">status</code> 工具会变成{" "}
              <code className="inline">mcp_git_status</code>。MCP
              工具和内置工具走同一套审批框架：只读的 MCP 辅助工具在策略允许时可免提示运行，有副作用的 MCP
              工具需要审批，Full Access 也不会绕过硬策略拦截。
            </>
          ) : (
            <>
              Discovered MCP tools are exposed to the model as{" "}
              <code className="inline">mcp_&lt;server&gt;_&lt;tool&gt;</code> — a server named{" "}
              <code className="inline">git</code> with a <code className="inline">status</code> tool
              becomes <code className="inline">mcp_git_status</code>. MCP tools flow through the same
              approval framework as built-in tools: read-only MCP helpers can run without prompts when
              policy permits, side-effectful MCP tools require approval, and Full Access does not bypass
              hard policy holds.
            </>
          )}
        </p>
        <p className={`${bodyClass} mt-3`}>
          {isZh
            ? "只配置你信任的 MCP 服务器，并把 MCP 服务器配置视为等同于在本机运行代码。经过审查的本地插件包也可以贡献 MCP 服务器：它们复用同一个 MCP 管理器、审批和网络策略路径，以 <plugin>-<server> 的命名空间身份出现，边界比手写的 mcp.json 更严格。"
            : "Only configure MCP servers you trust, and treat MCP server configuration as equivalent to running code on your machine. Reviewed local plugin bundles can also contribute MCP servers: they reuse the same MCP manager, approval, and network-policy paths, appear under namespaced <plugin>-<server> identities, and are held to a stricter boundary than hand-written mcp.json."}
        </p>
      </section>

      <section id="server" className="scroll-mt-32">
        <h2 className="font-display text-2xl mb-1">
          {isZh ? "把 Codewhale 作为 MCP 服务器" : "Codewhale as an MCP server"}
        </h2>
        <p className={`${bodyClass} mt-3`}>
          {isZh ? (
            <>
              <code className="inline">codewhale-tui serve --mcp</code> 会把 Codewhale
              作为 stdio MCP 服务器运行，让其他会话（或任何 MCP 客户端）调用它的工具；
              <code className="inline">codewhale mcp-server</code> 是 dispatcher
              暴露的等价入口。<code className="inline">codewhale-tui mcp add-self</code>{" "}
              会自动解析当前二进制路径并把服务器写进你的 MCP 配置。注意区分：
              <code className="inline">serve --http</code> 是运行时 HTTP/SSE API，是另一种模式。
            </>
          ) : (
            <>
              <code className="inline">codewhale-tui serve --mcp</code> runs Codewhale as an stdio MCP
              server so other sessions (or any MCP client) can call its tools;{" "}
              <code className="inline">codewhale mcp-server</code> is the equivalent dispatcher
              entrypoint. <code className="inline">codewhale-tui mcp add-self</code> resolves the current
              binary path and writes the server into your MCP config. Keep the modes distinct:{" "}
              <code className="inline">serve --http</code> is the runtime HTTP/SSE API, a separate
              surface.
            </>
          )}
        </p>
      </section>

      <section id="source" className="hairline-t pt-8">
        <p className="text-sm text-ink-mute">
          {isZh
            ? "来源文档：docs/MCP.md · 更新时请同步修改 docs-map.ts。"
            : "Source document: docs/MCP.md · Update docs-map.ts when changing."}
        </p>
      </section>
    </section>
  );
}
