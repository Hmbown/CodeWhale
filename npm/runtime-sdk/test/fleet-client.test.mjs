import assert from "node:assert/strict";
import test from "node:test";
import {
  CodeWhaleRuntimeClient,
  RuntimeApiError,
  RuntimeCapabilityError,
  createRuntimeClient,
} from "../index.js";

function jsonResponse(body, init = {}) {
  return new Response(JSON.stringify(body), {
    status: init.status ?? 200,
    headers: { "content-type": "application/json", ...(init.headers ?? {}) },
  });
}

function fakeFetch(responseFactory) {
  const calls = [];
  const fetch = async (url, init) => {
    calls.push({ url: url.toString(), init });
    return responseFactory(url, init, calls.length);
  };
  fetch.calls = calls;
  return fetch;
}

test("createRuntimeClient returns CodeWhaleRuntimeClient instance", () => {
  const fetch = fakeFetch(() => jsonResponse({}));
  const client = createRuntimeClient({ fetch });

  assert.ok(client instanceof CodeWhaleRuntimeClient);
  assert.equal(client.baseUrl, "http://127.0.0.1:7878/");
});

test("listFleetRuns calls the Runtime API with bearer auth", async () => {
  const fetch = fakeFetch(() =>
    jsonResponse({
      status: { runs: 1, workers: {} },
      runs: [{ id: "run-1", name: "smoke", tasks: [], labels: {} }],
    }),
  );
  const client = createRuntimeClient({
    baseUrl: "http://127.0.0.1:7878",
    token: "token-1",
    fetch,
  });

  const response = await client.listFleetRuns();

  assert.equal(response.runs[0].id, "run-1");
  assert.equal(fetch.calls[0].url, "http://127.0.0.1:7878/v1/fleet/runs");
  assert.equal(fetch.calls[0].init.method, "GET");
  assert.equal(fetch.calls[0].init.headers.get("authorization"), "Bearer token-1");
});

test("worker and run actions use POST endpoints", async () => {
  const fetch = fakeFetch((url) =>
    jsonResponse(
      url.pathname.endsWith("/stop")
        ? {
            action: "stop",
            run_id: "run-1",
            stopped: 1,
            status: { runs: 1, workers: {} },
          }
        : {
            action: url.pathname.endsWith("/restart")
              ? "restart"
              : url.pathname.endsWith("/stop")
                ? "stop"
                : "interrupt",
            worker: { worker_id: "w1", artifacts: [] },
          },
    ),
  );
  const client = new CodeWhaleRuntimeClient({ fetch });

  await client.interruptWorker("w1");
  await client.stopWorker("w1");
  await client.restartWorker("w1");
  await client.startFleetRun("run-1");
  await client.stopFleetRun("run-1");

  assert.deepEqual(
    fetch.calls.map((call) => [new URL(call.url).pathname, call.init.method]),
    [
      ["/v1/fleet/workers/w1/interrupt", "POST"],
      ["/v1/fleet/workers/w1/stop", "POST"],
      ["/v1/fleet/workers/w1/restart", "POST"],
      ["/v1/fleet/runs/run-1/start", "POST"],
      ["/v1/fleet/runs/run-1/stop", "POST"],
    ],
  );
});

test("managed Fleet helpers send explicit launch metadata and reconnect cursors", async () => {
  const fetch = fakeFetch((url) =>
    jsonResponse(
      url.pathname.endsWith("/events/replay")
        ? { run_id: "run-1", events: [], has_more: false, history_truncated: false }
        : { execution: "awaiting_start", run: { id: "run-1" }, warnings: [] },
    ),
  );
  const client = new CodeWhaleRuntimeClient({ fetch });
  const spec = {
    target: "this_computer",
    roles: [{ name: "reviewer" }],
    workflow: {
      id: "review",
      kind: "parallel",
      tasks: [
        {
          id: "review",
          name: "Review",
          instructions: "Review.",
          worker: { role: "reviewer" },
          budget: { max_steps: 0 },
        },
      ],
    },
  };

  await client.createFleetRun(spec);
  await client.replayFleetEvents("run-1", { after: "fev1_cursor_worker", limit: 25 });

  assert.deepEqual(JSON.parse(fetch.calls[0].init.body), spec);
  assert.equal(JSON.parse(fetch.calls[0].init.body).workflow.tasks[0].budget.max_steps, 0);
  const replayUrl = new URL(fetch.calls[1].url);
  assert.equal(replayUrl.pathname, "/v1/fleet/runs/run-1/events/replay");
  assert.equal(replayUrl.searchParams.get("after"), "fev1_cursor_worker");
  assert.equal(replayUrl.searchParams.get("limit"), "25");
});

