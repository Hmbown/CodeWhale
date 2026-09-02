#!/usr/bin/env python3
"""Protocol tests for the computer-use MCP server. No display needed:
failure paths and the JSON-RPC surface are pure; only screenshot and
screen_size touch the host, and the suite skips those without one."""

import json
import os
import subprocess
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import server


def call(method, params=None, msg_id=1):
    msg = {"jsonrpc": "2.0", "id": msg_id, "method": method}
    if params is not None:
        msg["params"] = params
    return server.handle(msg)


class HandshakeTest(unittest.TestCase):
    def test_initialize_advertises_tools(self):
        res = call("initialize", {"protocolVersion": "2024-11-05"})
        result = res["result"]
        self.assertEqual(result["protocolVersion"], "2024-11-05")
        self.assertIn("tools", result["capabilities"])

    def test_initialized_notification_is_silent(self):
        self.assertIsNone(
            server.handle({"jsonrpc": "2.0", "method": "notifications/initialized"})
        )

    def test_tools_list_names(self):
        res = call("tools/list")
        names = sorted(t["name"] for t in res["result"]["tools"])
        self.assertEqual(
            names,
            ["click", "press_key", "screen_size", "screenshot", "scroll", "type_text"],
        )
        for tool in res["result"]["tools"]:
            self.assertIn("inputSchema", tool)

    def test_unknown_method(self):
        res = call("resources/list")
        self.assertEqual(res["error"]["code"], -32601)

    def test_unknown_tool(self):
        res = call("tools/call", {"name": "nope", "arguments": {}})
        self.assertEqual(res["error"]["code"], -32602)


class ValidationTest(unittest.TestCase):
    def test_click_rejects_bad_coords(self):
        for args in ({"x": -1, "y": 5}, {"x": "a", "y": 1}, {}):
            res = call("tools/call", {"name": "click", "arguments": args})
            self.assertTrue(res["result"]["isError"], args)
            self.assertEqual(res["result"]["_meta"]["reason"], "bad_arguments")

    def test_type_rejects_empty_and_huge(self):
        for args in ({"text": ""}, {}, {"text": "x" * 2001}):
            res = call("tools/call", {"name": "type_text", "arguments": args})
            self.assertTrue(res["result"]["isError"], str(args)[:20])

    def test_press_key_allowlist(self):
        res = call("tools/call", {"name": "press_key", "arguments": {"key": ""}})
        self.assertTrue(res["result"]["isError"])
        if server._ON_MAC:
            res = call(
                "tools/call", {"name": "press_key", "arguments": {"key": "f13"}}
            )
            self.assertEqual(res["result"]["_meta"]["reason"], "bad_arguments")


class LiveWireTest(unittest.TestCase):
    def test_stdio_round_trip(self):
        proc = subprocess.run(
            [sys.executable, os.path.join(os.path.dirname(__file__), "server.py")],
            input=(
                '{"jsonrpc":"2.0","id":1,"method":"initialize",'
                '"params":{"protocolVersion":"2024-11-05"}}\n'
                '{"jsonrpc":"2.0","id":2,"method":"tools/list"}\n'
            ),
            capture_output=True,
            text=True,
            timeout=30,
        )
        lines = [json.loads(line) for line in proc.stdout.splitlines() if line.strip()]
        self.assertEqual(len(lines), 2)
        self.assertEqual(lines[0]["result"]["protocolVersion"], "2024-11-05")
        self.assertEqual(len(lines[1]["result"]["tools"]), 6)


if __name__ == "__main__":
    unittest.main()
