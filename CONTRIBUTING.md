# Contributing to codewhale

Thank you for your interest in contributing to codewhale! This document provides guidelines and instructions for contributing.

## Getting Started

### Prerequisites

- Rust 1.88 or later (edition 2024)
- Cargo package manager
- Git

### Setting Up Development Environment

1. Fork and clone the repository:
   ```bash
   git clone https://github.com/YOUR_USERNAME/CodeWhale.git
   cd CodeWhale
   ```

2. Build the project:
   ```bash
   cargo build
   ```

3. Run tests:
   ```bash
   cargo test --workspace --all-features
   ```

4. Run with development settings:
   ```bash
   cargo run --bin codewhale
   ```

## Development Workflow

### Code Style

- Run `cargo fmt` before committing to ensure consistent formatting
- Run `cargo clippy` and address all warnings
- Follow Rust naming conventions (snake_case for functions/variables, CamelCase for types)
- Add documentation comments for public APIs

### Testing

- Write tests for new functionality
- Ensure all existing tests pass: `cargo test --workspace --all-features`
- Colocate unit tests beside the code they cover (standard Rust `#[cfg(test)]`
  modules), and add integration tests under the owning crate's `tests/`
  directory (for example `crates/tui/tests/` or `crates/state/tests/`). The
  repository root `tests/` directory is not used

### Pre-push verification

Run these before every push. They match what CI enforces on pull
requests, so passing locally means the PR lanes should pass too:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-features --locked -- \
  -D warnings \
  -A clippy::uninlined_format_args \
  -A clippy::too_many_arguments \
  -A clippy::unnecessary_map_or \
  -A clippy::collapsible_if \
  -A clippy::assertions_on_constants
cargo test --workspace --all-features --locked
```

The release lane runs a stricter clippy that also lints test, bench, and
example targets. The PR template checklist asks for this form, and it is
the right command before requesting review or doing release-bound work,
because `--all-features` alone skips lints that will fail the release
lane later:

```bash
cargo clippy --workspace --all-targets --all-features --locked -- \
  -D warnings \
  -A clippy::uninlined_format_args \
  -A clippy::too_many_arguments \
  -A clippy::unnecessary_map_or \
  -A clippy::collapsible_if \
  -A clippy::assertions_on_constants
```

#### Fast local loop

The full gate above is what CI enforces, but you do not need it for every
edit. `crates/tui` is a ~750k-line crate, so the loop that stays fast is
the one that avoids rebuilding it more than necessary (numbers and the
reasoning are in [`docs/BUILD_PERFORMANCE.md`](docs/BUILD_PERFORMANCE.md)):

```bash
# 1. Type-check first (seconds after the first build; no codegen, no link).
scripts/dev-cargo.sh check -p codewhale-tui

# 2. Run only the tests near your change (one crate, one filter).
scripts/dev-test.sh tui fleet_setup
# or: scripts/dev-test.sh crates/tui/src/elapsed.rs

# 3. Run a whole crate's unit suite. scripts/dev-test.sh uses nextest when
#    it is installed (one process per test, all cores busy, slow tests
#    named; ~100 s here vs ~270 s with libtest).
cargo install cargo-nextest --locked      # once
scripts/dev-test.sh tui
scripts/dev-cargo.sh nextest run --workspace --all-features --locked

