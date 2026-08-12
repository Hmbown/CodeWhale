# Tool surface

This document describes the current model-facing tool contract. The v0.9.1
cutover that produced it is recorded in `docs/RUNTIME_SIMPLIFICATION_DESIGN.md`;
read the workspace version from `Cargo.toml`, not from this line. The registry
remains larger than the first-turn catalog so
saved transcripts can replay and uncommon capabilities can be loaded on demand.
The model should learn one canonical name for each common operation.

Implementation sources:

- `crates/tui/src/core/engine/tool_catalog.rs` owns the eager/deferred catalog.
- `crates/tui/src/tools/registry.rs` registers canonical tools and hidden aliases.
- `crates/tui/src/tools/{file,file_tool,shell}.rs` own the small foreground
  primitive behavior and schemas; the other native tools remain searchable.
- `docs/RUNTIME_SIMPLIFICATION_DESIGN.md` records the v0.9.1 cutover and receipt.

## Default-active contract

New turns start with exactly seven model-facing names:

1. `read`
2. `write`
3. `edit`
4. `bash`
5. `agent`
6. `todo_write`
7. `tool_search`

The first six are `DEFAULT_ACTIVE_NATIVE_TOOLS` in
`crates/tui/src/core/engine/tool_catalog.rs`. `tool_search` is synthetic and is
always active. An authority boundary may remove `agent` at the maximum child
depth, but route size alone must not change this core vocabulary.

The direct schemas deliberately stay small:

| Tool | Input | Purpose |
|---|---|---|
| `read` | `path`, optional `offset`, optional `limit` | Read a bounded file window with explicit continuation or truncation notices. |
| `write` | `path`, `content` | Create or replace a file. |
| `edit` | `path`, `edits` | Apply one or more unambiguous text replacements against one original snapshot. |
| `bash` | `command`, optional `timeout` | Run one cancellable foreground shell command and return a bounded tail. |
| `agent` | delegated task and optional scope/context controls | Start or inspect focused child work. |
| `todo_write` | complete replacement list of `{content, status}` items | Keep optional, agent-owned progress notes for genuinely multi-step work. |
| `tool_search` | `query`, optional matching controls | Discover policy-allowed deferred tools and add selected schemas to this conversation's toolbox. |

Mode is an authority decision, not a synonym system. Plan, Work, and Operate
use the same primitive identities. Plan centrally refuses `write`, `edit`, and
`bash`; Work and Operate still pass those calls through approval, sandbox,
trusted-path, repository-law, and managed-policy gates. Full Access changes
ordinary approval behavior but does not bypass hard safety or repository law.

`update_plan` remains registered only for saved-artifact compatibility and is
not model-visible. `tasks`, `Git`, `Run`, `Web`, `remember`, and other
specialized capabilities are searchable rather than first-turn ceremony.

## Deferred and dynamic tools

`Web` is conditional and deferred. It is discoverable through `tool_search`
only when the active policy and runtime backend permit it. Read-only
children retain its read-only search/fetch evidence path; read-only authority
does not mean "unable to research."

The durable `github`, `automation`, and `rlm` action families are also deferred
by default. `rlm` owns `open`, `eval`, `configure`, and `close` actions for a
persistent sandboxed Python session. Feature-gated native tools may be added to
the active or deferred catalog only when their implementation and host
dependencies are available.

MCP tools are dynamic. Successfully connected servers register names such as
`mcp_<server>_<tool>` from `~/.codewhale/mcp.json`; a failed or disabled server
must not be presented as available. MCP and plugin tools are deferred unless a
user explicitly names them in `[tools].always_load`.

### Conversation toolbox cache

A successful search activation is remembered by name for the current
conversation. The cache holds at most eight deferred names and 16 KiB of
serialized schemas, evicts least-recently-used entries, and revalidates every
entry against the current catalog and policy before advertising it again. A
session sync clears it. The cache cannot resurrect a removed, denied, or
newly-eager tool.

Each subagent gets its own policy-filtered deferred catalog, always-present
`tool_search`, and bounded activation cache. Forked messages and instructions
remain in context, but the child cache starts empty and discovers tools locally;
neither forked context nor a cache can become a discovery allowlist. A child can
still search every tool its own authority permits, including Web search/fetch
for read-only research roles.

