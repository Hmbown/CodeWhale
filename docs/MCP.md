# MCP (External Tool Servers)

> 阅读简体中文版：[zh_hans/MCP.md](zh_hans/MCP.md)

codewhale can load additional tools via MCP (Model Context Protocol). MCP servers can be local stdio processes that the TUI starts, or remote URL-based servers that speak Streamable HTTP with legacy SSE fallback.

Browsing note:
- `Web` is the canonical, deferred built-in browsing tool; it provides
  `search`, `fetch`, and `wait` actions when network policy permits.
- `web_search`, `fetch_url`, and `wait_for_dev_server` are hidden replay-only
  aliases. New prompts and integrations should use `Web`.

Server mode note:
- `codewhale serve --mcp` runs the MCP stdio server.
- `codewhale serve --http` runs the runtime HTTP/SSE API (separate mode).
- `codewhale mcp-server` is an equivalent stdio entrypoint on the same
  consolidated runtime.

## Setup wizard vs manual MCP setup (#3407)

The `/setup` hub includes an optional **Tools and MCP**
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

## Plugin-contributed MCP

A reviewed local plugin bundle may contribute MCP servers without creating a
second transport or approval system. The servers use the same MCP manager,
tool approval, resource, prompt, timeout, and network-policy paths documented
here, and appear under namespaced `<plugin>-<server>` identities.

The bundle boundary is intentionally stricter than user-authored `mcp.json`:
unknown fields and ambiguous transports fail closed; stdio environment values
must be exact environment-source references; remote literal headers and
secret-bearing URLs are rejected; declared network hosts must exactly match
the normalized endpoint host set; and redirects remain on the reviewed origin.
Reviewed plugin remotes also bypass ambient HTTP proxy configuration entirely;
proxy credentials and proxy-observed traffic are not part of the v1 review.
The plugin review discloses local host-user authority, structural argv,
environment provenance, endpoint, auth source names, scopes, and tool filters
without reading or printing secret values.

Trust stages reviewed content but does not enable it. Enablement attaches that
staged snapshot to the current workspace's MCP pool. Disable, revoke, and other
cross-process generation changes remove catalog entries, cancel in-flight
operations, and terminate plugin stdio children. Source or staged-tree drift is
fully revalidated before each dispatch/catalogue boundary and fails the next
boundary closed; v0.9.1 does not continuously hash mutable trees during an
already-running call and therefore does not promise drift-triggered mid-call
cancellation. MCP subscriptions are not exposed through plugin bundles. See
[Plugin bundles](PLUGIN_BUNDLES.md) for the complete lifecycle contract.

## Bootstrap MCP Config

Create a starter MCP config at your resolved MCP path:

```bash
codewhale mcp init
```

`codewhale setup --mcp` performs the same MCP bootstrap alongside skills setup.

Common management commands:

```bash
codewhale mcp list
codewhale mcp tools [server]
codewhale mcp add <name> --command "<cmd>" --arg "<arg>"
codewhale mcp add <name> --url "http://localhost:3000/mcp"
codewhale mcp add <name> --url "https://example.com/mcp" --bearer-token-env-var MCP_TOKEN
codewhale mcp login <name>
codewhale mcp logout <name>
codewhale mcp enable <name>
codewhale mcp disable <name>
codewhale mcp remove <name>
codewhale mcp validate
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
/mcp import
/mcp recommendations
/mcp add recommended <id>
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

### Suggested plugins and companion integrations

`/mcp recommendations` is Codewhale's native, curated suggestions surface.
The entries are described as product plugins, with their component type and
provenance, but `/mcp add recommended <id>` still writes only the named MCP
server component. Viewing recommendations never fetches, installs, trusts, or
enables anything. Adding one writes configuration; the server is first started
only after an explicit `/mcp restart`.

The v0.9.10 product suggestions use these reviewed, pinned definitions. The
Plugins view is the product/install surface; MCP, Skills, and sandbox adapters
are transparent component kinds and their own tabs remain operational and
diagnostic surfaces:

| Plugin | Component | Pinned definition | Provenance and maturity | Installation boundary |
| --- | --- | --- | --- | --- |
| Chrome DevTools | MCP server (stdio) | `npx -y chrome-devtools-mcp@1.7.0` (`npx.cmd` on Windows) | [Official ChromeDevTools project](https://github.com/ChromeDevTools/chrome-devtools-mcp) | npm may download the pinned package when the user restarts MCP. |
| Playwright | MCP server (stdio) | `npx -y @playwright/mcp@0.0.79 --isolated` (`npx.cmd` on Windows) | [Official Microsoft project](https://github.com/microsoft/playwright-mcp) | `--isolated` starts a fresh browser profile; npm may download the pinned package only after an explicit restart. |
| Cua Computer Use | MCP server (stdio) | `cua-driver mcp`; Driver `0.20.0` reviewed for this release | [Official Cua project](https://github.com/trycua/cua); preview integration | The signed driver and OS permissions are separate, explicit installs. `/mcp add recommended cua` only writes config and never installs or grants either. |
| Browser Use | Skill plus separately installed Python runtime | Skill/runtime release `0.13.8` | [Official browser-use project](https://github.com/browser-use/browser-use) | Optional companion: not an MCP server. Codewhale does not auto-run the upstream Skill installer or install its browser/runtime dependencies. |
| Anthropic Sandbox Runtime | Sandbox adapter companion | `@anthropic-ai/sandbox-runtime@0.0.73` | [Official anthropic-experimental project](https://github.com/anthropic-experimental/sandbox-runtime); beta | Documentation-only adapter candidate in v0.9.10: not an MCP server and not an active Codewhale plugin adapter. It does not replace Codewhale's sandbox policy. |

[Container Use](https://github.com/dagger/container-use) remains an additional
experimental suggestion with an MCP server component (`container-use stdio`).
The binary must be installed separately; `/mcp add recommended container-use`
only writes config and Codewhale never downloads it.

This presentation follows the same useful boundary found in the local
Grokbuild extensions view (one product plugin may expose MCP or Skill
components while component tabs stay inspectable), the Kimi marketplace's
explicit display name/tier/source fields, and the Codex marketplace's explicit
source and install-policy fields.
Codewhale keeps its stricter rule: provenance and foreign policy are display
metadata only, never inherited trust or automatic installation. For full
bundle and marketplace semantics, see [Plugin bundles](PLUGIN_BUNDLES.md).

`/mcp validate` (alias `/mcp doctor`) reconnects for UI discovery only: it
refreshes the manager snapshot you see in the pager, not the catalog the model
gets.

`/mcp reload` (aliases `/mcp reconnect`, `/mcp restart`) is the hot-reload path.
It re-reads the MCP config sources and reconnects through the engine-owned pool,
so the rebuilt catalog is the exact one the next model turn uses — no TUI
restart. Config edits made from the TUI are written immediately and the manager
marks the snapshot reload-required until you run it; a failed reload leaves the
previous live pool intact and says so.

Headless surfaces are the exception: the `ConfigReload` app-server request does
**not** refresh MCP connections, so a headless runtime still needs a restart
after MCP config changes.

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
codewhale mcp add remote --url "https://example.com/mcp"
codewhale mcp login remote
```

Codewhale discovers the server OAuth metadata, opens the authorization URL in
your browser, listens on a local callback, exchanges the code, and stores the
token response through the Codewhale secrets backend. Stored OAuth tokens are
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
datasets, Spaces, and community tools. Codewhale does not call Hugging Face's
Hub HTTP APIs from `/hf`; it only helps you inspect and set up the MCP config
that the regular MCP manager will load.

The recommended setup path is Hugging Face's settings-generated configuration:

1. Visit <https://huggingface.co/settings/mcp> while signed in.
2. Choose the MCP client closest to your Codewhale config shape and copy the
   generated server snippet.