test("unsupported fleet capabilities raise typed errors", async () => {
  const fetch = fakeFetch(() => jsonResponse({ error: "not found" }, { status: 404 }));
  const client = new CodeWhaleRuntimeClient({ fetch });

  await assert.rejects(
    () => client.createFleetRun({ name: "future" }),
    (error) =>
      error instanceof RuntimeCapabilityError &&
      error.capability === "fleet_run_create" &&
      error.status === 404,
  );

  await assert.rejects(
    async () => {
      for await (const _event of client.fleetEvents("run-1")) {
        throw new Error("unexpected event");
      }
    },
    (error) =>
      error instanceof RuntimeCapabilityError &&
      error.capability === "fleet_event_stream" &&
      error.status === 404,
  );
});

test("fleetEvents can replay JSON event fixtures when the API exposes them", async () => {
  const fetch = fakeFetch(() =>
    jsonResponse({
      events: [
        {
          seq: 1,
          run_id: "run-1",
          worker_id: "w1",
          task_id: "task-1",
          timestamp: "2026-06-13T00:00:00Z",
          label: "running",
          payload: { state: "running" },
        },
      ],
    }),
  );
  const client = new CodeWhaleRuntimeClient({ fetch });

  const events = [];
  for await (const event of client.fleetEvents("run-1", { path: "/v1/fleet/runs/run-1/events" })) {
    events.push(event);
  }

  assert.equal(events.length, 1);
  assert.equal(events[0].payload.state, "running");
});

test("fleetEvents parses text/event-stream frames", async () => {
  const encoder = new TextEncoder();
  const body = new ReadableStream({
    start(controller) {
      controller.enqueue(
        encoder.encode(
          'id: fev1_heartbeat_worker\nevent: fleet.worker.heartbeat\ndata: {"cursor":"fev1_heartbeat_worker","event":"fleet.worker.heartbeat","run_id":"run-1","worker_id":"w1","task_id":"task-1","timestamp":"2026-06-13T00:00:01Z","worker_seq":2,"payload":{"state":"heartbeat","memory_mb":128}}\n\n',
        ),
      );
      controller.close();
    },
  });
  const fetch = fakeFetch(
    () =>
      new Response(body, {
        status: 200,
        headers: { "content-type": "text/event-stream" },
      }),
  );
  const client = new CodeWhaleRuntimeClient({ fetch });

  const events = [];
  for await (const event of client.fleetEvents("run-1", { after: "fev1_previous", limit: 10 })) {
    events.push(event);
  }

  assert.equal(events.length, 1);
  assert.equal(events[0].payload.state, "heartbeat");
  assert.equal(events[0].payload.memory_mb, 128);
  const eventUrl = new URL(fetch.calls[0].url);
  assert.equal(eventUrl.searchParams.get("after"), "fev1_previous");
  assert.equal(eventUrl.searchParams.get("limit"), "10");
  assert.equal(fetch.calls[0].init.headers.get("accept"), "text/event-stream");
});

test("fleetEvents preserves SSE control event names", async () => {
  const encoder = new TextEncoder();
  const body = new ReadableStream({
    start(controller) {
      controller.enqueue(
        encoder.encode(
          'event: fleet.replay.cursor_unavailable\r\ndata: {"run_id":"run-1","reload_projection":true}\r\n\r\n',
        ),
      );
      controller.close();
    },
  });
  const client = new CodeWhaleRuntimeClient({
    fetch: fakeFetch(
      () =>
        new Response(body, {
          status: 200,
          headers: { "content-type": "text/event-stream" },
        }),
    ),
  });

  const events = [];
  for await (const event of client.fleetEvents("run-1")) {
    events.push(event);
  }

  assert.deepEqual(events, [
    {
      event: "fleet.replay.cursor_unavailable",
      run_id: "run-1",
      reload_projection: true,
    },
  ]);
});

test("ordinary HTTP errors remain RuntimeApiError", async () => {
  const fetch = fakeFetch(() => jsonResponse({ error: "bad" }, { status: 500 }));
  const client = new CodeWhaleRuntimeClient({ fetch });

  await assert.rejects(
    () => client.getFleetRun("run-1"),
    (error) =>
      error instanceof RuntimeApiError &&
      !(error instanceof RuntimeCapabilityError) &&
      error.status === 500,
  );
});
