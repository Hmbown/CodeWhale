#!/usr/bin/env python3
"""Smoke-test the mobile runtime server with real HTTP requests."""

from __future__ import annotations

import argparse
import json
import os
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path


TOKEN = "mobile smoke token + /?=&%"


def free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def request(
    url: str,
    *,
    method: str = "GET",
    token: str | None = None,
    body: bytes | None = None,
) -> tuple[int, str]:
    headers = {}
    if token is not None:
        headers["Authorization"] = f"Bearer {token}"
    if body is not None:
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(url, data=body, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=5) as response:
            return response.status, response.read().decode("utf-8", errors="replace")
    except urllib.error.HTTPError as exc:
        return exc.code, exc.read().decode("utf-8", errors="replace")


def wait_for_server(url: str, proc: subprocess.Popen[str]) -> None:
    deadline = time.monotonic() + 30
    last_error: Exception | None = None
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(f"server exited early with code {proc.returncode}")
        try:
            request(url)
            return
        except Exception as exc:  # noqa: BLE001 - surface the last startup failure.
            last_error = exc
            time.sleep(0.25)
    raise RuntimeError(f"server did not become reachable: {last_error}")


def start_server(binary: Path, *args: str) -> subprocess.Popen[str]:
    env = os.environ.copy()
    env.setdefault("NO_COLOR", "1")
    return subprocess.Popen(
        [str(binary), "serve", *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
        env=env,
    )


def stop_server(proc: subprocess.Popen[str]) -> tuple[str, str]:
    if proc.poll() is None:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
    stdout, stderr = proc.communicate(timeout=5)
    return stdout, stderr


def assert_status(name: str, actual: int, expected: int) -> None:
    if actual != expected:
        raise AssertionError(f"{name}: expected HTTP {expected}, got {actual}")


def auth_smoke(binary: Path) -> dict[str, object]:
    port = free_port()
    proc = start_server(
        binary,
        "--mobile",
        "--host",
        "127.0.0.1",
        "--port",
        str(port),
        "--auth-token",
        TOKEN,
    )
    try:
        base = f"http://127.0.0.1:{port}"
        wait_for_server(f"{base}/mobile", proc)
        mobile_no_token, _ = request(f"{base}/mobile")
        mobile_with_token, html = request(f"{base}/mobile", token=TOKEN)
        v1_no_token, _ = request(f"{base}/v1/threads/summary?limit=1")
        v1_with_token, threads = request(f"{base}/v1/threads/summary?limit=1", token=TOKEN)
        approval_status, approval_body = request(
            f"{base}/v1/approvals/no_such_id",
            method="POST",
            token=TOKEN,
            body=b'{"decision":"allow","remember":false}',
        )

        assert_status("mobile without token", mobile_no_token, 401)
        assert_status("mobile with bearer token", mobile_with_token, 200)
        assert_status("v1 without token", v1_no_token, 401)
        assert_status("v1 with bearer token", v1_with_token, 200)
        assert_status("approval retry missing id", approval_status, 404)
        if "<title>CodeWhale Mobile</title>" not in html:
            raise AssertionError("mobile page did not include expected title")
        if json.loads(threads) != []:
            raise AssertionError("expected empty thread summary in smoke workspace")
        if "no pending approval" not in approval_body:
            raise AssertionError("approval 404 did not include useful error body")
        return {
            "mobile_no_token": mobile_no_token,
            "mobile_with_token": mobile_with_token,
            "v1_no_token": v1_no_token,
            "v1_with_token": v1_with_token,
            "approval_retry_missing_id": approval_status,
        }
    finally:
        stop_server(proc)


def insecure_smoke(binary: Path) -> dict[str, object]:
    port = free_port()
    proc = start_server(
        binary,
        "--mobile",
        "--host",
        "127.0.0.1",
        "--port",
        str(port),
        "--insecure",
    )
    try:
        base = f"http://127.0.0.1:{port}"
        wait_for_server(f"{base}/mobile", proc)
        mobile_status, html = request(f"{base}/mobile")
        threads_status, threads = request(f"{base}/v1/threads/summary?limit=1")
        assert_status("insecure mobile page", mobile_status, 200)
        assert_status("insecure v1 route", threads_status, 200)
        if "<title>CodeWhale Mobile</title>" not in html:
            raise AssertionError("insecure mobile page did not include expected title")
        if json.loads(threads) != []:
            raise AssertionError("expected empty thread summary in insecure smoke workspace")
        return {"mobile": mobile_status, "v1_threads": threads_status}
    finally:
        stop_server(proc)


def lan_warning_smoke(binary: Path) -> dict[str, object]:
    port = free_port()
    proc = start_server(binary, "--mobile", "--port", str(port), "--insecure")
    try:
        wait_for_server(f"http://127.0.0.1:{port}/mobile", proc)
    finally:
        stdout, stderr = stop_server(proc)
    output = f"{stdout}\n{stderr}"
    if f"http://0.0.0.0:{port}" not in output:
        raise AssertionError("mobile default did not bind to 0.0.0.0")
    if "WARNING: --mobile is binding to 0.0.0.0" not in output:
        raise AssertionError("missing LAN binding warning")
    if "LAN:" not in output:
        raise AssertionError("missing LAN URL hint")
    return {"binds_0_0_0_0": True, "warning": True, "lan_hint": True}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "binary",
        nargs="?",
        default=str(Path("target") / "debug" / ("codewhale-tui.exe" if os.name == "nt" else "codewhale-tui")),
        help="Path to the built codewhale-tui binary",
    )
    args = parser.parse_args()
    binary = Path(args.binary)
    if not binary.exists():
        raise SystemExit(f"binary not found: {binary}")

    result = {
        "auth": auth_smoke(binary),
        "insecure": insecure_smoke(binary),
        "lan_warning": lan_warning_smoke(binary),
    }
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
