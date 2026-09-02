#!/usr/bin/env python3
"""codewhale computer-use MCP server (stdlib only, MCP 2024-11-05, stdio).

Tools: screen_size, screenshot, click, type_text, press_key, scroll.
Platform backends: macOS via screencapture + osascript, Linux via
grim/import + ydotool/wtype. Anything unavailable fails as an MCP
isError result with a machine-readable `reason` so the model can relay
it instead of retrying blind.

Protocol: newline-delimited JSON-RPC on stdin/stdout. Log to stderr;
stdout is the wire.
"""

from __future__ import annotations

import base64
import json
import os
import platform
import shutil
import struct
import subprocess
import sys
import tempfile

PROTOCOL_VERSION = "2024-11-05"
SERVER_NAME = "computer-use"
SERVER_VERSION = "0.1.0"
TOOL_TIMEOUT_SECS = 30
MAX_TYPE_CHARS = 2000

_ON_MAC = platform.system() == "Darwin"
_ON_LINUX = platform.system() == "Linux"

_size_cache: list | None = None
# full-display pixels per screenshot pixel. The model reads positions
# from the (possibly downscaled) screenshot; click scales back up.
_shot_scale: float = 1.0


def log(msg: str) -> None:
    sys.stderr.write(f"[computer-use] {msg}\n")
    sys.stderr.flush()


def err(reason: str, message: str) -> dict:
    return {
        "content": [{"type": "text", "text": message}],
        "isError": True,
        "_meta": {"reason": reason},
    }


def ok_text(text: str) -> dict:
    return {"content": [{"type": "text", "text": text}]}


def run(argv: list, **kwargs) -> subprocess.CompletedProcess:
    return subprocess.run(
        argv, capture_output=True, timeout=TOOL_TIMEOUT_SECS, **kwargs
    )


def need_tool(name: str) -> str | None:
    path = shutil.which(name)
    if path is None:
        return None
    return path


def png_size(path: str) -> tuple | None:
    try:
        with open(path, "rb") as fh:
            header = fh.read(26)
        if header[:8] != b"\x89PNG\r\n\x1a\n":
            return None
        width, height = struct.unpack(">II", header[16:24])
        return [width, height]
    except OSError:
        return None


def mac_screenshot(dest: str) -> str | None:
    if need_tool("screencapture") is None:
        return "Screen capture helper `screencapture` is not on PATH."
    proc = run(["screencapture", "-x", "-t", "png", "-m", dest])
    if proc.returncode != 0:
        detail = proc.stderr.decode("utf-8", "replace").strip()
        if "not permitted" in detail or "permission" in detail.lower():
            return (
                "Screen Recording permission is missing. Open System "
                "Settings > Privacy & Security > Screen Recording and "
                "enable the terminal running codewhale, then try again."
            )
        return f"screencapture failed: {detail or proc.returncode}"
    return None


def linux_screenshot(dest: str) -> str | None:
    for helper, argv in (
        ("grim", ["grim", dest]),
        ("import", ["import", "-window", "root", dest]),
        ("scrot", ["scrot", "--overwrite", dest]),
    ):
        if need_tool(helper) is None:
            continue
        proc = run(argv)
        if proc.returncode == 0:
            return None
        log(f"{helper} failed, trying next backend")
    return (
        "No working screenshot helper found. Install one of grim, "
        "ImageMagick import, or scrot."
    )


MAX_SCREENSHOT_WIDTH = 1920


def shrink_for_vision(png_path: str) -> tuple:
    """Downscale to a vision-sized JPEG with native platform tools.

    Returns (path, mime, size) of the best artifact, falling back to the
    original PNG when no resizer exists. Retina PNGs run to tens of
    megabytes; vision models read 1920px JPEGs fine at a tenth the cost.
    """
    size = png_size(png_path)
    if _ON_MAC and need_tool("sips") is not None:
        with tempfile.NamedTemporaryFile(suffix=".jpg", delete=False) as tmp:
            small = tmp.name
        try:
            proc = run(
                [
                    "sips",
                    "-Z",
                    str(MAX_SCREENSHOT_WIDTH),
                    "-s",
                    "format",
                    "jpeg",
                    "-s",
                    "formatOptions",
                    "80",
                    png_path,
                    "--out",
                    small,
                ]
            )
            if proc.returncode == 0:
                jpeg_size = png_size(small)
                if jpeg_size is None:  # JPEG has no PNG header; trust sips
                    width = min(size[0], MAX_SCREENSHOT_WIDTH) if size else 0
                    height = (
                        round(size[1] * width / size[0])
                        if size and size[0]
                        else 0
                    )
                    return small, "image/jpeg", [width, height]
        except Exception:
            pass
        try:
            os.unlink(small)
        except OSError:
            pass
    if _ON_LINUX and need_tool("convert") is not None:
        with tempfile.NamedTemporaryFile(suffix=".jpg", delete=False) as tmp:
            small = tmp.name
        try:
            proc = run(
                [
                    "convert",
                    png_path,
                    "-resize",
                    f"{MAX_SCREENSHOT_WIDTH}>",
                    "-quality",
                    "80",
                    small,
                ]
            )
            if proc.returncode == 0:
                width = min(size[0], MAX_SCREENSHOT_WIDTH) if size else 0
                height = (
                    round(size[1] * width / size[0]) if size and size[0] else 0
                )
                return small, "image/jpeg", [width, height]
        except Exception:
            pass
        try:
            os.unlink(small)
        except OSError:
            pass
    return png_path, "image/png", size or [0, 0]


