# MCP (External Tool Servers)

codewhale can load additional tools via MCP (Model Context Protocol). MCP servers are local processes that the TUI starts and communicates with over stdio.

Browsing note:
- `web.run` is the canonical built-in browsing tool.
- `web_search` remains available as a compatibility alias for older prompts and integrations.

Server mode note:
- `codewhale-tui serve --mcp` runs the MCP stdio server.
- `codewhale-tui serve --http` runs the runtime HTTP/SSE API (separate mode).
- The `codewhale` dispatcher exposes `codewhale mcp-server` as an equivalent stdio
  entrypoint used by the split CLI.

## Bootstrap MCP Config

Create a starter MCP config at your resolved MCP path:

```bash
codewhale-tui mcp init
```

`codewhale-tui setup --mcp` performs the same MCP bootstrap alongside skills setup.

Common management commands:

```bash
codewhale-tui mcp list
codewhale-tui mcp tools [server]
codewhale-tui mcp add <name> --command "<cmd>" --arg "<arg>"
codewhale-tui mcp add <name> --url "http://localhost:3000/mcp"
codewhale-tui mcp enable <name>
codewhale-tui mcp disable <name>
codewhale-tui mcp remove <name>
codewhale-tui mcp validate
```

## In-TUI Manager

Inside the interactive TUI, `/mcp` opens a compact manager for the resolved
MCP config path. It shows each configured server, whether it is enabled or
disabled, its transport, command or URL, timeout values, connection errors,
and discovered tools/resources/prompts when discovery has been run.

Supported in-TUI actions:

```text
/mcp init
/mcp init --force
/mcp add stdio <name> <command> [args...]
/mcp add http <name> <url>
/mcp enable <name>
/mcp disable <name>
/mcp remove <name>
/mcp validate
/mcp reload
```

`/mcp validate` and `/mcp reload` reconnect for UI discovery and refresh the
manager snapshot. Config edits made from the TUI are written immediately, but
the model-visible MCP tool pool is not hot-reloaded; the manager marks this as
restart-required until the TUI is restarted.

## Config File Location

Default path:

- `~/.codewhale/mcp.json` (`~/.deepseek/mcp.json` is still read when the CodeWhale file is absent)

Overrides:

- Config: `mcp_config_path = "/path/to/mcp.json"`
- Env: `DEEPSEEK_MCP_CONFIG=/path/to/mcp.json`

`codewhale-tui mcp init` (and `codewhale-tui setup --mcp`) writes to this resolved path.

The interactive `/config` editor also exposes `mcp_config_path`. Changing it in
the TUI updates the path used by `/mcp`, and requires a restart before the
model-visible MCP tool pool is rebuilt.

After editing the file or changing `mcp_config_path`, restart the TUI.

## Tool Naming

Discovered MCP tools are exposed to the model as:

- `mcp_<server>_<tool>`

Example: a server named `git` with a tool named `status` becomes `mcp_git_status`.

The command palette includes MCP entries grouped by server. It shows disabled
and failed servers instead of hiding them, and uses the same runtime tool names
shown to the model.

## Resource and Prompt Helpers

The CLI also exposes helper tools when MCP is enabled:

- `list_mcp_resources` (optional `server` filter)
- `list_mcp_resource_templates` (optional `server` filter)
- `mcp_read_resource` / `read_mcp_resource` (aliases)
- `mcp_get_prompt`

## Minimal Example

```json
{
  "timeouts": {
    "connect_timeout": 10,
    "execute_timeout": 60,
    "read_timeout": 120
  },
  "servers": {
    "example": {
      "command": "node",
      "args": ["./path/to/your-mcp-server.js"],
      "env": {},
      "disabled": false
    }
  }
}
```

You can also use `mcpServers` instead of `servers` for compatibility with other clients.

## Running DeepSeek as an MCP Server

You can register your local DeepSeek binary as an MCP server so other DeepSeek sessions (or any MCP client) can call its tools.

### Quick Setup

```bash
codewhale-tui mcp add-self
```

This resolves the current binary path, generates a config entry that runs `codewhale-tui serve --mcp`, and writes it to your MCP config file. The default server name is `codewhale`.

Options:

