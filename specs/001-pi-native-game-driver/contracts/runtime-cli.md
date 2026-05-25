# Contract: Runtime JSON Helper

`genmicon-game-runtime` is a local helper owned by `crates/game`. It reads one
JSON request from stdin and writes one JSON response to stdout.

## Request Envelope

```json
{
  "command": "validate|status|render|playbook|lookup|fact_check|run_driver|commit_turn|list_saves",
  "game_root": "examples/games/reconciliation-demo",
  "save_id": "default",
  "developer": false,
  "payload": {}
}
```

## Response Envelope

```json
{
  "ok": true,
  "data": {},
  "warnings": [],
  "error": null
}
```

On failure:

```json
{
  "ok": false,
  "data": null,
  "warnings": [],
  "error": {
    "code": "revision_conflict",
    "message": "Expected revision 1 but save is at revision 2",
    "recoverable": true
  }
}
```

## Acceptance Rules

- The helper never reads paths outside declared game/driver/save roots.
- `commit_turn` is the only command that writes save files.
- All responses are valid JSON and contain no terminal control codes.
- Errors are structured so the Pi package can render player-safe and
  developer-diagnostic variants.

## Implementation Cross-Check

- `crates/game/src/bin/genmicon-game-runtime.rs` reads one stdin request and
  writes one stdout response.
- `crates/game/src/cli.rs` implements the request/response envelopes and all
  contracted commands.
- `crates/game/tests/runtime_cli.rs` covers invalid JSON, validation,
  save-listing, fact checks, commit-once revision protection, and resume from
  saved state without transcript context.
- Fixture tests cover `reconciliation-demo` and `thirteen-angry-man`.
