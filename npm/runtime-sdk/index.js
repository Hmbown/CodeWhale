const DEFAULT_BASE_URL = "http://127.0.0.1:7878";

export class RuntimeApiError extends Error {
  constructor(message, options = {}) {
    super(message);
    this.name = "RuntimeApiError";
    this.status = options.status;
    this.method = options.method;
    this.path = options.path;
    this.body = options.body;
  }
}

export class RuntimeCapabilityError extends RuntimeApiError {
  constructor(capability, message, options = {}) {
    super(message, options);
    this.name = "RuntimeCapabilityError";
    this.capability = capability;
  }
}

export class CodeWhaleRuntimeClient {
  constructor(options = {}) {
    this.baseUrl = normalizeBaseUrl(options.baseUrl ?? DEFAULT_BASE_URL);
    this.token = options.token ?? null;
    this.fetchImpl = options.fetch ?? globalThis.fetch;
    if (typeof this.fetchImpl !== "function") {
      throw new TypeError("CodeWhaleRuntimeClient requires a fetch implementation");
    }
  }

  async createFleetRun(spec) {
    return this.#jsonRequest("/v1/fleet/runs", {
      method: "POST",
      body: spec,
      capability: "fleet_run_create",
    });
  }

  async startFleetRun(runId) {
    return this.#jsonRequest(`/v1/fleet/runs/${segment(runId)}/start`, {
      method: "POST",
      capability: "fleet_run_start",
    });
  }

  async replayFleetEvents(runId, options = {}) {
    const path = fleetEventPath(
      `/v1/fleet/runs/${segment(runId)}/events/replay`,
      options,
    );
    return this.#jsonRequest(path, {
      capability: "fleet_event_replay",
    });
  }

  async listFleetRuns() {
    return this.#jsonRequest("/v1/fleet/runs");
  }

  async getFleetRun(runId) {
    return this.#jsonRequest(`/v1/fleet/runs/${segment(runId)}`);
  }

  async listFleetWorkers(runId) {
    return this.#jsonRequest(`/v1/fleet/runs/${segment(runId)}/workers`);
  }

  async getFleetWorker(workerId) {
    return this.#jsonRequest(`/v1/fleet/workers/${segment(workerId)}`);
  }

  async interruptWorker(workerId) {
    return this.#jsonRequest(`/v1/fleet/workers/${segment(workerId)}/interrupt`, {
      method: "POST",
    });
  }

  async stopWorker(workerId) {
    return this.#jsonRequest(`/v1/fleet/workers/${segment(workerId)}/stop`, {
      method: "POST",
    });
  }

  async restartWorker(workerId) {
    return this.#jsonRequest(`/v1/fleet/workers/${segment(workerId)}/restart`, {
      method: "POST",
    });
  }

  async stopFleetRun(runId) {
    return this.#jsonRequest(`/v1/fleet/runs/${segment(runId)}/stop`, {
      method: "POST",
    });
  }

  async *fleetEvents(runId, options = {}) {
    const path = fleetEventPath(
      options.path ?? `/v1/fleet/runs/${segment(runId)}/events`,
      options,
    );
    const response = await this.#rawRequest(path, {
      method: "GET",
      capability: "fleet_event_stream",
      accept: "text/event-stream",
    });
    const contentType = response.headers.get("content-type") ?? "";
    if (contentType.includes("application/json")) {
      const payload = await response.json();
      const events = Array.isArray(payload) ? payload : (payload.events ?? []);
      for (const event of events) {
        yield event;
      }
      return;
    }
    if (!response.body) {
      throw new RuntimeApiError("Runtime API event response did not include a readable body", {
        method: "GET",
        path,
      });
    }
    for await (const event of parseEventStream(response.body)) {
      yield event;
    }
  }

  async #jsonRequest(path, options = {}) {
    const response = await this.#rawRequest(path, options);
    if (response.status === 204) {
      return null;
    }
    return response.json();
  }

  async #rawRequest(path, options = {}) {
    const method = options.method ?? "GET";
    const headers = new Headers(options.headers);
    headers.set("accept", options.accept ?? "application/json");
    if (this.token) {
      headers.set("authorization", `Bearer ${this.token}`);
    }
    const init = { method, headers };
    if (options.body !== undefined) {
      headers.set("content-type", "application/json");
      init.body = JSON.stringify(options.body);
    }

    const response = await this.fetchImpl(new URL(path, this.baseUrl), init);
    if (response.ok) {
      return response;
    }

    const body = await readErrorBody(response);
    const errorOptions = { status: response.status, method, path, body };
    if (options.capability && [404, 405, 501].includes(response.status)) {
      throw new RuntimeCapabilityError(
        options.capability,
        `Runtime API capability '${options.capability}' is not available at ${method} ${path}`,
        errorOptions,
      );
    }
    throw new RuntimeApiError(
      `Runtime API request failed (${response.status}) for ${method} ${path}`,
      errorOptions,
    );
  }
}