def do_screenshot() -> dict:
    if not (_ON_MAC or _ON_LINUX):
        return err(
            "unsupported_platform",
            f"Computer use supports macOS and Linux; this host is {platform.system()}.",
        )
    with tempfile.NamedTemporaryFile(suffix=".png", delete=False) as tmp:
        dest = tmp.name
    small: str | None = None
    try:
        if _ON_MAC:
            problem = mac_screenshot(dest)
        else:
            problem = linux_screenshot(dest)
        if problem is not None:
            reason = "missing_permission" if "permission" in problem else "missing_helper"
            return err(reason, problem)
        full = png_size(dest)
        if full is None:
            return err("capture_failed", "Screenshot helper produced no readable PNG.")
        small, mime, size = shrink_for_vision(dest)
        global _size_cache, _shot_scale
        _size_cache = size
        _shot_scale = (full[0] / size[0]) if size[0] else 1.0
        with open(small, "rb") as fh:
            data = base64.b64encode(fh.read()).decode("ascii")
        return {
            "content": [
                {
                    "type": "image",
                    "data": data,
                    "mimeType": mime,
                },
                {
                    "type": "text",
                    "text": f"Screenshot {size[0]}x{size[1]}.",
                },
            ]
        }
    finally:
        for path in (dest, small):
            if path is not None:
                try:
                    os.unlink(path)
                except OSError:
                    pass


def do_screen_size() -> dict:
    global _size_cache
    if _size_cache is not None:
        width, height = _size_cache
        return ok_text(f'{{"width": {width}, "height": {height}}}')
    if _ON_MAC and need_tool("screencapture") is not None:
        with tempfile.NamedTemporaryFile(suffix=".png", delete=False) as tmp:
            dest = tmp.name
        try:
            problem = mac_screenshot(dest)
            if problem is not None:
                return err("missing_helper", problem)
            size = png_size(dest)
        finally:
            try:
                os.unlink(dest)
            except OSError:
                pass
        if size is None:
            return err("capture_failed", "Could not determine the screen size.")
        _size_cache = size
        return ok_text(f'{{"width": {size[0]}, "height": {size[1]}}}')
    return err(
        "unknown_size" if (_ON_MAC or _ON_LINUX) else "unsupported_platform",
        "Screen size is unknown until the first screenshot on this platform. "
        "Call screenshot first.",
    )


def osascript(script: str) -> tuple:
    with tempfile.NamedTemporaryFile(
        suffix=".scpt", mode="w", delete=False, encoding="utf-8"
    ) as tmp:
        tmp.write(script)
        path = tmp.name
    try:
        proc = run(["osascript", path])
    finally:
        try:
            os.unlink(path)
        except OSError:
            pass
    return proc.returncode, proc.stderr.decode("utf-8", "replace").strip()


def mac_click(x: int, y: int) -> str | None:
    if need_tool("osascript") is None:
        return "Missing helper: osascript is not on PATH."
    code, detail = osascript(
        f'tell application "System Events" to click at {{{x}, {y}}}'
    )
    if code != 0:
        if "not permitted" in detail or "1002" in detail:
            return (
                "Accessibility permission is missing. Open System Settings "
                "> Privacy & Security > Accessibility and enable the "
                "terminal running codewhale, then try again."
            )
        return f"Click failed: {detail or code}"
    return None


def linux_type_backend() -> str | None:
    if need_tool("ydotool") is not None:
        return "ydotool"
    if need_tool("wtype") is not None:
        return "wtype"
    return None