- `--name <NAME>` — custom server name (default: `codewhale`)
- `--workspace <PATH>` — workspace directory for the server

### Manual Config

Equivalent manual entry in `~/.codewhale/mcp.json`:

```json
{
  "servers": {
    "codewhale": {
      "command": "/path/to/codewhale",
      "args": ["serve", "--mcp"],
      "env": {}
    }
  }
}
```

The `codewhale-tui` binary supports `serve --mcp` directly. The `codewhale`
dispatcher offers the equivalent `codewhale mcp-server` stdio entrypoint. Use
whichever is on your `PATH` (run `which codewhale` or `which codewhale-tui` to
find the full path). The `mcp add-self` command automatically resolves the
correct binary.

### Prerequisites

- The binary referenced in `command` must exist and be executable.
- The MCP server runs as a child process via stdio — no network ports required.
- Each MCP client session spawns its own server process.

### Tool Naming

Tools from a self-hosted DeepSeek server follow the standard naming convention:

- `mcp_deepseek_<tool>` (if the server is named `codewhale`)

For example, the `shell` tool becomes `mcp_deepseek_shell`.

### MCP Server vs HTTP/SSE API vs ACP

| | `codewhale-tui serve --mcp` | `codewhale-tui serve --http` | `codewhale-tui serve --acp` |
|---|---|---|---|
| **Protocol** | MCP stdio | HTTP/SSE JSON-RPC | ACP stdio |
| **Use case** | Tool server for MCP clients | Runtime API for apps | Editor agent for Zed/custom ACP clients |
| **Config** | `~/.codewhale/mcp.json` entry | Direct URL connection | Editor `agent_servers` custom command |
| **Lifecycle** | Spawned per client session | Long-running daemon | Spawned per editor agent session |

Use `mcp add-self` when you want DeepSeek tools available to other MCP clients.
Use `serve --http` when building applications that consume the API directly.
Use `serve --acp` when an editor wants to talk to DeepSeek as an ACP agent.

### Verification

After adding, test the connection:

```bash
codewhale-tui mcp validate
codewhale-tui mcp tools codewhale
```

## Server Fields

Per-server settings:

- `command` (string, required)
- `args` (array of strings, optional)
- `env` (object, optional)
- `connect_timeout`, `execute_timeout`, `read_timeout` (seconds, optional)
- `disabled` (bool, optional)
- `enabled` (bool, optional, default `true`)
- `required` (bool, optional): startup/connect validation fails if this server cannot initialize.
- `enabled_tools` (array, optional): allowlist of tool names for this server.
- `disabled_tools` (array, optional): denylist applied after `enabled_tools`.

## Safety Notes

MCP tools now flow through the same tool-approval framework as built-in tools. Read-only MCP helpers (resource/prompt listing and reads) can run without prompts in suggestive approval modes, while side-effectful MCP tools require approval.

You should still only configure MCP servers you trust, and treat MCP server configuration as equivalent to running code on your machine.

## Hugging Face MCP Server

Hugging Face provides an MCP server for Hub search, model-card lookups,
dataset discovery, and community tools. Configure it by adding a `"huggingface"`
entry to your MCP config:

```jsonc
{
  "servers": {
    "huggingface": {
      "url": "https://huggingface.co/api/mcp",
      "headers": {
        "Authorization": "Bearer ${HF_TOKEN}"
      }
    }
  }
}
```

- Replace `${HF_TOKEN}` with a [Hugging Face access token](https://huggingface.co/settings/tokens) that has read access.
- Never commit or share your real token.
- The TUI `/hf` command can detect whether this server is configured:
  - `/hf mcp status` — check configuration status
  - `/hf mcp setup` — print this skeleton
  - `/hf concepts` — explain HF provider vs MCP vs Hub

For more details: https://huggingface.co/docs/hub/hf-mcp-server

## Troubleshooting

- Run `codewhale-tui doctor` to confirm the MCP config path it resolved and whether it exists.
- In the TUI, run `/mcp validate` to refresh the visible server/tool snapshot.
- If the MCP config is missing, run `codewhale-tui mcp init --force` to regenerate it.
- If tools don’t appear, verify the server command works from your shell and that the server supports MCP `tools/list`.
