#!/usr/bin/env python3
"""Measure serialized tool catalog size before and after default deferral.

This delegates catalog construction to an ignored Rust test so the measurement
uses the same tool definitions, JSON serialization, and deferral policy as the
runtime. Token counts are deterministic estimates using ceil(serialized_bytes/4).
"""

from __future__ import annotations

import json
import subprocess
import sys


MARKER = "TOOL_CATALOG_METRICS "


def main() -> int:
    cmd = [
        "cargo",
        "test",
        "-p",
        "codewhale-tui",
        "print_agent_tool_catalog_metrics",
        "--",
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ]
    proc = subprocess.run(cmd, text=True, capture_output=True, check=False)
    sys.stderr.write(proc.stderr)

    for line in proc.stdout.splitlines():
        if MARKER in line:
            metrics = json.loads(line.split(MARKER, 1)[1])
            print(json.dumps(metrics, indent=2, sort_keys=True))
            return proc.returncode

    sys.stdout.write(proc.stdout)
    sys.stderr.write("missing TOOL_CATALOG_METRICS marker\n")
    return proc.returncode or 1


if __name__ == "__main__":
    raise SystemExit(main())