# 4. Before pushing, run the authoritative gate exactly as CI does:
cargo test --workspace --all-features --locked
```

`.config/nextest.toml` already serializes the PTY suite and bounds the
integration tests that spawn the real binary, so `cargo nextest run` is
safe to use on the whole workspace (nextest does not run doctests; the
authoritative `cargo test` gate does). Tests must not depend on running in
the same process as another test (nextest gives every test its own
process); if a test needs the rustls crypto provider, install it in that
test as production does at startup.

On a machine with less than 16 GB of RAM (or when cross-compiling, e.g.
for OHOS), build one rustc at a time: `CARGO_BUILD_JOBS=1` (or `-j1`), one
crate at a time, `--lib` for tests, never `--workspace`/`--all-targets`.
The tui library needs ~6 GB for its own rustc and its unit-test build ~8 GB;
`cargo test --workspace` runs both at once. Numbers and the full recipe:
[`docs/BUILD_PERFORMANCE.md`](docs/BUILD_PERFORMANCE.md#low-memory-build-recipe-machines-with--16-gb-cross-builds).

If you work in several worktrees, do **not** share one `CARGO_TARGET_DIR`
by default: two cargos on the same target flock and serialize. Use
`scripts/dev-cargo.sh` / `scripts/dev-test.sh`, which give each workspace
its own Cargo `build-dir` (`{workspace-path-hash}` under
`${CODEWHALE_CACHE_ROOT:-${XDG_CACHE_HOME:-$HOME/.cache}/codewhale}`).
`CODEWHALE_DEV_CACHE=local` keeps `./target` if you want that.
`sccache` wraps rustc only when incremental compilation is already off
(`CARGO_INCREMENTAL=0` or `CODEWHALE_SCCACHE=1`) and `sccache` is on
`PATH`; a missing binary is a printed fallback, not an error. Override
the cache root with `CODEWHALE_CACHE_ROOT` — there is no machine-specific
default. A single shared `CARGO_TARGET_DIR` remains valid only for
serialized trunk work. See
[`docs/BUILD_PERFORMANCE.md`](docs/BUILD_PERFORMANCE.md).

Some checks are platform-bound or intentionally excluded from an ordinary
change. Choose them for the risk they answer rather than treating every
available suite as ritual. Visible TUI behavior is accepted in the actual
terminal at the sizes and interaction path affected by the change; the former
full-screen PTY assertion suite was removed because it froze layout and copy
while missing product quality.

- **Long-running process acceptance** should use a sealed local home, local
  fixtures, and the real binary. Record the terminal size, inputs, visible
  result, and any filesystem side effect instead of adding a full-screen
  golden.
- **OCR** (`image_ocr`) uses the macOS Vision framework or a locally
  installed `tesseract`; its platform-specific paths are
  `cfg(target_os = "macos")`-gated and depend on host tooling.
- **Seatbelt sandbox** tests are macOS-only (`cfg(target_os =
  "macos")` at the module level) and do not run elsewhere.

#### Local git hooks are optional

This repository does not install git hooks, and no hook installer
exists; CI is the enforced gate. If you want a local `pre-push` hook
that runs the commands above, add it yourself (`.git/hooks/pre-push` or
`git config core.hooksPath`). Constraints for any local hook:

- A hook must never push, tag, publish, deploy, mutate credentials, or
  rewrite the working tree (no auto-fix commits or silent file
  modification). It may only verify and report.
- To bypass your own hook for a knowingly documented reason (for
  example, pushing work-in-progress to your own fork branch), use
  `git push --no-verify` and say so in the PR description. Bypassing a
  local hook does not make the gates pass — CI still runs them, and a
  bypassed gate must never be reported as a passing one.
- Release publication (tags, GitHub Releases, crates/npm artifacts) is a
  separate, owner-approved gate. Neither local hooks nor a green local
  run authorize any publication step.

### Commit Messages

Use clear, descriptive commit messages following conventional commits:

- `feat:` New feature
- `fix:` Bug fix
- `docs:` Documentation changes
- `refactor:` Code refactoring
- `test:` Adding or updating tests
- `chore:` Maintenance tasks

Example: `feat: add doctor subcommand for system diagnostics`

**Changelog entries are written on `main` at merge time, not in PRs.** Do not
edit `CHANGELOG.md` or `crates/tui/CHANGELOG.md` on a branch: every PR
touching them re-conflicts with every other PR touching them. The release
manager writes one batched "receipts" commit per merge session, and
`./scripts/sync-changelog.sh` keeps the packaged slice in sync. A PR that
carries changelog hunks will be asked to strip them
(`git checkout origin/main -- CHANGELOG.md crates/tui/CHANGELOG.md`).

**AI-assistant co-author trailers are fine.** Using an assistant is welcome and
needs no disclosure, and CI no longer rejects an auto-appended
`Co-authored-by: <some tool>` line. What we do care about is that the humans who
did the work are named — `Co-authored-by` feeds the GitHub contribution graph.
Remove an auto-appended line only if you want to:

```bash
git rebase -i origin/main   # reword each commit, delete the Co-authored-by line
```

Co-author a *person* freely; the address must be their GitHub-linked one
(`id+login@users.noreply.github.com`) or the credit does not register.

When a commit harvests code from a community PR (see "How Your Contribution
Lands" below), include a `Harvested from PR #N by @author` line in the commit
body. An auto-close workflow watches for this pattern and closes the
referenced PR with credit so the contributor gets a clear signal that
their work shipped.

