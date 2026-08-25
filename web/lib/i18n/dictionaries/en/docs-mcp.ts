import type { DocsMcpDict } from "../types";

/**
 * English reference dictionary for `app/[locale]/docs/mcp/page.tsx`.
 * Copy moved verbatim from the page's `isZh` ternaries — any wording change
 * belongs in its own commit, never mixed into a structural move.
 */
export const docsMcp: DocsMcpDict = {
  metaTitle: "MCP · Codewhale Docs",
  metaDescription:
    "Consume external tool servers over the Model Context Protocol, or expose Codewhale itself as an MCP server.",
  bodyClassName: "text-ink-soft leading-relaxed",
  overviewLead:
    "Codewhale can load additional tools via MCP (Model Context Protocol). MCP servers can be local stdio processes that the TUI starts, or remote URL-based servers that speak Streamable HTTP with legacy SSE fallback. A successfully connected server registers its tools into the model catalog; a failed or disabled server is never presented as an available tool.",
  overviewConfig:
    "The config file defaults to {configPath} (the legacy {legacyConfigPath} is still read when the Codewhale file is absent), overridable with {configPathOption} or {configEnvVar}. The {serversKey} key used by other clients is accepted too.",
  setupTitle: "Setup and management",
  setupLead:
    "Bootstrap a starter config with {initCommand}; inside the TUI, {mcpCommand} opens a compact manager showing each server's enabled state, transport, command or URL, timeouts, and connection errors. Common commands:",
  setupReload:
    "Config edits made from the TUI are written immediately, but the model-visible MCP tool pool is not hot-reloaded — the manager marks it restart-required. /mcp validate and /mcp reload reconnect to refresh the on-screen snapshot.",
  authTitle: "Remote authentication",
  authLead:
    "URL-based servers can use static headers, env-derived env_headers, bearer_token_env_var, or OAuth. Precedence is conservative: headers and env_headers apply first; bearer_token_env_var adds an Authorization header only when one is not already set; OAuth login tokens likewise never override an explicit header. Avoid committing literal Authorization headers — prefer env_headers, bearer_token_env_var, or OAuth login so secrets stay outside the MCP file.",
  toolsTitle: "Tool naming and safety",
  toolsLead:
    "Discovered MCP tools are exposed to the model as {toolNamePattern} — a server named {gitServer} with a {statusTool} tool becomes {gitStatusTool}. MCP tools flow through the same approval framework as built-in tools: read-only MCP helpers can run without prompts when policy permits, side-effectful MCP tools require approval, and Full Access does not bypass hard policy holds.",
  toolsTrust:
    "Only configure MCP servers you trust, and treat MCP server configuration as equivalent to running code on your machine. Reviewed local plugin bundles can also contribute MCP servers: they reuse the same MCP manager, approval, and network-policy paths, appear under namespaced <plugin>-<server> identities, and are held to a stricter boundary than hand-written mcp.json.",
  serverTitle: "Codewhale as an MCP server",
  serverLead:
    "{serveMcp} runs Codewhale as an stdio MCP server so other sessions (or any MCP client) can call its tools; {mcpServerCommand} is the equivalent dispatcher entrypoint. {addSelfCommand} resolves the current binary path and writes the server into your MCP config. Keep the modes distinct: {serveHttp} is the runtime HTTP/SSE API, a separate surface.",
  sourceNote: "Source document: docs/MCP.md · Update docs-map.ts when changing.",
};
