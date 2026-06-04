# Skill Invocation Design — the `$<skill-name>` inline syntax

Status: **DESIGN ONLY** (v0.8.53 cycle). No catalog/parser code ships in this
cycle; the implementation target is **0.9.0**. This document describes what
*will* be built and the contracts it must honor against today's code.

Related design docs: `TOOL_LIFECYCLE.md` (tool lifecycle states + per-skill tool
restriction), command-surface taxonomy notes for `/memory`, `/context`,
`/rules`, `/workflow` (`/whaleflow`), `/overlay`. Open PRs on `codex/v0.8.53`:
#2684 (subagent role vocab / lifecycle signals / eval ergonomics) and #2685
(git history active + RLM/field errors). Nothing here contradicts those.

---

## 1. Problem

Skill activation has no single, model-legible entry point, and the candidate
surfaces all compete with each other:

- A `/skill` slash command, a `load_skill`-style tool, plugin/namespace naming
  (`superpowers:systematic-debugging`, `github:gh-fix-ci`), and the long-running
  workflow commands (`/workflow` / `/whaleflow`) all *could* be "the way you
  start a skill." None of them is canonical.
- Slash commands are already overloaded. `/memory`, `/context`, `/rules`,
  `/config`, `/provider`, `/workflow`, `/overlay` each map to one subsystem;
  jamming skill invocation into `/`-space forces a weaker model to disambiguate
  "is this a command or a skill?" on every keystroke.
- Weaker / smaller models (the cheaper providers CodeWhale targets) do not
  reliably pick the right mechanism. They will free-text "let me use systematic
  debugging" instead of actually loading the skill body, so the guidance never
  enters the context window.
- Today there is **no parser that activates an inline skill mention on submit.**
  `slash_menu.rs:86` (`partial_inline_skill_mention_at_cursor`) recognizes an
  inline `/<skill>` token *under the cursor for popup purposes only*; the submit
  path in `ui.rs:4721` (`build_queued_message`) does not resolve or activate any
  inline mention. There is also no activation-mode concept (always-on / glob /
  model-decision / manual) and skills cannot restrict tools yet.

We need one prefix that means exactly "invoke this skill," is visually distinct
from commands, and is cheap for a small model to emit correctly.

---

## 2. Proposal

Adopt **`$` as the skill-invocation prefix**, where **the token *is* the skill
name** — not a literal command called `$skill`.

```
$systematic-debugging figure out why MiMo auth fails
$test-driven-development add coverage before fixing
$github:gh-fix-ci inspect the failing checks
$aleph search the planning doc
```

The leading `$` is the marker; everything from `$` up to the next whitespace is
the **skill id**. The rest of the line is the user's request, passed through to
the model with the skill body already loaded as active guidance.

This is deliberately a *reference / macro* sigil, like a shell variable
expansion or an `@mention`: `$skill-id` resolves to "the contents and tool
policy of that skill," then the surrounding prose is the task.

`$` works in three places (see §4): the user composer, the command-palette
input, and **model-facing planning text** — so the model itself can write
`$systematic-debugging` in its plan and have it resolve.

---

## 3. Resolution rules

Given a token `$<id>` (id captured up to the next whitespace):

1. **Exact name first.** Look the id up directly:
   `discover_in_workspace(workspace).get(id)` — `skills/mod.rs:553` builds the
   registry; `SkillRegistry::get` (`skills/mod.rs:421`) matches on `s.name == id`
   exactly. Skill names come from frontmatter `name:` (or the first `# Heading`
   fallback) parsed at `skills/mod.rs:382-417`. An exact hit wins unconditionally.

2. **Namespaced `$ns:skill`.** If the id contains a `:`, treat the part before
   the colon as a source/plugin namespace and the part after as the skill name:
   `$github:gh-fix-ci`, `$superpowers:systematic-debugging`. Namespaced ids are
   the disambiguation handle a user is told to type when a bare id is ambiguous.
   (Glob/wildcard namespacing — `$github:*` — is explicitly deferred, see §6.)