def do_click(args: dict) -> dict:
    try:
        x, y = int(args["x"]), int(args["y"])
    except (KeyError, TypeError, ValueError):
        return err("bad_arguments", "click needs integer x and y pixel coordinates.")
    if x < 0 or y < 0:
        return err("bad_arguments", "click coordinates must be non-negative.")
    # Coordinates arrive in screenshot space; scale to display pixels.
    x, y = round(x * _shot_scale), round(y * _shot_scale)
    if _ON_MAC:
        problem = mac_click(x, y)
        if problem is not None:
            reason = "missing_permission" if "permission" in problem else "missing_helper"
            if problem.startswith("Click failed"):
                reason = "action_failed"
            return err(reason, problem)
        return ok_text(f"Clicked at {x},{y}.")
    if _ON_LINUX:
        backend = linux_type_backend()
        if backend == "ydotool":
            proc = run(["ydotool", "mousemove", str(x), str(y), "click", "1"])
        elif backend == "wtype":
            return err(
                "missing_helper",
                "wtype moves no mouse. Install ydotool for clicking on Linux.",
            )
        else:
            return err(
                "missing_helper",
                "No input helper found on Linux. Install ydotool (X11/Wayland) "
                "or wtype (Wayland typing only).",
            )
        if proc.returncode != 0:
            return err("action_failed", f"Click failed: {proc.stderr.decode()[:200]}")
        return ok_text(f"Clicked at {x},{y}.")
    return err(
        "unsupported_platform",
        f"Clicking is supported on macOS and Linux; this host is {platform.system()}.",
    )


def do_type_text(args: dict) -> dict:
    text = args.get("text", "")
    if not isinstance(text, str) or not text:
        return err("bad_arguments", "type_text needs a non-empty text string.")
    if len(text) > MAX_TYPE_CHARS:
        return err(
            "bad_arguments",
            f"type_text accepts at most {MAX_TYPE_CHARS} characters per call.",
        )
    if _ON_MAC:
        if need_tool("osascript") is None:
            return err("missing_helper", "Missing helper: osascript is not on PATH.")
        escaped = text.replace("\\", "\\\\").replace('"', '\\"')
        code, detail = osascript(
            f'tell application "System Events" to keystroke "{escaped}"'
        )
        if code != 0:
            if "not permitted" in detail or "1002" in detail:
                return err(
                    "missing_permission",
                    "Accessibility permission is missing. Open System Settings "
                    "> Privacy & Security > Accessibility and enable the "
                    "terminal running codewhale, then try again.",
                )
            return err("action_failed", f"Typing failed: {detail or code}")
        return ok_text(f"Typed {len(text)} characters.")
    if _ON_LINUX:
        backend = linux_type_backend()
        if backend == "ydotool":
            proc = run(["ydotool", "type", "--", text])
        elif backend == "wtype":
            proc = run(["wtype", "--", text])
        else:
            return err(
                "missing_helper",
                "No input helper found on Linux. Install ydotool or wtype.",
            )
        if proc.returncode != 0:
            return err("action_failed", f"Typing failed: {proc.stderr.decode()[:200]}")
        return ok_text(f"Typed {len(text)} characters.")
    return err(
        "unsupported_platform",
        f"Typing is supported on macOS and Linux; this host is {platform.system()}.",
    )


MAC_KEYS = {
    "return": "keystroke return",
    "enter": "keystroke return",
    "tab": "keystroke tab",
    "escape": "key code 53",
    "esc": "key code 53",
    "space": "keystroke space",
    "delete": "key code 51",
    "up": "key code 126",
    "down": "key code 125",
    "left": "key code 123",
    "right": "key code 124",
}


def do_press_key(args: dict) -> dict:
    key = str(args.get("key", "")).strip().lower()
    if not key:
        return err("bad_arguments", "press_key needs a key name such as return or tab.")
    if _ON_MAC:
        if key not in MAC_KEYS:
            return err(
                "bad_arguments",
                f"Unsupported key '{key}'. Supported: "
                + ", ".join(sorted(MAC_KEYS)),
            )
        code, detail = osascript(
            f'tell application "System Events" to {MAC_KEYS[key]}'
        )
        if code != 0:
            return err("action_failed", f"Key press failed: {detail or code}")
        return ok_text(f"Pressed {key}.")
    if _ON_LINUX:
        if need_tool("ydotool") is None:
            return err("missing_helper", "press_key on Linux needs ydotool.")
        proc = run(["ydotool", "key", key])
        if proc.returncode != 0:
            return err("action_failed", f"Key press failed: {proc.stderr.decode()[:200]}")
        return ok_text(f"Pressed {key}.")
    return err(
        "unsupported_platform",
        f"Key presses are supported on macOS and Linux; this host is {platform.system()}.",
    )


