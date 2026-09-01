#!/usr/bin/env python3
"""Local Concentrate contract stub for keyless dogfood (no network, no account).

Speaks the documented surface of https://api.concentrate.ai/v1 well enough to
prove Codewhale's real request path end to end:

  GET  /v1/responses/health   -> 200, empty body (unauthenticated)
  GET  /v1/models             -> {"object":"list","data":[{"id":...}]} (unauthenticated)
  POST /v1/responses          -> typed `response.*` SSE events, no `[DONE]`

Contract sources (fetched 2026-08-29):
  https://concentrate.ai/docs/api-reference/introduction
  https://concentrate.ai/docs/api-reference/endpoint/request-parameters
  https://concentrate.ai/docs/api-reference/endpoint/streaming
  https://concentrate.ai/docs/api-reference/endpoint/errors
  https://concentrate.ai/docs/api-reference/endpoint/list-models
  https://concentrate.ai/docs/api-reference/endpoint/health

The stub asserts what a real gateway would enforce and what Codewhale must
send: a `Bearer` Authorization header equal to CONCENTRATE_STUB_EXPECT_KEY,
`model` passed through verbatim, `stream: true`, and no undocumented top-level
fields. Every request is appended as JSON to CONCENTRATE_STUB_LOG so the
driver can assert the receipt after the run. A wrong key answers with the
documented 401 body so the error path is exercised too.

Usage: CONCENTRATE_STUB_PORT=8790 CONCENTRATE_STUB_EXPECT_KEY=stub-key \
       CONCENTRATE_STUB_LOG=/tmp/concentrate-stub.jsonl python3 scripts/concentrate-stub.py
"""
from __future__ import annotations

import json
import os
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(os.environ.get("CONCENTRATE_STUB_PORT", "8790"))
EXPECT_KEY = os.environ.get("CONCENTRATE_STUB_EXPECT_KEY", "stub-key")
LOG = os.environ.get("CONCENTRATE_STUB_LOG", "")
REPLY_TEXT = os.environ.get("CONCENTRATE_STUB_REPLY", "ok from the concentrate stub")

# https://concentrate.ai/docs/api-reference/endpoint/request-parameters
DOCUMENTED_TOP_LEVEL = {
    "model", "input", "max_output_tokens", "temperature", "top_p", "stream",
    "text", "reasoning", "tools", "tool_choice", "parallel_tool_calls",
    "routing", "cache_control", "prompt_cache_options",
}

# A slice of the live catalog shape read on 2026-08-29 (ids are plain; the
# upstream provider lives in `owned_by`).
MODELS = [
    {"id": "deepseek-v4-pro", "object": "model", "owned_by": "deepseek", "type": "chat", "display_name": "DeepSeek V4 Pro"},
    {"id": "gpt-5.6-sol", "object": "model", "owned_by": "openai", "type": "chat", "display_name": "GPT-5.6 Sol"},
    {"id": "claude-fable-5", "object": "model", "owned_by": "anthropic", "type": "chat", "display_name": "Claude Fable 5"},
]


def log_event(record: dict) -> None:
    if not LOG:
        return
    with open(LOG, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(record) + "\n")