export function createRuntimeClient(options = {}) {
  return new CodeWhaleRuntimeClient(options);
}

function normalizeBaseUrl(value) {
  return value.endsWith("/") ? value : `${value}/`;
}

function segment(value) {
  if (value === null || value === undefined || String(value).trim() === "") {
    throw new TypeError("Runtime API path segment must be a non-empty value");
  }
  return encodeURIComponent(String(value));
}

function fleetEventPath(path, options) {
  const query = new URLSearchParams();
  if (options.after !== undefined && options.after !== null && String(options.after) !== "") {
    query.set("after", String(options.after));
  }
  if (options.limit !== undefined && options.limit !== null) {
    query.set("limit", String(options.limit));
  }
  const encoded = query.toString();
  if (!encoded) {
    return path;
  }
  return `${path}${path.includes("?") ? "&" : "?"}${encoded}`;
}

async function readErrorBody(response) {
  try {
    const text = await response.text();
    return text.length > 4096 ? `${text.slice(0, 4096)}...` : text;
  } catch {
    return "";
  }
}

async function* parseEventStream(body) {
  const decoder = new TextDecoder();
  let buffer = "";
  for await (const chunk of body) {
    buffer += decoder.decode(chunk, { stream: true });
    let boundary;
    while ((boundary = eventStreamBoundary(buffer)) !== null) {
      const frame = buffer.slice(0, boundary.index);
      buffer = buffer.slice(boundary.index + boundary.length);
      const event = parseSseFrame(frame);
      if (event !== undefined) {
        yield event;
      }
    }
  }
  buffer += decoder.decode();
  const event = parseSseFrame(buffer);
  if (event !== undefined) {
    yield event;
  }
}

function eventStreamBoundary(buffer) {
  const lf = buffer.indexOf("\n\n");
  const crlf = buffer.indexOf("\r\n\r\n");
  if (lf < 0 && crlf < 0) {
    return null;
  }
  if (crlf >= 0 && (lf < 0 || crlf < lf)) {
    return { index: crlf, length: 4 };
  }
  return { index: lf, length: 2 };
}

function parseSseFrame(frame) {
  const lines = frame.split(/\r?\n/);
  const eventName = lines
    .find((line) => line.startsWith("event:"))
    ?.slice("event:".length)
    .trimStart();
  const eventId = lines
    .find((line) => line.startsWith("id:"))
    ?.slice("id:".length)
    .trimStart();
  const data = lines
    .filter((line) => line.startsWith("data:"))
    .map((line) => line.slice("data:".length).trimStart())
    .join("\n");
  if (!data || data === "[DONE]") {
    return undefined;
  }
  const parsed = JSON.parse(data);
  if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
    if (eventName && parsed.event === undefined) {
      parsed.event = eventName;
    }
    if (eventId && parsed.cursor === undefined) {
      parsed.cursor = eventId;
    }
  }
  return parsed;
}
