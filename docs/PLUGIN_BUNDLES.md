# Plugin bundles

Codewhale v0.9.1 supports a deliberately small plugin-bundle boundary. A
bundle may contribute declarative Skills and MCP server configuration through
Codewhale's existing engines. Discovery alone never executes, enables, trusts,
downloads, updates, or installs anything. For the `/plugin install` on-ramp
that places bundles onto disk, see [PLUGINS.md](PLUGINS.md).

## Discovery and precedence

Codewhale scans only its own roots:

- User: `~/.codewhale/plugins/<name>/plugin.toml`
- Workspace: `<workspace>/.codewhale/plugins/<name>/plugin.toml`

No built-in bundle ships in v0.9.1. The internal precedence order is built-in,
user, then workspace; the first bundle with a given name wins. This prevents a
repository from shadowing an explicitly installed user bundle. Symbolic-link
roots, manifests, component paths, and nested component files fail closed.

New user and workspace bundles are always untrusted and disabled. Discovery is
read-only and does not inspect any other application's extension or credential
directories.

Pre-v0.9.1 `overrides.json` enablement is intentionally not imported as trust.
Existing bundles therefore return to disabled until they receive the v1
content and capability review.

## Manifest

Every bundle uses a versioned `plugin.toml` and a semantic version:

```toml
schema_version = 1

[plugin]
name = "example"
version = "0.1.0"
description = "Example instruction and MCP bundle"
author = "Example Author"

[skills]
path = "skills"

[mcp_servers.local]
command = "node"
args = ["server.js"]
cwd = "mcp"

[mcp_servers.remote]
url = "https://example.invalid/mcp"

[capabilities]
network_hosts = ["example.invalid"]

[when]
os = ["macos", "linux", "windows"]
binaries = ["node"]
```

Component paths must be relative, contained, present, and free of symbolic
links or Windows reparse points (including junctions and mount points). The v1
schema rejects unknown MCP fields, ambiguous local/remote
transport combinations, unbounded lists/timeouts, and overlapping tool
filters.

Remote MCP URLs must use HTTPS, except for explicit loopback HTTP endpoints.
They cannot contain user information, a query, or a fragment. Literal headers
are rejected: authentication must name a source environment variable through
`env_headers` or `bearer_token_env_var`. A remote bundle must declare exactly
the normalized host set used by its endpoints in
`capabilities.network_hosts`; endpoint scheme, normalized host, port, and path
remain bound to the review. Redirects are limited and must retain that exact
normalized origin. Reviewed remote transports use an explicit no-proxy HTTP
client: v1 bundles never read or use ambient `HTTP_PROXY`, `HTTPS_PROXY`, or
`NO_PROXY` values, because proxy credentials and proxy observation are outside
the reviewed authority. User-authored MCP configuration keeps its existing
explicit proxy support.

Local stdio environment entries must use exact `${SOURCE_ENV}` references.
The review shows destination and source names, but never reads or prints their
values. Plugin children inherit only Codewhale's base secret-scrubbed child
environment plus those reviewed mappings; credential-capable proxy variables
and the broader compatibility environment used by user-authored MCP
configuration are not inherited ambiently. Absolute arguments and parent
traversal are rejected; contained bundle entrypoints are frozen to their
staged paths before spawn.

Every stdio argument is shown losslessly as a JSON string during review.
Common credential-bearing flags and known literal token shapes are rejected
from argv; credentials must instead use a reviewed environment mapping.
Plugin-contributed MCP OAuth is disabled for v0.9.1, including discovery,
login, refresh, and token storage.

`[skills]` and `[mcp_servers.*]` are the only active component adapters in
v0.9.1. The manifest can inventory the following future surfaces, but a bundle
declaring any of them cannot be enabled yet:

```toml
[commands]
path = "commands"

[agents]
path = "agents"

[hooks]
path = "hooks"

[lsp]
path = "lsp"

[native]
path = "native"

[capabilities]
filesystem_roots = ["workspace"]
network_hosts = ["api.example.invalid"]
lifecycle_mutation = true
```

Remote MCP endpoint hosts must exactly match the displayed network inventory.
A successful environment or health check is never treated as trust.

## Review, trust, and enablement

Use the in-session command surface:

```text
/plugin list
/plugin validate example
/plugin show example
/plugin enable example
```

The first `enable` opens a review showing source, component inventory,
requested permissions, sanitized MCP endpoints, full content and capability
hashes, and inactive declarations. It also prints an exact confirmation:

```text
/plugin trust example <full-content-sha256>.<full-capability-sha256>
```