## How Your Contribution Lands

We follow a deliberate "land what's useful, credit the contributor" model
that occasionally surprises new contributors. Two paths:

### Path 1 — Direct merge

If your PR is well-scoped, passes CI, doesn't touch the trust-boundary
surface (auth / sandbox / publishing / branding), and doesn't conflict
with main, a maintainer merges it directly. This is the most common
outcome for small bug fixes and well-tested feature additions.

### Path 2 — Harvest

If your PR is large, mixes scope, conflicts with main, or needs polish
that's faster for the maintainer to apply than to round-trip with the
contributor, the maintainer may **harvest** the useful commits or hunks
into a new commit on `main` rather than merging the PR directly. This is
**not a rejection** — it means your code landed.

When this happens:

- The harvested commit's message includes `Harvested from PR #N by
  @your-handle`. This is the contract: that line is your credit and the
  signal that your contribution shipped.
- If the maintainer copies or adapts your code, the harvested commit also
  keeps attribution with the original author identity when possible: either by
  preserving the commit author on a cherry-pick or by adding a
  `Co-authored-by: Name <id+login@users.noreply.github.com>` trailer. This is
  what lets GitHub's contribution surfaces recognize more than prose credit.
  Maintainers should use `.github/AUTHOR_MAP`, or run
  `gh api users/<login> --jq '"\(.id)+\(.login)@users.noreply.github.com"'`,
  rather than copying raw, `.local`, or old-style noreply emails from a
  contributor's machine.
- The `CHANGELOG.md` entry for the next release credits you by handle.
- The auto-close workflow closes your PR with a templated thank-you and
  a link to the commit on `main`.

When a maintainer closes a harvested PR by hand, the closing comment
follows this template (the pattern set on PR #2634):

```text
Closing with harvest credit, @handle — <what landed> landed via
<commit sha(s) or PR #N>. <If work remains:> The remainder is tracked
in #NNN — follow-ups welcome there.
Thank you for <one specific thing the contribution got right>.
```

Three required elements: the contributor's handle, the exact commits or
PRs where their work landed, and — when the PR contained more than what
landed — a tracking issue for the remainder. A harvested PR is never
closed with a bare "superseded".

To make a future contribution land via the faster Direct-Merge path
instead of the Harvest path, the highest-leverage things you can do are:

1. **Keep PRs single-purpose.** One bug fix per PR; one feature per PR.
   Don't mix a refactor with a feature.
2. **Rebase onto current `main` before opening the PR**, and after CI
   feedback. Conflicts force the harvest path even when the change is
   small.
3. **Include tests** with new behavior. The maintainer often harvests
   PRs without tests because adding the test is faster than asking the
   contributor for one.
4. **Avoid the trust-boundary surface** without prior maintainer
   sign-off. That includes auth/credential flows, sandbox policy,
   publishing/release plumbing, and `prompts/` content. PRs that touch
   these without prior discussion are unlikely to merge directly even
   when the change is well-implemented.

## Layered and EPIC-Sized Work

Some architecture work is too large for one PR but still needs to be built in
dependent layers. For those changes, use this workflow:

1. Start with a tracking issue or EPIC when the work spans multiple PRs. Name
   the intended slices and state what each slice is not trying to close yet.
2. Keep each implementation PR focused on one behavior boundary.
3. Later layers may stay in your fork or open as draft PRs while the lower
   layer is still moving. Draft stacked PR titles or descriptions should say
   `Draft / depends on #NNNN`.
4. A dependent PR is not ready for merge review until the lower layer has
   landed, the branch has been rebased onto current `main`, and the PR targets
   `main`.
5. The PR body should identify which earlier PR it builds on, what is in scope,
   what is explicitly out of scope, which issues it references, and which local
   commands were run.
6. Use `Closes #...` only when the slice fully satisfies an issue. Use
   `Refs #...` with a short `(partial)` note when the PR advances a broad issue
   but leaves follow-up work.