3. **Fuzzy match *suggests*, never silently chooses.** If there is no exact (or
   namespaced-exact) hit, run a case-insensitive substring / prefix match over
   `SkillRegistry::list()` (`skills/mod.rs:426`). If exactly one skill matches,
   surface it as a suggestion ("did you mean `$systematic-debugging`?") but do
   **not** auto-activate it. If more than one matches, list them and require the
   user/model to re-issue with a disambiguated id (§7). Ambiguity never resolves
   to a silent pick.

4. **Respect enable-state.** A resolved skill is only activated if
   `SkillStateStore::is_enabled(id)` is true (`skill_state.rs:73`:
   `!self.disabled.contains(skill_name)`). A disabled skill that resolves by
   name produces a clear "skill is disabled; enable it with `/skill enable <id>`"
   message rather than silently activating or silently doing nothing.

Resolution order is therefore: **exact → namespaced-exact → enabled-check →
fuzzy-suggest (never auto-pick).**

---

## 4. Behavior

When a `$<id>` mention resolves and is enabled:

- **Visible activation line.** The transcript shows `Using skill: <name>` so the
  user can see which skill body entered context. (Mirrors the existing skill UX
  vocabulary; one line per activated skill.)
- **Body loaded as active guidance.** The skill's `body`
  (`skills/mod.rs` `Skill.body`) is injected into the turn as authoritative
  guidance, the same content a `/skill`-style activation would load. The user's
  trailing prose is the task the guidance applies to.
- **Tool-surface narrowing (when declared).** If the skill declares a set of
  allowed tools, the active tool surface narrows to that set for the duration of
  the skill's influence. **Per-skill tool restriction is net-new** — skills
  cannot restrict tools today; the mechanism, and how narrowing interacts with
  the catalog-head byte-stability invariant (`tool_catalog.rs:169-196`), is
  specified in `TOOL_LIFECYCLE.md`. Until that lands, a declared tool list is
  parsed and shown but not enforced.
- **Multiple `$mentions` compose explicitly, or prompt.** Until formal
  composition rules exist, two or more `$mentions` in one message either compose
  only when the rule is unambiguous (e.g. one guidance skill + one tool-scoping
  skill) or return a **"choose one"** prompt listing the mentioned skills. We
  never silently activate multiple complex skills at once (see §7 and Non-goals).
- **Three input surfaces.** Resolution runs for: (a) user prompts in the
  composer, (b) command-palette input, and (c) model-facing planning text, so a
  model that writes `$test-driven-development` in its plan triggers the same
  activation path a human would.
- **Slash commands remain supported.** `/skill ...` and the rest of the slash
  surface keep working unchanged. `$` is the *preferred* path for models because
  it is one token and unambiguous, but it is additive, not a replacement (§7
  Non-goals).

---

## 5. Why `$`

- **Visually distinct from `/commands`.** A glance separates "run a subsystem
  command" (`/memory`, `/context`, `/workflow`) from "load a skill" (`$aleph`).
  Weaker models stop confusing the two surfaces.
- **Reads like a reference / macro.** `$name` already means "expand this named
  thing" to anyone who has touched a shell or a templating language. Skill
  invocation *is* an expansion: `$skill-id` → that skill's guidance + tool policy.
- **Avoids overloading the slash namespace.** `/workflow`, `/memory`, `/config`,
  `/provider`, `/rules`, `/overlay`, `/context` each already own one meaning in
  the command-surface taxonomy. Skills get their own sigil instead of a crowded
  `/skill <name>` subcommand competing with all of them.
- **Easy to type and remember.** Single leading character, then the literal
  skill name. Nothing to memorize beyond the skill ids the user already sees in
  `/skill list`.

---

## 6. Implementation plan (smallest viable 0.8.53-ready slice → 0.9.0)

The 0.8.53 cycle is **docs only**. The plan below is the build order once code
is unblocked; the first slice is intentionally the minimum that proves the path.

**Slice 1 — token scanner at submit (the minimum viable feature).**
- Add a `$<skill-id>` token scanner invoked on submit, **before**
  `build_queued_message` runs (`ui.rs:4721`). The scanner finds leading-`$`
  tokens, captures the id up to the next whitespace, and hands each id to the
  resolver. The scanner must skip `$` occurrences inside code fences and inline
  command strings (see Non-goals) so shell `$VAR` references are never treated as
  skill mentions.
