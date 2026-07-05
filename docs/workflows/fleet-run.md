# Run parallel work with Fleet

Use Fleet when work needs retry, sleep/restart survival, or a ledgered audit
trail — each worker is a headless `codewhale exec` run tracked durably in
`.codewhale/fleet.jsonl`.

Write a task spec (JSON or TOML):

```json
{
  "name": "local smoke",
  "tasks": [
    { "id": "lint", "name": "Lint",
      "instructions": "Run the lint check and report failures.",
      "expected_artifacts": ["log"] }
  ]
}
```

Then run and watch it:

```bash
codewhale fleet run tasks.json --max-workers 4
codewhale fleet status
codewhale fleet logs <worker-id>
codewhale fleet resume <run-id>   # safe after a crash/sleep; replays the ledger
```

**Done when:** `fleet status` shows every task completed and the artifacts you
declared exist under `.codewhale/fleet/`.

See also: [FLEET.md](../FLEET.md) — full task-spec fields, agent profiles, and
`/fleet setup`.