7. Structured commits are fine during review. Maintainers may squash or harvest
   at merge time, with contributor credit preserved through authorship,
   co-author trailers, changelog entries, or PR/issue comments. When the merge
   commit itself carries a `Harvested from PR #N by @author` line, that PR is
   merged with rebase or a merge commit rather than squashed, so the line
   reaches `main` intact and the auto-close credit fires.

Before asking for merge review on a layered PR, check that it is:

- rebased onto current `main`
- marked ready for review, not draft
- focused to one behavior boundary
- backed by local command evidence in the PR body
- green in CI, or has any remaining red lane clearly explained
- covered by round-trip or migration-preservation tests when it changes config
  or schema behavior
- referencing broad issues as partial unless it really closes them

For layered work, a useful PR description shape is:

```text
Summary:
Scope:
Not in this slice:
Builds on:
Issues:
Validation:
```

## The Stewardship Branch

Large refactors and architecture work stage on
`codex/v0.9.0-stewardship` before reaching `main`. The branch exists so
that multi-layer series (like the command-group refactor) can land layer
by layer against a stable base, get validated by their parity harnesses,
and then flow to `main` in periodic stewardship merges — instead of each
layer racing `main`'s daily churn.

What this means for you:

- **Base layered/EPIC-sized refactor PRs on `codex/v0.9.0-stewardship`**
  and target the PR there (see #2888 for the model). Ordinary bug fixes
  and features still target `main`.
- Maintainers merge the stewardship branch into `main` periodically;
  your work reaches `main` with its history and credit intact.
- If you're unsure which base to use, ask in your tracking issue — the
  default for anything that isn't a multi-PR series is `main`.

## Contribution Gate

Codewhale uses a maintainer-managed contribution gate for the community front
door. Maintainers and collaborators bypass this gate automatically. The gate
workflows default to dry-run / comment-only mode so maintainers can observe the
signal before changing contributor flow.

The maintainer posture is documented in
[docs/AGENT_ETHOS.md](docs/AGENT_ETHOS.md): automation should reduce load while
keeping good-faith contributors seen, credited, and able to keep helping.

Issues are never auto-closed by the contribution gate. Unapproved external
issues receive a short welcome note that asks for reproduction details and then
remain open for maintainer triage. Codewhale depends on real edge cases from
real users, so issue intake should stay warm and open.

Pull requests are different because they can touch code, CI, release plumbing,
auth, sandboxing, provider policy, and other trust-boundary surfaces. The PR
gate can be switched from dry-run to enforcement when maintainers decide they
need that safety control, but it should be treated as a review-load control,
not a judgment on contributor quality. Before enabling PR enforcement, seed the
allowlist broadly enough for active external contributors who should not be
interrupted by the rollout.

The allowlist is scoped:

- `pr:username` allows pull requests.
- `issue:username` allows issues.
- `all:username` allows both.

A maintainer can approve someone by commenting `/lgtm` on a pull request for PR
access, or `/lgtmi` on an issue for issue access. The exact bare commands
`lgtm` and `lgtmi` are also accepted for compatibility, but the prefixed forms
are preferred because they are harder to trigger accidentally in ordinary review
discussion.

Approvals do not edit `main` directly. The approval workflow opens a small
allowlist update PR so the new entry is reviewable before it takes effect.

If the PR gate fires on a good contributor incorrectly, use the same approval
flow to restore them: comment `/lgtm`, merge the generated allowlist PR, then
reopen the affected pull request. If GitHub will not allow the closed PR to be
reopened, ask the contributor to resubmit after the allowlist PR is merged.

## Agent-Assisted Improvements

Codewhale is allowed to help improve Codewhale, but the contribution still has
to be shaped for human review. The recommended workflow is the recursive self-improvement prompt
in the private `codewhale-ops` repo: run it
from a fresh fork or branch, let the agent find exactly one small friction point,
and stop after one patch. DeepSeek V4 Pro is the reference path for this loop
today, but any configured provider works — the review shape matters more than
the provider.

Agents and maintainers should follow the stewardship posture in
[docs/AGENT_ETHOS.md](docs/AGENT_ETHOS.md): use automation for evidence,
verification, and narrow patches while keeping the final community decision
human-reviewed.

The useful output is not "ideas for improvement." The useful output is a
specific reproduction, a minimal diff, focused checks, and a PR description that
explains the trade-off. Do not use an agent to touch auth, credentials, sandbox
policy, publishing/release plumbing, provider policy, telemetry, sponsorship,
branding, or global prompts without prior maintainer sign-off.

## Project Structure

codewhale is a Cargo workspace. The live runtime and the majority of TUI,
engine, and tool code currently live in `crates/tui/src/`. Smaller workspace
crates provide shared abstractions that are being extracted incrementally.

```
crates/
├── tui/           codewhale-tui binary (interactive TUI + runtime API)
├── cli/           codewhale binary (dispatcher facade)
├── app-server/    HTTP/SSE + JSON-RPC transport
├── core/          Agent loop / session / turn management
├── protocol/      Request/response framing
├── config/        Config loading, profiles, env precedence
├── state/         SQLite thread/session persistence
├── tools/         Typed tool specs and lifecycle
├── mcp/           MCP client + stdio server
├── hooks/         Lifecycle hooks (stdout/jsonl/webhook)
├── execpolicy/    Approval/sandbox policy engine
├── agent/         Model/provider registry
```

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the live data flow across
these crates, including the bottom-up build order.

## Submitting Changes

1. Create a feature branch from `main`:
   ```bash
   git checkout -b feat/your-feature
   ```

2. Make your changes and commit them

3. Run the pre-push verification commands (see
   [Pre-push verification](#pre-push-verification) above for the exact
   gate and the stricter release clippy form)

4. Push your branch and create a Pull Request

5. Describe your changes clearly in the PR description

## Pull Request Guidelines

- Use the [pull request template](.github/PULL_REQUEST_TEMPLATE.md) when opening
  a PR — it includes the Summary, Testing, and Checklist sections reviewers
  expect
- Keep PRs focused on a single change
- Update documentation if needed
- Add tests for new functionality
- Ensure CI passes before requesting review

## Shape of a Typical PR

A well-structured PR follows a consistent pattern. Recent exemplars include:

- **#386** — `/init` command: new `crates/tui/src/commands/groups/project/init.rs` module, project-type detection,
  AGENTS.md generation, command registration in `commands/mod.rs`, localization strings.
- **#389** — Inline LSP diagnostics: LSP subsystem in `crates/tui/src/lsp/`, engine hooks in
  `crates/tui/src/core/engine/lsp_hooks.rs`, config toggle, test coverage.
- **#387** — Self-update: new `crates/cli/src/update.rs` module, CLI subcommand registration,
  HTTP download + SHA256 verification + atomic binary replacement.
- **#393** — `/share` session URL: new `crates/tui/src/commands/groups/project/share.rs`, HTML rendering,
  `gh gist create` integration, command registration.
- **#343/#346** — (v0.8.5) Runtime thread/turn timeline and durable task manager refactors.

Typically each PR touches 1–3 new files, modifies 2–5 existing files for wiring
(registries, dispatch matches, localization), and adds or updates tests. Changes
are scoped to a single feature or fix — if you discover related work that needs
doing, open a separate issue rather than expanding the PR scope.

Before submitting, run the commands in
[Pre-push verification](#pre-push-verification).

## Reporting Issues

When reporting issues, please use one of the issue templates:

- [Bug report](.github/ISSUE_TEMPLATE/bug_report.md) — for reproducible problems
  or regressions
- [Feature request](.github/ISSUE_TEMPLATE/feature_request.md) — for ideas and
  improvements

Issue reports should include:

- Operating system and version
- Rust version (`rustc --version`)
- codewhale version (`codewhale --version`)
- Steps to reproduce the issue
- Expected vs actual behavior
- Relevant error messages or logs

## Security

If you discover a security vulnerability, please do **not** open a public issue.
See [SECURITY.md](SECURITY.md) for the responsible disclosure process and
contact information.

## Code of Conduct

Be respectful and inclusive. We welcome contributors of all backgrounds and
experience levels. See [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) for the full
code of conduct.

## License

By contributing to codewhale, you agree that your contributions will be licensed under the MIT License.

## Questions?

Feel free to open an issue for any questions about contributing.
