# Contributing to DeepSeek TUI

Thank you for your interest in contributing to DeepSeek TUI! This document provides guidelines and instructions for contributing.

## Getting Started

### Prerequisites

- Rust 1.88 or later (edition 2024)
- Cargo package manager
- Git

### Setting Up Development Environment

1. Fork and clone the repository:
   ```bash
   git clone https://github.com/YOUR_USERNAME/DeepSeek-TUI.git
   cd DeepSeek-TUI
   ```

2. Build the project:
   ```bash
   cargo build
   ```

3. Run tests:
   ```bash
   cargo test
   ```

4. Run with development settings:
   ```bash
   cargo run
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

### Commit Messages

Use clear, descriptive commit messages following conventional commits:

- `feat:` New feature
- `fix:` Bug fix
- `docs:` Documentation changes
- `refactor:` Code refactoring
- `test:` Adding or updating tests
- `chore:` Maintenance tasks

Example: `feat: add doctor subcommand for system diagnostics`

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
- The `CHANGELOG.md` entry for the next release credits you by handle.
- The auto-close workflow closes your PR with a templated thank-you and
  a link to the commit on `main`.

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

## Project Structure

DeepSeek TUI is a Cargo workspace. The live runtime and the majority of TUI,
engine, and tool code currently live in `crates/tui/src/`. Smaller workspace
crates provide shared abstractions that are being extracted incrementally.

```
crates/
├── tui/           deepseek-tui binary (interactive TUI + runtime API)
├── cli/           deepseek binary (dispatcher facade)
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
└── tui-core/      Event-driven TUI state machine scaffold
```

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the live data flow across
these crates, including the bottom-up build order.

## Submitting Changes

1. Create a feature branch from `main`:
   ```bash
   git checkout -b feat/your-feature
   ```

2. Make your changes and commit them

3. Ensure CI passes:
   ```bash
   cargo fmt --check
   cargo clippy
   cargo test
   ```

4. Push your branch and create a Pull Request

5. Describe your changes clearly in the PR description

## Pull Request Guidelines

- Keep PRs focused on a single change
- Update documentation if needed
- Add tests for new functionality
- Ensure CI passes before requesting review

## Shape of a Typical PR

A well-structured PR follows a consistent pattern. Recent exemplars include:

- **#386** — `/init` command: new `crates/tui/src/commands/init.rs` module, project-type detection,
  AGENTS.md generation, command registration in `commands/mod.rs`, localization strings.
- **#389** — Inline LSP diagnostics: LSP subsystem in `crates/tui/src/lsp/`, engine hooks in
  `core/engine/lsp_hooks.rs`, config toggle, test coverage.
- **#387** — Self-update: new `crates/cli/src/update.rs` module, CLI subcommand registration,
  HTTP download + SHA256 verification + atomic binary replacement.
- **#393** — `/share` session URL: new `crates/tui/src/commands/share.rs`, HTML rendering,
  `gh gist create` integration, command registration.
- **#343/#346** — (v0.8.5) Runtime thread/turn timeline and durable task manager refactors.

Typically each PR touches 1–3 new files, modifies 2–5 existing files for wiring
(registries, dispatch matches, localization), and adds or updates tests. Changes
are scoped to a single feature or fix — if you discover related work that needs
doing, open a separate issue rather than expanding the PR scope.

Before submitting, run:
```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features 2>&1 | head -50
cargo check
```

## Reporting Issues

When reporting issues, please include:

- Operating system and version
- Rust version (`rustc --version`)
- DeepSeek TUI version (`deepseek --version`)
- Steps to reproduce the issue
- Expected vs actual behavior
- Relevant error messages or logs

## Code of Conduct

Be respectful and inclusive. We welcome contributors of all backgrounds and experience levels.

## License

By contributing to DeepSeek TUI, you agree that your contributions will be licensed under the MIT License.

## Questions?

Feel free to open an issue for any questions about contributing.
