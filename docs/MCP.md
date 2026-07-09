# MCP (External Tool Servers)

codewhale can load additional tools via MCP (Model Context Protocol). MCP servers can be local stdio processes that the TUI starts, or remote URL-based servers that speak Streamable HTTP with legacy SSE fallback.

Browsing note:
- `web.run` is the canonical built-in browsing tool.
- `web_search` remains available as a compatibility alias for older prompts and integrations.

Server mode note:
- `codewhale-tui serve --mcp` runs the MCP stdio server.
- `codewhale-tui serve --http` runs the runtime HTTP/SSE API (separate mode).
- The `codewhale` dispatcher exposes `codewhale mcp-server` as an equivalent stdio
  entrypoint used by the split CLI.

## Setup wizard vs manual MCP setup (#3407)

The constitution-first `/setup` wizard includes an optional **Tools and MCP**
step. That step is discovery/readiness only:

| Wizard can do | Still requires manual / explicit action |
| --- | --- |
| Show configured servers as `healthy` / `needs_config` / `off` | Start or connect MCP servers |
| Report config path presence (global + project) | Write or edit `mcp.json` contents |
| Safe static health probe (missing command/url, broken absolute path, missing bearer env) | `codewhale mcp validate`, live connect, OAuth login |
| Point at safe on-ramps (`/mcp`, `codewhale mcp init`, `codewhale doctor`) | Install community skills, trust skills, enable plugins |
| Share Hotbar source counts from the same skill/MCP adapters (#3399) | Bind Hotbar slots (Hotbar step / `H`) |
| Record optional/`needs_action` setup_state without blocking first-run | Anything that spawns processes or installs packages |

Empty inventory is **not** an error: first-run users see “nothing configured
yet, that’s fine.” Failing or incomplete configured servers surface as
`needs_config` with an actionable hint and never block setup completion.
Enumeration never executes MCP/plugin commands beyond the static probe.
Summaries redact commands, args, env, headers, and tokens.

`codewhale doctor` reports MCP/skills/tools/plugins health with the same
optional-surface intent (paths, counts, static checks) so wizard and doctor
stay consistent.

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
codewhale-tui mcp add <name> --url "https://example.com/mcp" --bearer-token-env-var MCP_TOKEN
codewhale-tui mcp login <name>
codewhale-tui mcp logout <name>
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
/mcp login <name> [--scope scope]
/mcp logout <name>
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

## Remote HTTP Auth

URL-based MCP servers can use static headers, env-derived headers, bearer-token
env vars, or OAuth. Authorization precedence is conservative:

1. `headers` and `env_headers` are applied first.
2. `bearer_token_env_var` adds `Authorization: Bearer <env value>` when no
   Authorization header was already set.
3. Stored OAuth credentials are used only when no Authorization header exists.

For bearer-token auth, prefer env-backed config:

```json
{
  "servers": {
    "remote": {
      "url": "https://example.com/mcp",
      "bearer_token_env_var": "EXAMPLE_MCP_TOKEN"
    }
  }
}
```

For generic remote MCP OAuth, add the URL server and run login:

```bash
codewhale-tui mcp add remote --url "https://example.com/mcp"
codewhale-tui mcp login remote
```

CodeWhale discovers the server OAuth metadata, opens the authorization URL in
your browser, listens on a local callback, exchanges the code, and stores the
token response through the CodeWhale secrets backend. Stored OAuth tokens are
looked up by server name plus URL and refreshed when possible before requests.
During login, the CLI prints the authorization URL and a waiting status while
the local callback listener is active. If a URL-based server returns 401 or
Unauthorized during connect/discovery, `codewhale mcp connect <name>` reports
that OAuth authentication is required and points to
`codewhale mcp login <name>`. Resource helper listings also surface an
`authentication_required` entry for auth-shaped failures instead of silently
looking empty.

Optional OAuth fields:

```json
{
  "servers": {
    "remote": {
      "url": "https://example.com/mcp",
      "scopes": ["tools/read"],
      "oauth": {
        "client_id": "public-client-id"
      },
      "oauth_resource": "https://example.com"
    }
  }
}
```

User-level config can set callback behavior when the provider requires a fixed
redirect:

```toml
mcp_oauth_callback_port = 1455
mcp_oauth_callback_url = "http://127.0.0.1:1455/callback"
```

These callback fields are ignored from project-scope config overlays.

## Hugging Face MCP

Hugging Face provides a hosted MCP server for Hub resources, documentation,
datasets, Spaces, and community tools. CodeWhale does not call Hugging Face's
Hub HTTP APIs from `/hf`; it only helps you inspect and set up the MCP config
that the regular MCP manager will load.

The recommended setup path is Hugging Face's settings-generated configuration:

1. Visit <https://huggingface.co/settings/mcp> while signed in.
2. Choose the MCP client closest to your CodeWhale config shape and copy the
   generated server snippet.
3. Paste the Hugging Face server entry into your resolved MCP config file.
4. Restart CodeWhale, or run `/mcp reload` for the manager snapshot and restart
   if the model-visible tool pool still needs to rebuild.

CodeWhale reads both `servers` and `mcpServers`, so settings-generated snippets
can be adapted without changing the rest of the MCP file. A placeholder-only
shape looks like this:

```json
{
  "servers": {
    "huggingface": {
      "url": "https://huggingface.co/mcp",
      "headers": {
        "Authorization": "Bearer ${HF_TOKEN}"
      }
    }
  }
}
```

The placeholder above is not a runnable secret. Use the settings-generated
value in your private MCP config and never commit real Hugging Face tokens.

Interactive helpers:

```text
/hf mcp status
/hf mcp setup
/hf concepts
```

`/hf mcp status` checks the configured MCP file for common Hugging Face server
names or Hugging Face MCP URLs. `/hf concepts` explains the difference between
the Hugging Face provider route, Hugging Face MCP, and explicit Hub workflows.

Official docs: <https://huggingface.co/docs/hub/hf-mcp-server>

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
- `url` (string, optional): Streamable HTTP endpoint for a remote MCP server.
- `transport` (string, optional): set to `"sse"` for legacy SSE endpoints.
- `headers` (object, optional): literal HTTP headers for URL-based servers.
- `env_headers` or `env_http_headers` (object, optional): header names mapped to environment variable names.
- `bearer_token_env_var` (string, optional): environment variable containing a bearer token.
- `scopes` (array, optional): default OAuth scopes for `mcp login`.
- `oauth.client_id` (string, optional): pre-registered OAuth client ID.
- `oauth_resource` (string, optional): resource parameter appended to the authorization URL.

## Safety Notes

MCP tools now flow through the same tool-approval framework as built-in tools. Read-only MCP helpers (resource/prompt listing and reads) can run without prompts in suggestive approval modes, while side-effectful MCP tools require approval.

You should still only configure MCP servers you trust, and treat MCP server configuration as equivalent to running code on your machine.
Avoid committing literal `Authorization` headers. Prefer `env_headers`,
`bearer_token_env_var`, or OAuth login so secrets stay outside the MCP file.

## Troubleshooting

- Run `codewhale-tui doctor` to confirm the MCP config path it resolved and whether it exists.
- In the TUI, run `/mcp validate` to refresh the visible server/tool snapshot.
- If the MCP config is missing, run `codewhale-tui mcp init --force` to regenerate it.
- If tools don’t appear, verify the server command works from your shell and that the server supports MCP `tools/list`.
