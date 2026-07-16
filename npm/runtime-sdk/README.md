# @codewhale/runtime-sdk

Small JavaScript helpers and TypeScript declarations for Codewhale's local
Runtime API. The package is intentionally transport-only: it never bypasses the
Rust runtime, sandbox, approvals, provider configuration, or fleet ledger.

```js
import { createRuntimeClient } from "@codewhale/runtime-sdk";

const client = createRuntimeClient({
  baseUrl: "http://127.0.0.1:7878",
  token: process.env.CODEWHALE_RUNTIME_TOKEN,
});

const { runs } = await client.listFleetRuns();
const workers = await client.listFleetWorkers(runs[0].id);
await client.interruptWorker(workers.workers[0].worker_id);
```

## Fleet Helpers

- `listFleetRuns()`
- `getFleetRun(runId)`
- `listFleetWorkers(runId)`
- `getFleetWorker(workerId)`
- `interruptWorker(workerId)`
- `restartWorker(workerId)`
- `stopFleetRun(runId)`
- `fleetEvents(runId)`
- `createFleetRun(spec)`

`fleetEvents` and `createFleetRun` are typed ahead of the current v0.8.60 Rust
Runtime API. If the local runtime does not expose those endpoints, the helpers
raise `RuntimeCapabilityError` with a stable `capability` string instead of a
generic fetch failure.