3. Paste the Hugging Face server entry into your resolved MCP config file.
4. Run `/mcp reload` to rebuild the live model-visible tool pool.

Codewhale reads both `servers` and `mcpServers`, so settings-generated snippets
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

- `~/.codewhale/mcp.json` (`~/.deepseek/mcp.json` is still read when the Codewhale file is absent)

Overrides:

- Config: `mcp_config_path = "/path/to/mcp.json"`
- Env: `DEEPSEEK_MCP_CONFIG=/path/to/mcp.json`

`codewhale mcp init` (and `codewhale setup --mcp`) writes to this resolved path.

The interactive `/config` editor also exposes `mcp_config_path`. Changing it in
the TUI updates the path used by `/mcp` and marks the pool reload-required;
`/mcp reload` then switches the live pool to the new config source.

After editing the MCP file or changing `mcp_config_path`, run `/mcp reload`. No
TUI restart is needed.

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

## Running Codewhale as an MCP Server

You can register your local Codewhale binary as an MCP server so other Codewhale sessions (or any MCP client) can call its tools.

### Quick Setup

```bash
codewhale mcp add-self
```

This resolves the current binary path, generates a config entry that runs
`codewhale serve --mcp`, and writes it to your MCP config file. The default
server name is `codewhale`.

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

The consolidated `codewhale` runtime supports `serve --mcp` directly and also
offers the equivalent `codewhale mcp-server` stdio entrypoint. Release
installers expose the same runtime as `codew`; `mcp add-self` automatically
resolves the command that invoked it.

### Prerequisites

- The binary referenced in `command` must exist and be executable.
- The MCP server runs as a child process via stdio — no network ports required.
- Each MCP client session spawns its own server process.

### Tool Naming

Tools from an MCP server follow the standard naming convention:

- `mcp_<server>_<tool>`

For example, the `shell` tool from the default server (named `codewhale`)
becomes `mcp_codewhale_shell`.

### MCP Server vs HTTP/SSE API vs ACP

| | `codewhale serve --mcp` | `codewhale serve --http` | `codewhale serve --acp` |
|---|---|---|---|
| **Protocol** | MCP stdio | HTTP/SSE JSON-RPC | ACP stdio |
| **Use case** | Tool server for MCP clients | Runtime API for apps | Editor agent for Zed/custom ACP clients |
| **Config** | `~/.codewhale/mcp.json` entry | Direct URL connection | Editor `agent_servers` custom command |
| **Lifecycle** | Spawned per client session | Long-running daemon | Spawned per editor agent session |

Use `mcp add-self` when you want Codewhale tools available to other MCP clients.
Use `serve --http` when building applications that consume the API directly.
Use `serve --acp` when an editor wants to talk to Codewhale as an ACP agent.

### Verification

After adding, test the connection:

```bash
codewhale mcp validate
codewhale mcp tools codewhale
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

MCP tools flow through the same approval framework as built-in tools. Read-only
MCP helpers (resource/prompt listing and reads) can run without prompts in Ask
and Auto-Review when policy permits, while side-effectful MCP tools require
approval. Full Access does not bypass hard policy holds.

You should still only configure MCP servers you trust, and treat MCP server configuration as equivalent to running code on your machine.
Avoid committing literal `Authorization` headers. Prefer `env_headers`,
`bearer_token_env_var`, or OAuth login so secrets stay outside the MCP file.

## Troubleshooting

- Run `codewhale doctor` to confirm the MCP config path it resolved and whether it exists.
- In the TUI, run `/mcp validate` to refresh the visible server/tool snapshot.
- If tools are missing from the model's catalog after a config or credential
  change, run `/mcp reload` — `/mcp validate` only refreshes the UI snapshot.
- If the MCP config is missing, run `codewhale mcp init --force` to regenerate it.
- If tools don’t appear, verify the server command works from your shell and that the server supports MCP `tools/list`.
