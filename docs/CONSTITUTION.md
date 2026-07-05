# The Constitution

CodeWhale ranks its instructions instead of flattening them into one prompt. As
a project evolves, instructions pile up and conflict: the original spec, a later
refactor that contradicts it, stale memory, a previous agent's handoff, your
current request, and fresh test output that doesn't match what the handoff
claimed. A flat system prompt makes the model resolve that by guess. CodeWhale
uses a **nested constitution** so there's a defined rank instead of vibes.

One boundary up front: the constitution is **guidance for model behavior, not a
runtime security gate**. Managing any constitution layer never changes approval
policy, sandbox, shell, network, trust, default mode, or MCP authority — those
are enforced in code and configured separately (see
[docs/SANDBOX.md](SANDBOX.md) and [docs/MODES.md](MODES.md)). The single
exception is the repo-law invariant mechanism described
[below](#repo-local-law-codewhaleconstitutionjson), which *is* mechanically
enforced — and it can only tighten, never loosen.

## The layers

The system prompt is layered, most-static first, and the order is enforced in
code (tests assert it can't drift):

1. **Bundled global Constitution** — the base law, compiled into every binary
   from [`crates/tui/src/prompts/constitution.md`](../crates/tui/src/prompts/constitution.md).
2. **User-global constitution** — your standing guidance, managed with
   `/constitution` or `/setup`, stored as structured data at
   `$CODEWHALE_HOME/constitution.json` (default `~/.codewhale/constitution.json`).
3. **Repo-local constitution** — project authority policy in
   `.codewhale/constitution.json`, committed to the repo like `.github/`.
4. **Project instructions** — `AGENTS.md` (with `CLAUDE.md` and
   `.claude/instructions.md` as compatibility fallbacks).
5. **Memory and handoffs** — recalled state, lower authority than everything
   above.

Your current request and live tool evidence still control the active turn. When
two instructions conflict, each yields to the higher layer; at equal rank the
more specific governs, then the more recent. Because the law lives in the
harness, not the model, swapping models keeps the structure intact.

Release verification for these surfaces lives in
[`docs/evidence/v0867-constitution-setup-qa-matrix.md`](evidence/v0867-constitution-setup-qa-matrix.md).

## The bundled law

The compiled-in Constitution
([`crates/tui/src/prompts/constitution.md`](../crates/tui/src/prompts/constitution.md))
opens with eight articles — the durable judgment frame:

- **I. Ground Truth** — report what tools return, never what memory suggests.
  The operator may order the agent past a fact; no one may invent one.
- **II. Verification** — no completion claim without checking. Run the work's
  own real check, not an invented stand-in.
- **III. Momentum** — parallelize independent work; a turn that ends with a
  promise is a turn that could have shipped.
- **IV. Legacy** — less is enough until evidence says otherwise; prefer
  deletion, repair, and existing capability over new code. Judgment names a
  duty; mechanism (code, tests, tool gates) carries it.
- **V. Help** — when blocked, ask; asking is fidelity to the work.
- **VI. Priority** — the conflict-resolution order itself, fixed as law.
- **VII. Domain Context** — keep CodeWhale's standards without forcing
  terminal-coding habits onto non-coding domains.
- **VIII. Inquiry** — a failed prediction switches you from building to
  investigating; know which one you're doing.

Below the articles sit lower tiers — **Statutes** and **Regulations** — that
carry the operational rules: language mirroring, terminal output formatting,
the verification protocol, execution discipline, orchestration and sub-agent
strategy, and context management. The tiering matters: higher tiers win when
rules collide.

## Your constitution: `/setup` and `/constitution`

On first launch CodeWhale runs a **constitution-first** setup path: language →
provider/model readiness → runtime posture → create or confirm your
constitution. The bundled law is always valid, so you can defer; reopen the hub
any time with `/setup`.

On the Constitution step:

- **`1`–`6`** tune the guided draft; **`G`** previews it, and **`G`** again
  ratifies and saves a fresh structured `constitution.json`.
- **`T`** opens a typed custom-guidance field: write your own global
  behavioral guidance in plain text (typing, paste, Backspace all work;
  bounded at 2,000 characters). The text is sanitized and saved into the
  constitution as advisory guidance — like everything here, it steers model
  behavior and is never a runtime approval, sandbox, or security gate. Any
  edit invalidates a pending preview, so `G` must preview again before
  ratifying.
- **`A`** (when a provider is configured) asks your first configured model to
  draft it. Drafting is not saving — the draft renders through the same preview
  and you still press **`G`** to ratify; your typed custom guidance is carried
  verbatim and cannot be authored by the model.
- **`K`** keeps your existing loaded constitution (shown only when a valid file
  is present).
- **`U`** (or `/constitution bundled`) records the bundled/default law, and
  **`D`** defers the choice entirely.

`/constitution` (alias `/law`) is the management surface afterward, with
subcommands `status` (default), `preview`, `review`, `repo`, `explain`,
`edit`/`guided`, `repair`, `posture`, and `bundled`. It is guided setup output
saved as structured data and rendered into a separate
`<codewhale_user_constitution>` prose block — not a raw prompt editor. Full
reference: [docs/CONFIGURATION.md](CONFIGURATION.md#constitution-project-instructions-and-repo-authority).

## Repo-local law: `.codewhale/constitution.json`

A repo can carry two complementary files: `AGENTS.md` for ordinary working
instructions, and `.codewhale/constitution.json` for **authority policy** —
when local sources conflict, what CodeWhale should trust first, and what to
verify before claiming done. All fields are optional:

```json
{
  "schema_version": 1,
  "authority": ["current user request", "live code and tests", "AGENTS.md"],
  "protected_invariants": ["do not break old-session transcript replay"],
  "branch_policy": "PRs target the integration branch, not main",
  "verification_policy": {
    "before_claiming_done": ["run focused tests", "read changed files back"]
  },
  "escalate_when": ["a destructive action was not explicitly authorized"]
}
```

### Enforced invariants — where law becomes mechanism

A plain-string `protected_invariants` entry is advisory prose. An entry written
as an **object with `paths` globs** additionally compiles into a mechanical
write hold in the engine's tool gate
([`crates/tui/src/repo_law.rs`](../crates/tui/src/repo_law.rs)):

```json
{
  "text": "The wire format is frozen; protocol changes need a human.",
  "paths": ["crates/protocol/**"],
  "action": "block"
}
```

- `action: "ask"` (the default) **force-prompts** for approval in every mode,
  including YOLO. `action: "block"` **denies the write outright**.
- **Tighten-only.** The schema has no allow/widen shape, so law can only add
  holds — a crafted constitution can never grant authority or weaken a gate.
- **Fails safe.** A missing file, parse error, or bad glob degrades to fewer or
  zero rules, never a poisoned gate. Across matches the strongest action wins.
- **Leaves a receipt.** Every hold emits a `tool.repo_law_decision` audit event
  naming the invariant, the matched path, and the source file.
- **Coverage is deliberately limited.** Holds are evaluated for the write tools
  (`write_file`, `edit_file`, `apply_patch`, `fim_edit` — the `WRITE_TOOLS`
  list in `repo_law.rs`) against the filesystem targets named in their inputs.
  A shell command that writes a protected path is governed by the ordinary
  approval/sandbox/shell-write gates, not by repo law.
- **Repo-local only.** Only the repo's `.codewhale/constitution.json`
  participates; the user-global constitution stays advisory prose.

Details and semantics:
[docs/CONFIGURATION.md § Enforced repo-law invariants](CONFIGURATION.md#enforced-repo-law-invariants).

## Expert override

The bundled base prompt can be replaced per-user without rebuilding — an expert
escape hatch for repurposing the TUI beyond software engineering, not the
normal guided path. It takes two deliberate steps: drop the replacement at
`~/.codewhale/prompts/constitution.md` *and* set
`CODEWHALE_ALLOW_BASE_PROMPT_OVERRIDE=1`. File without flag is ignored. Only
the byte-stable base prompt segment is overridable; mode deltas, approval
policy, the tool taxonomy, and context management stay owned by the runtime, so
an override cannot remove safety-relevant guidance. See
[docs/CONFIGURATION.md § Expert full base-prompt override](CONFIGURATION.md#expert-full-base-prompt-override-3638).
