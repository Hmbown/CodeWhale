# Skills Manager

Skills are reusable `SKILL.md` instruction packs. Codewhale discovers them from
several roots, but **only CodeWhale-owned directories are writable**. The unified
`/skills` manager is the interactive surface for audit and mutation; slash
aliases share the same write path.

For Claude Code plugin boundaries, see [CLAUDE_PLUGIN_COMPAT.md](CLAUDE_PLUGIN_COMPAT.md).
For `skills_dir` and `[skills]` config keys, see [CONFIGURATION.md](CONFIGURATION.md).

## Architecture (four layers)

| Layer | Role |
| --- | --- |
| **Root catalog** | Single source of precedence and ownership (`SkillRootCatalog`). |
| **Audit** | Read-only, unmerged on-disk inventory (status, digest, actions). |
| **Mutation controller** | Only writer for install / import / update / remove / trust. |
| **Skills manager view** | TUI: emits events only; never writes files itself. |

Runtime discovery (`SkillRegistry`) still merges skills for the model. Audit
intentionally does **not** merge — it shows every on-disk copy so conflicts and
shadowing stay visible.

## Ownership and roots

**Writable (CodeWhale-owned)**

| Scope | Path |
| --- | --- |
| Project | `<workspace>/.codewhale/skills/` |
| Global | `~/.codewhale/skills/` |

**Read-only compatible** (discover / import source only — never mutated in place)

Examples: `<workspace>/.agents/skills`, `./skills`, `.claude/skills`,
`.cursor/skills`, `.opencode/skills`, `~/.agents/skills`, `~/.claude/skills`,
and similar harness layouts.

**Audit-only (not runtime-active)**

- `.codex/skills` appears in **compatible** audit scans so operators can see it.
  It does **not** join the runtime discovery set.

Configured `skills_dir` that is not one of the owned CodeWhale roots stays
read-only. Discovery and the manager can list it; mutations still target owned
project/global roots only.

## Slash commands

| Command | Behavior |
| --- | --- |
| `/skills` | Opens the Skills Manager (owned-only scan, **no network**). |
| `/skills <prefix>` | Text list filtered by name prefix. |
| `/skills inspect` | Text discovery mode, searched directories, and source paths. |
| `/skills --remote` | Explicit registry listing (network). |
| `/skills sync` | Explicit registry → local cache sync (network). |
| `/skill <name>` | Activate a skill for the next turn. |
| `/skill install [--project\|--global] <spec>` | Install via mutation controller. |
| `/skill update [--project\|--global] <name>` | Update a managed skill from its registry provenance. |
| `/skill uninstall [--project\|--global] <name>` | Remove a managed skill. |
| `/skill trust [--project\|--global] <name>` | Write digest-bound advisory trust. |

Notes:

- There is **no** `/skills audit` subcommand. Use the manager (and `c` to toggle
  compatible roots) or `/skills inspect` for discovery details.
- Bare `/skill install <spec>` (no scope flag) installs into the CodeWhale
  **global** owned root.
- If the same name exists in both project and global owned roots, update /
  uninstall / trust require `--project` or `--global`.
- If a name exists only under a compatible external root, writes are refused;
  import it through `/skills` instead of editing harness directories.

## Skills Manager (TUI)

Default open path: type `/skills` and confirm. The surface is zero-network on
open (owned-only audit).

| Key | Action |
| --- | --- |
| `↑`/`↓` or `j`/`k` | Move selection |
| `Enter` | Primary available action / confirm pending prompt |
| `i` | Import (external → owned) |
| `u` | Update (managed + registry provenance) |
| `r` | Remove (managed; confirms first) |
| `t` | Trust (managed; digest-bound) |
| `s` | Toggle import target: project ↔ global |
| `c` | Toggle scan: owned-only ↔ compatible (still local disk only) |
| `Esc` | Cancel confirm, or close the manager |

The view never calls install helpers or touches the filesystem. It emits a
mutation request; the host runs the controller, shows a receipt, and rebuilds
the inventory.

## Bundled catalog tiers

Codewhale presents its shipped skills in two compact tiers so agentic workflows
are not buried under document and integration helpers:

- **Core agentic** — planning, implementation, debugging, review, verification,
  delegation, Fleet, release, and `best-of-n` comparison workflows.
- **Format & tooling** — document formats, data visualization, frontend and web
  testing, and skill/plugin/MCP authoring helpers.

Workspace, user, and compatible-harness skills stay labeled **custom**; Codewhale
does not guess their intent from their name. The shipped pack also does not
advertise capabilities the runtime lacks. In particular, image understanding
is available, but an image-generation skill is not bundled until a real
image-generation tool exists.

### Invocation and alias metadata

Bundled and user skills may declare two runtime-routing fields in frontmatter:

| Field | Meaning |
| --- | --- |
| `invocation: model+user` | The default; the skill appears in the model's compact catalogue and can be loaded by the model or user. |
| `invocation: explicit-only` | The skill remains loadable by an explicit name, but is omitted from the model catalogue so opt-in instructions do not become ambient context. |
| `aliases-for: name, other-name` | Additional lookup names for the same canonical skill. Aliases are not separate catalogue entries and do not duplicate prompt content. |

