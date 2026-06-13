# `crates/tui/tests/`

Integration tests for the TUI binary. Per `CONTRIBUTING.md`, each crate's
integration tests live in its own `tests/` directory; the repository-root
`tests/` directory is unused.

## Mock LLM client (`integration_mock_llm.rs`)

`crates/tui/src/llm_client/mock.rs` provides a `MockLlmClient` that implements
the `LlmClient` trait by replaying queue-driven canned responses and capturing
every outgoing `MessageRequest`. Tests mock at the **trait boundary** — never
at the `reqwest` HTTP layer — because the trait is the durable abstraction the
runtime is meant to depend on.

Coverage today exercises the trait surface end-to-end:

- streaming turn loop
- reasoning-content replay across tool-call rounds (V4 §5.1.1, the bug that
  broke v0.4.9-v0.5.1)
- tool-call round-trip with chunked input JSON
- multi-tool-call ordering inside a single turn
- compaction-style non-streaming `create_message`
- sub-agent style independent parent/child mocks
- capacity-gate observation of a captured request before stream drain

Four full-engine tests (`engine_full_*`) are `#[ignore]`-marked. They unblock
when `core::engine::Engine` is refactored to take `Arc<dyn LlmClient>` instead
of a concrete `Option<DeepSeekClient>`. See the comment block at the bottom of
`integration_mock_llm.rs` for the exact refactor surface.

## `--record` mode for `deepseek eval`

The offline `deepseek eval` harness now accepts `--record <DIR>`. When set,
each tool step appends one JSON Lines record to `<DIR>/<scenario>.jsonl`
(default scenario: `offline-tool-loop.jsonl`). Each line is a self-contained
JSON object with the schema:

```json
{ "request":  { "step": "list_dir", "kind": "List" },
  "response_events": [ { "type": "ok", "output": "…" } ] }
```

The mock LLM client (`crate::llm_client::mock`) replays these fixtures by
mapping each `response_events` array onto a canned `Vec<StreamEvent>`. Drop
generated fixtures into `crates/tui/tests/fixtures/` so they ride the repo and
feed the mock in CI.

Quick example:

```bash
cargo run --bin codewhale -- eval --record crates/tui/tests/fixtures
cat crates/tui/tests/fixtures/offline-tool-loop.jsonl | jq .
```

The scenario name is sanitized to `[A-Za-z0-9_-]` before forming the filename,
so unusual scenario strings stay portable across platforms.
