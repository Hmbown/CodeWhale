# TUI modularization plan

## Objective

Turn `crates/tui/src/tui/ui.rs` into a small composition root without changing
the observable terminal contract or creating a second turn loop. The final file
should own shared UI types, module wiring, and top-level orchestration only.

The baseline at `c93d21373` was 4,248 lines. Slices 1–9 are all extracted
(2026-08-21): `ui.rs` is now 2,130 lines of shared UI types, module wiring, and
top-level orchestration. Follow-on debt (partitioning `ui/event_loop.rs` and
`ui/tests.rs` by event source) is deliberately a second phase so it does not
hide the `ui.rs` extraction behind a wholesale rewrite.

## Rules for every extraction

- Move one cohesive responsibility at a time; do not redesign behavior while
  moving it.
- Give the new module explicit imports. Temporary parent re-exports are allowed
  only where existing sibling modules or tests still depend on them.
- Keep `Engine::run_turn` as the only turn loop.
- Run formatting, a `codewhale-tui` library check, and the focused tests that
  protect the moved behavior before starting the next extraction.
- Land conflict-prone slices only after the active owner has finished. In
  particular, the remote-control bridge must incorporate the current `/rc`
  recovery work rather than overwriting it.

## Ordered slices

1. **Terminal input and event fairness — extracted**
   `ui/terminal_input.rs` owns the input thread, liveness recovery, child-terminal
   pause/resume, and the engine-drain budget.

2. **Approval routing — extracted**
   `ui/approval_routing.rs` owns session approval/denial resolution and durable
   denial receipts.

3. **Remote-control bridge — extracted**
   Move enrollment events, local-turn attachment, command acknowledgement, and
   start/stop UI projection to `ui/remote_control_bridge.rs`. Do this after the
   active `/rc` changes in the v0.9.11 integration lane are committed, because
   those edits currently overlap this exact block.

4. **Observer hooks — extracted**
   Move subagent and turn-end hook payload construction, preview bounding, and
   completion classification to `ui/observer_hooks.rs`.

5. **Task and shell projection — extracted**
   Move task-panel refresh, shell live-output reconciliation, detached-job
   projection, and RLM task entries to `ui/task_projection.rs`.

6. **Paused-command and dispatch preparation — extracted**
   Move pause/resume planning and the dispatch preparation/outcome types to
   `ui/dispatch_prepare.rs`; keep actual dispatch execution in `dispatch.rs`.

7. **Compaction UI state — extracted**
   Move manual/automatic compaction queueing, settlement, receipts, and cancel
   behavior to `ui/compaction_flow.rs`.

8. **Provider configuration — extracted**
   Move web-config event draining, rollback snapshots, key verification, and
   provider setup tests to `ui/provider_setup.rs` and
   `ui/provider_setup/tests.rs`.

9. **Small residual policies — extracted**
   Move context-pressure warnings, update notices, and notification decisions
   into their existing domain modules. Leave `ui.rs` as wiring plus genuinely
   shared aliases.

## Follow-on debt

`ui/event_loop.rs` and `ui/tests.rs` are larger than `ui.rs`. Once the composition
root split is complete, partition the event loop by event source and move tests
next to the modules they exercise. This is a second phase so it does not hide
the `ui.rs` extraction behind a wholesale rewrite.