class Handler(BaseHTTPRequestHandler):
    server_version = "concentrate-stub/0.1"

    def log_message(self, fmt, *args):  # quiet by default; the driver reads the JSONL log
        if os.environ.get("CONCENTRATE_STUB_VERBOSE"):
            sys.stderr.write("%s - %s\n" % (self.address_string(), fmt % args))

    def _json(self, status: int, payload: dict | None, headers: dict | None = None) -> None:
        body = b"" if payload is None else json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        for key, value in (headers or {}).items():
            self.send_header(key, value)
        self.end_headers()
        if body:
            self.wfile.write(body)

    def do_GET(self):  # noqa: N802 (http.server API)
        path = self.path.split("?", 1)[0].rstrip("/")
        log_event({"method": "GET", "path": self.path, "authorization": self.headers.get("Authorization")})
        if path == "/v1/responses/health":
            # https://concentrate.ai/docs/api-reference/endpoint/health — 200, empty body, no auth.
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if path == "/v1/models":
            # https://concentrate.ai/docs/api-reference/endpoint/list-models — no auth required.
            self._json(200, {"object": "list", "data": MODELS})
            return
        self._json(404, {"error": "Not Found", "message": f"No route for {path}"})

    def do_POST(self):  # noqa: N802
        path = self.path.split("?", 1)[0].rstrip("/")
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b""
        try:
            body = json.loads(raw or b"{}")
        except json.JSONDecodeError:
            self._json(400, {"error": "Bad Request", "message": "Invalid JSON body"})
            return
        auth = self.headers.get("Authorization") or ""
        record = {
            "method": "POST",
            "path": self.path,
            "authorization": auth,
            "model": body.get("model"),
            "stream": body.get("stream"),
            "top_level_fields": sorted(body.keys()),
            "undocumented_fields": sorted(set(body.keys()) - DOCUMENTED_TOP_LEVEL),
            "input_roles": [item.get("role") for item in body.get("input", []) if isinstance(item, dict)],
            "tool_names": [tool.get("name") for tool in body.get("tools", []) if isinstance(tool, dict)],
        }
        log_event(record)
        if path != "/v1/responses":
            self._json(404, {"error": "Not Found", "message": f"No route for {path}"})
            return
        # https://concentrate.ai/docs/api-reference/endpoint/errors
        if auth != f"Bearer {EXPECT_KEY}":
            self._json(401, {"error": "Unauthorized", "message": "Invalid API key"})
            return
        if not body.get("model"):
            self._json(400, {"error": "Bad Request", "message": "Invalid model name: ''"})
            return
        if record["undocumented_fields"]:
            self._json(400, {"error": "Bad Request", "message": f"Invalid parameters: {record['undocumented_fields']}"})
            return
        if body.get("model") == "stub/insufficient-credits":
            self._json(402, {"error": "Insufficient funds", "message": "Your account has insufficient credits. Please add credits to continue."})
            return
        if not body.get("stream"):
            self._json(200, self._completed_response(body))
            return
        self._stream(body)

    def _completed_response(self, body: dict) -> dict:
        selected = body["model"] if "/" in body["model"] else f"stub/{body['model']}"
        return {
            "id": "resp_stub_1",
            "object": "response",
            "created_at": int(time.time()),
            "status": "completed",
            "model": selected,
            "output": [{
                "type": "message", "id": "msg_stub_1", "status": "completed", "role": "assistant",
                "content": [{"type": "output_text", "text": REPLY_TEXT, "annotations": []}],
            }],
            "usage": {"input_tokens": 12, "output_tokens": 5, "total_tokens": 17,
                      "input_tokens_details": {"cached_tokens": 0}},
        }

    def _stream(self, body: dict) -> None:
        # https://concentrate.ai/docs/api-reference/endpoint/streaming — typed
        # events, `event:` + `data:` frames, sequence numbers, no [DONE].
        response = self._completed_response(body)
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.end_headers()
        seq = 0

        def emit(event_type: str, payload: dict) -> None:
            nonlocal seq
            payload = {"type": event_type, "sequence_number": seq, **payload}
            seq += 1
            self.wfile.write(f"event: {event_type}\ndata: {json.dumps(payload)}\n\n".encode())
            self.wfile.flush()

        in_progress = {**response, "status": "in_progress", "output": [], "usage": None}
        emit("response.created", {"response": in_progress})
        emit("response.in_progress", {"response": in_progress})
        item = {"type": "message", "id": "msg_stub_1", "status": "in_progress", "role": "assistant", "content": []}
        emit("response.output_item.added", {"output_index": 0, "item": item})
        emit("response.content_part.added", {"item_id": "msg_stub_1", "output_index": 0, "content_index": 0,
                                             "part": {"type": "output_text", "text": "", "annotations": []}})
        words = REPLY_TEXT.split(" ")
        for index, word in enumerate(words):
            delta = word if index == len(words) - 1 else word + " "
            emit("response.output_text.delta", {"item_id": "msg_stub_1", "output_index": 0, "content_index": 0, "delta": delta})
        emit("response.output_text.done", {"item_id": "msg_stub_1", "output_index": 0, "content_index": 0, "text": REPLY_TEXT})
        emit("response.content_part.done", {"item_id": "msg_stub_1", "output_index": 0, "content_index": 0,
                                            "part": {"type": "output_text", "text": REPLY_TEXT, "annotations": []}})
        emit("response.output_item.done", {"output_index": 0, "item": response["output"][0]})
        emit("response.completed", {"response": response})


def main() -> int:
    server = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    print(f"concentrate-stub listening on http://127.0.0.1:{server.server_address[1]}/v1", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