Missing or unknown invocation values retain the historical `model+user`
behavior. Canonical names win over aliases when a collision exists. Loading a
skill reports its canonical invocation and aliases so receipts remain
inspectable.

### Starter-pack parity decisions

The v0.9.2 parity audit in [#4698](https://github.com/Hmbown/CodeWhale/issues/4698)
compared the five `xai-grok-memory` / `xai-grok-shell` reference skills with
the actual Codewhale bundle. This is a decision matrix, not a request to copy
reference text or advertise unsupported tools:

| Reference skill | Codewhale decision | Runtime grounding |
| --- | --- | --- |
| `check-work` | Canonical alias/compatibility mapping to `verify` | `verify` is the shipped evidence-collection workflow. |
| `code-review` | Canonical alias/compatibility mapping to `review` | `review` is the shipped read-only correctness workflow. |
| `create-skill` | Canonical alias/compatibility mapping to `skill-creator` | `skill-creator` is the shipped authoring workflow. |
| `help` | Do not add a giant ambient skill; resolve help from installed docs/config on request | `read_file`, `/help`, and the documented skill manager are the existing surfaces. |
| `imagine` | Intentionally out of scope | Codewhale has no image-generation/edit tool, so the starter pack must not advertise one. |

No reference skill body is copied by this compatibility slice. The explicit
aliases and invocation metadata are bounded routing facts; the full skill body
still enters context only through `load_skill`.

## Audit statuses

Each audited row carries precedence and relationship flags:

| Status | Meaning |
| --- | --- |
| **Active** | Highest-precedence copy for that canonical name in the scan. |
| **Shadowed** | Same name exists at a higher-precedence root. |
| **Duplicate** | Same canonical name and same package digest as another copy. |
| **Conflict** | Same canonical name, different package digest. |

External skills with no owned peer (and a valid digest) are **import
candidates**. Externals that conflict with or exactly duplicate an owned copy
can still offer Import — duplicate → already present; conflict → confirm replace
in the selected import scope.

## Provenance and markers

Managed installs write schema **v2** metadata under the skill directory:

**`.installed-from` (v2)** — written last on successful install/import:

```json
{
  "schema_version": 2,
  "spec": "github:owner/repo",
  "url": "https://…",
  "source_checksum": "…",
  "content_digest": "…",
  "installed_name": "my-skill",
  "registry_version": null
}
```

- `content_digest` is a bounded package tree hash (not SKILL.md alone).
- Display of URLs strips userinfo, query, and fragment.
- Imports use a local `import:…` provenance and **cannot** be updated from a
  registry; re-import or remove them instead.
- Legacy v1 markers are recognized as managed with
  `LegacyMetadataUnknown` integrity until refreshed.

**`.trusted` (v2)** — advisory, digest-bound:

```json
{
  "schema_version": 2,
  "content_digest": "…"
}
```

Trust records review intent. It does **not** sandbox the skill or auto-approve
tools. Content updates clear trust so a stale marker cannot outlive the bytes.

Manual skills (owned root, no managed marker) are visible but not
update/remove/trust through the managed actions.

## Package digest and safety

Audit and mutation share a bounded package digest:

- Regular files only; symlinks that escape the skill root or cycle → fail closed.
- Caps on total size, file count, and depth.
- Mutations re-check an expected digest before write (TOCTOU).
- Import/replace keeps a `.bak` until digest + marker finalize succeed; failure
  restores the previous owned package.

## Readiness

The audit model has a readiness field and optional provider hook for a future
readiness cache ([#4407](https://github.com/Hmbown/CodeWhale/issues/4407)).
Today, when no cache is wired, readiness is always **`Unknown`**. The manager
does not run readiness probes and does not block mutations on readiness.

## Config knobs

```toml
# Optional override for discovery preference (not automatically a write target
# unless it is the CodeWhale project/global owned path).
skills_dir = "/path/to/skills"

[skills]
# When true, runtime discovery skips cross-tool roots (.claude, .agents, …).
# Owned CodeWhale roots and an explicit skills_dir override still apply.
scan_codewhale_only = false

# Optional registry / install size overrides used by --remote, sync, and install.
# registry_url = "https://…"
# max_install_size_bytes = 5242880
```

See [CONFIGURATION.md](CONFIGURATION.md) for the full config surface.

## Operator checklist

1. Prefer `/skills` for day-to-day management; keep `--remote` / `sync` explicit.
2. Never hand-edit `.claude` / `.agents` / `.cursor` trees to “install” for
   Codewhale — import into `.codewhale/skills` instead.
3. Treat `.trusted` as advisory documentation of review, not a security boundary.
4. After registry updates that change content, re-trust if you still want the
   advisory marker.
5. Dual project+global copies of the same name need an explicit scope flag on
   CLI mutations.