Run that exact command only after reviewing the bundle. The confirmation token
uses both complete SHA-256 receipts rather than display prefixes. Trust first
copies the complete reviewed tree into a Codewhale-owned, content-addressed
runtime snapshot and records the matching receipt; it does not activate
anything.
Then run `/plugin enable example` again. Trust and enablement are separate:

- `/plugin disable example` stops contribution while preserving trust.
- `/plugin revoke example` removes trust while preserving the enablement bit;
  the bundle remains inactive until reviewed again.
- `/plugin reload` rebuilds the current workspace registry when files have
  changed on disk.

Trust, enable, disable, revoke, and reload rebuild the current workspace's
Skill catalogue and MCP pool immediately. Each persisted transition advances a
per-bundle generation under a stable cross-process lock. A generation change
cancels in-flight MCP work, removes cached catalog entries, terminates an idle
plugin stdio child, and denies persisted queued Skills carrying the older
authority receipt.

The review distinguishes remote MCP endpoints from local stdio MCP servers.
A local stdio server is a child process running with the Codewhale user's host
filesystem and network authority; plugin trust is not an OS sandbox. The
review therefore shows the command, argument count, working directory,
environment-variable names, and this host-authority warning without printing
environment or header values. MCP tool approval still applies after the
server starts.

Trust receipts live in `~/.codewhale/plugins/state.json`. Atomic owner-only
writes record the full content hash, capability hash, reviewed capability
inventory, generation, and review time, with the latest 32 reviews retained as
a bounded audit trail. Malformed or unsupported state is not overwritten: all
bundles fail closed until the state file is repaired or moved.

The content hash covers the manifest, complete bundle tree, and executable
shape in deterministic path order, including local MCP entrypoints and
companion assets. Staging is bounded, rejects symbolic links and unsupported
file kinds (plus every Windows reparse point and hard-linked files), uses an
atomic destination swap, and applies owner-only runtime permissions or ACLs
through validated object handles on Windows. The capability hash covers the
normalized component and permission inventory. A source or staged-content
edit, capability change, or unsafe runtime-root replacement invalidates the
receipt deterministically; an already-enabled bundle becomes inactive until
it is reviewed again.

## Runtime behavior

An active bundle must be enabled, trusted for its current hashes, applicable to
the host, free of validation errors, and limited to supported component kinds.

- Skills are exposed only as `<plugin>:<skill>`. The model-facing catalogue and
  `load_skill` use an in-memory snapshot bound to the reviewed staged tree,
  rather than reading a mutable source path at execution time. `load_skill`
  revalidates source, stage, receipt, workspace, and generation immediately
  before releasing content and fails closed on drift. Queued messages persist
  the same provenance and repeat that check at dispatch. `/skills inspect`
  identifies the reviewed bundle without exposing its mutable source path.
- MCP server names are exposed as
  `plugin-<plugin-name-byte-length>-<plugin>-<server>` so hyphens in either
  component cannot create an authority collision. Disabled or untrusted
  bundles are denied again at the headless MCP adapter. Authority is checked
  before connection, immediately before every lazy stdio spawn, after
  transport construction, and before each tool/resource/prompt operation.
  Persisted generation/enablement/trust state is also watched while an
  operation is in flight, so disable, revoke, or another cross-process state
  transition cancels the operation and terminates a plugin stdio child. Full
  source and staged-tree hashes are revalidated at dispatch/catalogue
  boundaries; v0.9.1 does not continuously re-hash those trees during an
  already-running MCP call. Source or stage drift therefore fails the next
  boundary and drops the stale connection/catalogue entry, but is not claimed
  to interrupt a call already executing. Every failure includes instructions
  to reload, review, trust, and enable the bundle again.
- Plain launch, resume, fork, exec, and serve each construct an immutable
  workspace-scoped registry before constructing their Skill or MCP catalogue.
- Constitution, repository instructions, permission rules, sandbox policy,
  and MCP tool approval continue to outrank plugin instructions.

`/plugin list`, `show`, and `validate` perform no network requests, process
launches, credential reads, or configuration writes. Reviews render structural
argv as lossless JSON strings and environment provenance without values.
Credential-bearing argv is rejected at manifest validation; plugin-originated
errors suppress URL query, authentication, argv, and environment material.
Legacy executable tools under `[tools].plugin_dir` remain a distinct system
and are listed under `/plugin tools`.

## Explicit non-goals for v0.9.1

There is no remote marketplace, install/update command, ambient compatibility
discovery, automatic trust, hook adapter, command adapter, agent adapter, LSP
adapter, native extension runtime, MCP subscription adapter, or migration of
another application's bundle. These remain later work rather than implied
capabilities.
