# `crates/tui/tests/`

Integration tests for the TUI binary. Per `CONTRIBUTING.md`, each crate's
integration tests live in its own `tests/` directory; the repository-root
`tests/` directory is unused.

## Harness consolidation (build-time lane #5247)

`crates/tui/tests/` used to ship 26 root-level `*.rs` binaries, each linking
the full `codewhale-tui` graph plus `cucumber`/`wiremock`/`rio-vt`. That was
~26 large link jobs per `cargo test -p codewhale-tui` and a major share of the
30-minute suite in #4991.

Since #5247 the 26 files are consolidated into **3 directory harnesses** (plus
the `codewhale-tui` bin unit tests):

| harness | binary | what lives there | why it stays separate |
|---|---|---|---|
| `tests/integration/main.rs` | `integration` | 17 plain `#[test]`/`#[tokio::test]` suites: `adaptive_evidence_acceptance`, `cache_guard`, `coordination_acceptance`, `diagnostic_read_only`, `dotenv_authority`, `eval_harness`, `exec_persistent_service`, `exec_stream_drop_acceptance`, `exec_turn_usage`, `integration_mock_llm`, `palette_audit`, `protocol_recovery`, `reasoning_content_replayed_after_tool_call`, `skill_cli`, `telemetry_contract`, `verifiers_harness_contract`, `workflow_tool_stream_acceptance` | All are process-level but require no PTY or Gherkin runner; they share `wiremock`/`tempfile` and link the TUI once instead of 17 times. `crate::` for `eval`/`models`/`llm_client`/`palette`/`tool_parser`/`network_policy`/`config`/`install` is satisfied by `integration/main.rs` re-exporting those `#[path]` modules at the harness crate root so `crate::config` etc. resolve. |
| `tests/cucumber/main.rs` | `cucumber` | 6 Gherkin runners: `core_session_command_extraction`, `directory_listing_acceptance`, `epic_acceptance_harness`, `eval_smoke_acceptance`, `plugin_e2e_acceptance`, `tool_lifecycle_acceptance` | Each defines a distinct `cucumber::World`; steps are registered per-World via inventory, so merging is safe and cuts 6 `cucumber` link jobs to 1. `plugin_e2e`’s PTY part is `#[cfg(all(unix, feature="long-running-tests"))]` and stays dormant in the default run. |
| `tests/pty/main.rs` | `pty` | 4 real-PTY suites: `qa_pty`, `release_runtime_qa`, `terminal_matrix_qa`, `work_bar_subagents_pty` | Each boots the real binary in a `portable-pty` + `rio-vt` session. They are `#[cfg(unix)]` and serialize on `QA_PTY_TEST_LOCK` etc.; one binary links `rio-vt`/`portable-pty` once instead of 4 times. |

`ls crates/tui/tests/*.rs | wc -l` is now **0** (all `*.rs` live under `integration/`, `cucumber/`, `pty/`). The surviving binaries are the 3 directory harnesses above.

Filtering still works via the module path:

```sh
cargo test -p codewhale-tui --tests -- --list | grep adaptive_evidence
cargo test -p codewhale-tui --test integration adaptive_evidence_acceptance -- --nocapture
cargo test -p codewhale-tui --test cucumber tool_lifecycle -- --nocapture
cargo test -p codewhale-tui --test pty qa_pty -- --nocapture
```

The shared helpers in `crates/tui/tests/support/` (`qa_harness`, `llm_client`) and fixtures in `crates/tui/tests/fixtures/` are untouched — harnesses reach them via `../support`.

## Mock LLM client (`integration::integration_mock_llm`)

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

Full-engine journeys use `Engine::new_with_model_client` and the same mock.
When a model-visible behavior crosses prompt assembly, a provider call, a tool
gate or execution, and the follow-up prompt, add a keyless assembled journey to
the existing engine harness. Assert the externally meaningful events, exact
side effects, accounting, and next request. Do not add a new snapshot framework,
network call, provider key, timing sleep, or platform shell merely to cover the
journey.

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