## Inspect the model-client request tool payload

Run `/tools` after a model turn to inspect a bounded projection of the exact
tool field in the latest prepared model-client request. `/tools json` emits the
same evidence as bounded machine-readable JSON. Both formats open in a pager;
they are not copied into transcript history. `/tool-studio` remains a human-
command compatibility alias; it is not a model tool.

The snapshot distinguishes an absent tool field from a present empty array. It
reports the exact model-client tool JSON byte count and SHA-256 digest only when
measurement fits the one-MiB inspection bound; larger payloads stay unavailable.
Provider adapters may transform, sanitize, or omit those fields while building
a provider-specific wire body, so `/tools` marks provider delivery and the wire
payload unavailable. Capture and rendering are bounded: retained schemas,
descriptions, caller lists, catalog rows, turn IDs, and payload measurement all
carry explicit truncation, omission, or unavailable receipts. The snapshot stays
in memory only for the current session and is replaced on each prepared request.

Provider, model, approval, registry provenance, and runtime capability metadata
are not fields in the request tool schema. `/tools` therefore reports them as
unavailable instead of joining against mutable state or inferring values. Use
the separate route and permission receipts for those facts.

## Modes and permission postures

Modes and permission postures are separate controls:

- **Plan** keeps the stable primitive vocabulary but centrally refuses shell
  execution and file mutation.
- **Work** is ordinary interactive execution.
- **Operate** uses the same direct-tool authority as Work while preferring Fleet
  workers for independent, parallel, isolated, background, or long-running work.
- **Ask**, **Auto-Review**, and **Full Access** control approval behavior within
  an action-capable mode. They never widen Plan into write or shell access.

See `docs/MODES.md` for the full mode and posture contract.

## Compatibility names

The model-facing contract is the lowercase core above. Saved v0.9.x
transcripts and protocol clients may still call exact hidden compatibility
names such as `File`, `Bash`, and the older single-operation file names. Those
names never enter a new model catalog or `tool_search` result.

Compatibility is execution compatibility, not fuzzy aliasing: an exact legacy
call must reach the handler for its legacy schema. It must not be rewritten
into a small lowercase primitive whose input shape is different. Unknown or
retired names still fail closed instead of guessing a destination.

Specialized native families such as `Git`, `Run`, and `Web` are not aliases for
the lowercase core. They remain real, policy-filtered deferred tools and are
loaded through `tool_search` when needed.

## Long-running work

`bash` runs one cancellable foreground command. It does not carry background,
TTY, wait, interact, or cancel action fields. Stateful process and terminal
control is specialized functionality that must be discovered explicitly; it
does not enlarge the first-turn shell schema.

Use `tasks` when the work itself needs a durable lifecycle, structured gates,
artifacts, replayable timelines, or a stable task id. Large tool results should
remain behind bounded handles or artifacts instead of being copied wholesale
into the parent transcript.

## Parallel fan-out

The sub-agent capacity source of truth is
`crates/tui/src/config/subagent_limits.rs`:

- default configured concurrency: **64**;
- maximum configured concurrency: **128**;
- maximum admitted running-plus-queued work: **1024**.

These are capacity ceilings, not advice to dispatch every available slot. A
manager should use the smallest useful fan-out, preserve a single owner for
fan-in, and verify worker receipts before reporting combined completion.

RLM child-query batching is a different, cheaper cost class. Its
`sub_query_batch` helper accepts 1–16 one-shot children inside a live `rlm`
session; it is not a substitute for tool-carrying `agent` workers.

## Human inspection: `/tools` (`/tool-studio`)

`/tools` renders a **read-only, bounded human projection** of the tool field of
the request that was prepared for one `(turn, step)`. It is not a second
registry and not an execution surface.

