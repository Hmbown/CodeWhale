---
name: mcp-builder
description: Design, build, configure, or debug Model Context Protocol servers for DeepSeek TUI, including stdio and HTTP/SSE transports.
---

# MCP Builder

Use this skill when the user asks to create, configure, or debug an MCP server
or tool integration.

## Design Rules

- Prefer stdio MCP servers for local tools and HTTP/SSE for remote services.
- Keep tool schemas small, typed, and explicit. Return structured JSON where
  possible.
- Put secrets in environment variables, never in committed config.
- For HTTP/SSE clients, send `Accept: application/json, text/event-stream` by
  default unless the server explicitly requires something else.
- Add timeouts and clear error messages around external APIs.

## DeepSeek Setup

Common commands:

```bash
deepseek mcp init
deepseek mcp add my-server --command node --arg server.js
deepseek mcp add remote-server --url http://127.0.0.1:3000/mcp
deepseek mcp list
deepseek mcp validate
deepseek mcp tools
```

HTTP/SSE entries can include per-server headers in `~/.deepseek/mcp.json` when
credentials or custom routing headers are required.

## Workflow

1. Define the service boundary and the minimum useful tools.
2. Choose transport and credential handling.
3. Implement the server using a maintained MCP SDK when available.
4. Add the server with `deepseek mcp add` or edit `~/.deepseek/mcp.json`.
5. Run `deepseek mcp validate`, then `deepseek mcp tools`.
6. Test one happy path and one failure path before calling it done.