- Resolve via `discover_in_workspace(workspace).get(id)` (`skills/mod.rs:553` /
  `:421`), gate on `SkillStateStore::is_enabled` (`skill_state.rs:73`), and emit
  the `Using skill: <name>` line plus the loaded body.

**Slice 2 — inline-mention popup.**
- Extend the inline-mention popup machinery in `slash_menu.rs:86`
  (`partial_inline_skill_mention_at_cursor`) to recognize a `$`-prefixed token
  under the cursor and offer skill-name completions from `SkillRegistry::list()`,
  the same way the slash popup offers commands. This is a UX accelerator on top
  of Slice 1, not a precondition for it.

**Slice 3 — ambiguity diagnostics.**
- When resolution is ambiguous, emit actionable diagnostics, e.g.
  `"$debugging matched 3 skills: systematic-debugging, root-cause-debugging,
   superpowers:systematic-debugging — use $superpowers:systematic-debugging"`.
  Diagnostics name the disambiguated id the user should type next.

**Deferred to 0.9.0+ (explicitly out of the first slices):**
- `$ns:skill` **globs / wildcards** (`$github:*`). Plain namespaced-exact
  (`$github:gh-fix-ci`) ships in Slice 1; globbing does not.
- **Per-skill tool restriction enforcement.** Parsing/display can land early;
  enforcement and its catalog-head-stability handling are owned by
  `TOOL_LIFECYCLE.md`.
- **Multi-skill composition rules.** Until defined, fall back to the "choose one"
  prompt (§4, §7).

---

## 7. Ambiguity / error UX, tests, and non-goals

### Error / ambiguity UX examples

| Input | Outcome |
|---|---|
| `$systematic-debugging fix the auth bug` | Exact hit. `Using skill: systematic-debugging`, body loaded, task = "fix the auth bug". |
| `$github:gh-fix-ci inspect failing checks` | Namespaced-exact hit. `Using skill: github:gh-fix-ci`, body loaded. |
| `$nope do a thing` | No match. `"No skill named 'nope'. Run /skill list to see available skills."` No activation; the line is sent as ordinary text. |
| `$debugging ...` (3 candidates) | `"$debugging matched 3 skills: systematic-debugging, root-cause-debugging, superpowers:systematic-debugging — use $superpowers:systematic-debugging."` No auto-pick. |
| `$systematic-debug ...` (1 fuzzy candidate) | Suggest only: `"No exact skill 'systematic-debug'. Did you mean $systematic-debugging?"` No silent activation. |
| `$aleph ...` but aleph disabled | `"Skill 'aleph' is disabled. Enable it with /skill enable aleph."` No activation. |
| `$tdd $systematic-debugging ...` (2 mentions) | `"Choose one skill to lead this turn: $test-driven-development or $systematic-debugging."` (until composition rules exist). |
| `echo $PATH` inside a code fence / command string | Not a mention. Scanner skips `$` inside code/command contexts. |

### Tests (planned)

- **Exact:** `$systematic-debugging` resolves via `get(id)`, activates, loads body.
- **Namespaced:** `$github:gh-fix-ci` resolves on the `ns:skill` form.
- **Missing:** `$nope` → no-match message, no activation, line passed as text.
- **Ambiguous:** `$debugging` (≥2 candidates) → "matched N skills … use $ns:skill",
  asserts **no** auto-activation occurred.
- **Disabled:** a skill with `is_enabled == false` → disabled message, no activation.
- **Guardrail — `$` in code:** `$VAR` inside a fenced block or command string is
  not treated as a mention.

### Non-goals

- **Do not remove slash commands.** `/skill` and the whole `/` surface stay; `$`
  is preferred for models but additive.
- **Do not auto-run arbitrary scripts.** A `$mention` loads guidance (and, later,
  a declared tool policy) — it never executes shell or skill-attached scripts on
  its own.
- **Do not silently activate multiple complex skills.** Multi-mention falls back
  to a "choose one" prompt until composition rules are specified.
- **Do not let `$` collide with shell variables.** `$` inside code fences and
  command strings is never parsed as a skill mention.