def do_scroll(args: dict) -> dict:
    if _ON_LINUX and need_tool("ydotool") is not None:
        try:
            dx, dy = int(args.get("dx", 0)), int(args.get("dy", 0))
        except (TypeError, ValueError):
            return err("bad_arguments", "scroll needs integer dx/dy.")
        proc = run(["ydotool", "mousemove", "--", str(dx), str(dy)])
        if proc.returncode != 0:
            return err("action_failed", "Scroll failed.")
        return ok_text(f"Scrolled by {dx},{dy}.")
    if _ON_MAC:
        return err(
            "unsupported_platform",
            "Scrolling is not implemented on macOS in computer-use 0.1.0. "
            "Use press_key (space, up, down) to move through scrollable content.",
        )
    return err(
        "unsupported_platform",
        f"Scrolling is supported on Linux with ydotool; this host is {platform.system()}.",
    )


TOOLS = {
    "screen_size": {
        "description": "Report the main display size in pixels. Call before clicking.",
        "inputSchema": {"type": "object", "properties": {}},
    },
    "screenshot": {
        "description": "Capture the main display as a PNG vision block.",
        "inputSchema": {"type": "object", "properties": {}},
    },
    "click": {
        "description": "Click at screenshot pixels from the top-left (server scales to the display).",
        "inputSchema": {
            "type": "object",
            "properties": {
                "x": {"type": "integer"},
                "y": {"type": "integer"},
            },
            "required": ["x", "y"],
        },
    },
    "type_text": {
        "description": "Type text into the focused field. Never secrets.",
        "inputSchema": {
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"],
        },
    },
    "press_key": {
        "description": "Press one named key: return, tab, escape, space, delete, up, down, left, right.",
        "inputSchema": {
            "type": "object",
            "properties": {"key": {"type": "string"}},
            "required": ["key"],
        },
    },
    "scroll": {
        "description": "Scroll by dx/dy (Linux with ydotool only).",
        "inputSchema": {
            "type": "object",
            "properties": {
                "dx": {"type": "integer"},
                "dy": {"type": "integer"},
            },
        },
    },
}

HANDLERS = {
    "screen_size": lambda args: do_screen_size(),
    "screenshot": lambda args: do_screenshot(),
    "click": do_click,
    "type_text": do_type_text,
    "press_key": do_press_key,
    "scroll": do_scroll,
}


def handle(msg: dict):
    msg_id = msg.get("id")
    method = msg.get("method")

    def reply(result: dict) -> dict:
        return {"jsonrpc": "2.0", "id": msg_id, "result": result}

    def error(code: int, message: str) -> dict:
        return {
            "jsonrpc": "2.0",
            "id": msg_id,
            "error": {"code": code, "message": message},
        }

    if method == "initialize":
        return reply(
            {
                "protocolVersion": PROTOCOL_VERSION,
                "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION},
                "capabilities": {"tools": {}},
            }
        )
    if method in ("notifications/initialized", "notifications/cancelled"):
        return None
    if method == "ping":
        return reply({})
    if method == "tools/list":
        return reply(
            {
                "tools": [
                    {
                        "name": name,
                        "description": spec["description"],
                        "inputSchema": spec["inputSchema"],
                    }
                    for name, spec in TOOLS.items()
                ]
            }
        )
    if method == "tools/call":
        params = msg.get("params", {}) or {}
        name = params.get("name", "")
        args = params.get("arguments", {}) or {}
        handler = HANDLERS.get(name)
        if handler is None:
            return error(-32602, f"Unknown tool '{name}'.")
        try:
            return reply(handler(args))
        except subprocess.TimeoutExpired:
            return reply(err("timed_out", f"Tool '{name}' timed out."))
        except Exception as exc:  # never break the wire
            log(f"tool {name} crashed: {exc}")
            return reply(err("internal", f"Tool '{name}' failed unexpectedly."))
    return error(-32601, f"Unsupported method '{method}'.")


def main() -> None:
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        try:
            response = handle(msg if isinstance(msg, dict) else {})
        except Exception as exc:  # never break the wire
            log(f"dispatch crashed: {exc}")
            continue
        if response is not None:
            sys.stdout.write(json.dumps(response) + "\n")
            sys.stdout.flush()


if __name__ == "__main__":
    main()
