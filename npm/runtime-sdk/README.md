# @codewhale/runtime-sdk

Small JavaScript helpers and TypeScript declarations for Codewhale's local
Runtime API. The package is intentionally transport-only: it never bypasses the
Rust runtime, sandbox, approvals, provider configuration, or Runtime execution
ledger.

```js
import { createRuntimeClient } from "@codewhale/runtime-sdk";

const client = createRuntimeClient({
  baseUrl: "http://127.0.0.1:7878",
  token: process.env.CODEWHALE_RUNTIME_TOKEN,
});

const created = await client.createFleetRun({
  target: "this_computer",
  roles: [{ name: "reviewer" }, { name: "verifier" }],
  workflow: {
    id: "release-check",
    kind: "parallel",
    tasks: [
      { id: "review", name: "Review", instructions: "Review locally.", worker: { role: "reviewer" } },
      { id: "verify", name: "Verify", instructions: "Verify locally.", worker: { role: "verifier" } },
    ],
  },
});

// Creation is durable but does not launch work. Launch remains explicit.
await client.startFleetRun(created.run.id);

let cursor;
for await (const event of client.fleetEvents(created.run.id, { after: cursor })) {
  if (event.cursor) cursor = event.cursor; // persist durable cursors only
  if (event.event === "fleet.replay.cursor_unavailable") {
    // Reload getFleetRun(created.run.id), then reconnect without the old cursor.
  }
}
```

## Fleet Helpers

- `listFleetRuns()`
- `getFleetRun(runId)`
- `listFleetWorkers(runId)`
- `getFleetWorker(workerId)`
- `interruptWorker(workerId)`
- `stopWorker(workerId)`
- `restartWorker(workerId)`
- `stopFleetRun(runId)`
- `startFleetRun(runId)`
- `replayFleetEvents(runId, { after, limit })`
- `fleetEvents(runId, { after, limit })`
- `createFleetRun(spec)`

The v0.9.11 Runtime implements the complete local managed-Fleet path. Fleet
names the roster and selected member; the Runtime owns launch authority,
durable run/worker/event state, replay, and execution. A creation request must
name its roles, define a `parallel` Workflow, and select the explicit
`this_computer` target. `another_computer` and `cloud` are contract values but
fail closed until those targets are implemented. Event cursors are opaque and
durable across Runtime restarts; if Runtime ledger compaction removes an old
cursor, replay returns a conflict so the client can reload the run projection.
Local worker IDs are generated per run; managed creation does not yet accept
caller-assigned `worker_specs` because worker controls address IDs globally.

Older runtimes that do not expose one of these endpoints produce a
`RuntimeCapabilityError` with a stable capability string instead of a generic
fetch failure.