**The seam.** The snapshot is built in `crates/tui/src/core/engine/turn_loop.rs`
immediately after `MessageRequest` is constructed, from `request.tools` — the
same value the model client is handed. The engine resolves the surrounding
per-turn data once in `engine.rs` (`ToolSurfaceContext`: flattened registry
facts, the MCP pool's own server attribution, the engine-injected catalog names,
and the resolved model client's receipt) and passes it as plain data, so the
per-step seam never re-locks the MCP pool or holds a tool object.

**Turn and step identity.** The tool set can differ between steps of a turn, so
each snapshot is stamped with turn id and step and each seam emits its own. The
TUI keeps only the latest (`SessionState.last_tool_request_snapshot`). Before
the first seam there is no snapshot and `/tools` says so rather than rebuilding
a registry in the UI.

Two kinds of fact are kept apart:

- **Wire facts** come from the prepared request: name, description, schema,
  `defer_loading` / `strict` / `allowed_callers` / `cache_control`, byte
  accounting, and the catalog digest.
- **Surface facts** come from the `ToolSurfaceContext`: provenance
  (`builtin` / `plugin` / `mcp` / `synthetic` / `unknown`), MCP server identity,
  declared capabilities, declared approval requirement, and model visibility.

Contract:

- **One digest.** `active_tool_catalog_sha256`
  (`crates/tui/src/core/engine/preview.rs`) is the single definition of the
  active-tool-catalog hash. The request manifest publishes it as
  `ToolSurfaceFacts::active_tool_catalog_sha256` and `/tools` reports the same
  value for the same prepared request; neither surface keeps a hash of its own.
- **Nothing is guessed.** MCP server identity is shown only when the real pool
  attributed that exact model tool name. `McpPool::mcp_model_tool_name` is the
  single definition shared by the model catalog and the human attribution, and
  an ambiguous name (two servers colliding on one model name) resolves to no
  server. Synthetic provenance comes from
  `default_synthetic_catalog_tool_names`, which is asserted against the engine's
  own `is_synthetic_catalog_tool` predicate. A transmitted tool with no registry
  entry reports `capabilities: unknown`, never "none".
- **Provider availability follows the resolved client.** It comes from
  `Engine::tool_surface_provider_receipt`, never from "a tool registry exists".
  With no client the receipt is `unavailable` even when the registry is full.
- **Unknown shrinks, it does not vanish.** `unavailable_for_this_request` always
  contains `provider_wire_payload`: nothing on this path observes what the
  provider adapter finally transmits. It additionally contains `provider` and
  `model` without a resolved client, and `provenance` / `capabilities` /
  `approval` when no surface context was captured.
- **Absent stays distinct from empty.** A request with no tools field is not a
  request with an empty tools array; an unresolved field is `unknown` with a
  reason, not a default.
- **Bounded.** Rendering is capped by tool count (32), name, description, schema
  bytes, allowed-caller count, and a payload measurement bound, each with an
  explicit truncation or omission receipt. Registered tools that this request
  does *not* carry are reported as a bounded name list plus an exact count
  rather than expanding the projection.
- **Inert.** The snapshot lives beside the transcript, never in
  `session.messages`, so it cannot enter a model request or perturb the
  provider's prefix cache. It never executes a tool, never reads credentials,
  never reorders the catalog, and is never registered as a model-callable tool.
- **Delivery is never claimed.** The capture happens before connection setup, so
  `delivery_status` stays `unknown`.

## Release verification

Do not infer the public surface from handler function names. Verify the model
catalog and alias visibility at the exact candidate SHA:

```bash
python3 scripts/measure-runtime-contract.py
cargo test -p codewhale-tui --lib --locked core::engine::tests::default_active_contract_keeps_discovery_and_core_tools_eager -- --exact
cargo test -p codewhale-tui --lib --locked tools::file_tool::tests::primitive_schemas_are_separate_and_small_contract_shaped -- --exact
cargo test -p codewhale-tui --lib --locked tools::shell::tests::lowercase_bash_schema_is_small_contract -- --exact
cargo test --locked -p codewhale-tui --lib core::engine::tests::print_mode_tool_catalog_metrics -- --ignored --exact --nocapture
```

Check the test names against the source before trusting a green run: `cargo test`
exits 0 with "0 passed; N filtered out" when a filter matches nothing, so a
misspelled filter is indistinguishable from a pass. (Three filters printed here
before v0.9.4 named tests that did not exist.)

The provider-free receipt must report the seven default-active names listed
above. A separate repository-wide tool count may include deferred, dynamic,
feature-gated, and compatibility-only registrations; it is not the number of
tools placed in the first-turn model catalog.
