# TUI deconstruction

Stop paying the monolith tax by **consolidating then extracting**, never the reverse. This is the playbook. It is not permission to open a crate per file.

## Invariants (mechanical, every PR)

- Runtime-contract receipt (`python3 scripts/measure-runtime-contract.py` / `check-runtime-contract-budget.py`) is **byte-identical** before and after. That is the KV-cache prefix made checkable.
- `crates/core/tests/single_turn_loop.rs` stays green. There is one turn loop: `Engine::run_turn`. Do not add a second.
- `BASE_PROMPT` in `crates/tui/src/prompts/text.rs` is the sole base prompt. Tool catalog order is a cache-prefix fact; do not shuffle it as a drive-by.
- Dead-code / file-size / persistence budgets may go **down**, never up, unless the PR names why.
- Clippy/fmt + targeted tests via `scripts/dev-test.sh <area> [filter]`. Do not `cargo test --workspace` for a single-area edit.
- Never merge `#5576` or `#5628`. Never add `target-*` at the repo root.

## Anti-goal

No micro-crates. No "extract because the file is large." A new crate exists only when it has **more than one production consumer** already, or when the extraction is the last step of a finished consolidation.

## Target topology

| Lives in | Owns |
| --- | --- |
| `crates/tui` | UI, slash commands, process entry. Target: **&lt;150K lines**. |
| `crates/mcp`, `crates/tools`, `crates/state` | Grow in place. One MCP client (the rmcp stack). |
| **new** `codewhale-models` | After catalog/config consolidation (C3): client, one catalog, pricing, credentials, routing. |
| **Decision A** | Thread store + HTTP automation either becomes `codewhale-runtime` **or** folds into `crates/app-server`. Pick one; do not ship both. |
| `codewhale-engine` | Extracted **last**. Today the engine still lives in `tui/src/core`. |

`crates/core` already owns request construction, bounded fragments, and thread/session types. It does not run turns. Do not rename it as a substitute for extracting the engine.

## Sequencing

### Phase 0 — preconditions (do these first; they are the audit's C1–C4)

Never extract a crate before these finish. Extraction-before-consolidation relocates the mess.

1. **C1 MCP unification** — one client. Move the rmcp stack down, delete the hand-rolled stdio client.
2. **C2 Config mirror deletion** — one schema crate owns `config.toml`. TUI's `Config` becomes a resolved view. One struct pair per PR.
3. **C3 Model-facts unification** — config crate catalog is the single source. Delete the seeded `model_registry` table and its drift-guard test. Prices become data.
4. **C4 Test-giant migration** — move the six `>10K` `tests.rs` files out via the existing `#[path = "tests/..."]` pattern. Test count identical before/after.

Off-ramp: stop after Phase 0 if that is all 0.9.12 can hold. That is a legitimate ship.

### Phase 1 — dismember `lib.rs` intra-crate

Stay inside `crates/tui`. Follow `scripts/command-migration-topology.json`. Order: `cli_args` → `doctor` → subcommands → tests out.

Gate: `crates/tui/src/lib.rs` **&lt; 2,000 lines**.

### Phase 2 — leaf extractions, fewest-dependents first

Always **two PRs per extraction**:

1. Pure `git mv` + re-export shims. Zero logic edits. Receipt identical.
2. Repoint consumers, delete shims, `cargo machete`. Shims get a removal issue at merge.

Never mix a move with an edit.

### Phase 3 — engine last

Extract `Engine` / `run_turn` only after C4 (suite builds fast) and after the leaves are gone. Freeze behavior with the existing runtime-contract receipt. Introduce `TurnLoopState` as a field grouping, not a second loop.

Off-ramp after "2e" (leaves extracted, engine still in tui) is allowed.

## Contributor loop (already exists — do not add a second script)

```sh
./scripts/dev-test.sh config
./scripts/dev-test.sh tui session_metrics::
./scripts/dev-test.sh tui-integration
./scripts/dev-test.sh crates/tui/src/elapsed.rs
```

- Incremental by default. Isolated build-dir via `scripts/dev-cache.sh`.
- Prints the exact `cargo` / `nextest` command (`+ cargo …`).
- `CODEWHALE_DEV_NEXTEST=0` forces libtest. There is no `CARGO_INCREMENTAL=0` requirement for ordinary targeted work.
- `--lib` does not cover `crates/tui/tests/`. Use `tui-integration` / `tui-cucumber`.
- Full CI remains the release gate. Local `tui` is `--lib` on purpose.

If a `tui full` alias is needed, add it to **this** script, not a new one.

## Providers (OMP / OpenCode, not a new enum)

Hosted OpenAI Chat Completions backends (Baseten, Groq, Cerebras, SenseNova) are **data rows** in `crates/config/src/provider_templates.rs` (`ProviderSetupApply::Compatible`). They persist as `[providers.<id>] kind = "openai-compatible"`. They do **not** get a `ProviderKind` / `ApiProvider` variant.

Add a new hosted Chat Completions host by appending one template (id, URL, env, default model, docs). Enum variants stay for distinct **wires**: Anthropic Messages, Codex Responses, Google thought signatures, OAuth-only import.

OMP does the same: one catalog descriptor + one auth file. OpenCode does models.dev + named `provider` config + plugins. Neither adds a 15-arm match.

## Recipe reminder

Add a layer only when the PR **names or deletes** the layer it replaces. Before adding `model_*`, `*_config`, `provider_*`, or anything that "bridges" / "mirrors" / "stages", grep the existing thing and edit it.
