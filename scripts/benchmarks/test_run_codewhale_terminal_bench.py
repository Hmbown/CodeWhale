#!/usr/bin/env python3
"""Focused tests for the CodeWhale Terminal-Bench summary layer."""

from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve()
RUNNER = SCRIPT.with_name("run-codewhale-terminal-bench.py")


def load_runner():
    spec = importlib.util.spec_from_file_location("codewhale_tbench_runner", RUNNER)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"unable to load {RUNNER}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


runner = load_runner()


class CodeWhaleTerminalBenchSummaryTests(unittest.TestCase):
    def test_readiness_probe_uses_task_specific_predicate(self) -> None:
        probe = runner.readiness_probe_for_task("terminal-bench/qemu-alpine-ssh")
        self.assertIsNotNone(probe)
        self.assertIn("login:", probe)
        self.assertIn("nc -w 5 127.0.0.1 6665", probe)

    def test_repeated_denied_tool_calls_classify_as_tool_policy_loop(self) -> None:
        row = {
            "reward": 0,
            "exception": None,
            "verifier_exception": None,
            "denied_tool_counts": {"grep_files": 3},
        }

        self.assertEqual(runner.classify_failure(row), "tool_policy_loop")
        self.assertEqual(row["denied_tool"], "grep_files")
        self.assertEqual(row["denied_tool_repeat_count"], 3)

    def test_artifact_preflight_errors_classify_as_artifact_incompatible(self) -> None:
        row = {
            "reward": None,
            "exception": "RuntimeError: error while loading shared libraries: libssl.so.3: cannot open shared object file",
            "verifier_exception": None,
            "denied_tool_counts": {},
        }

        self.assertEqual(runner.classify_failure(row), "artifact_incompatible")

    def test_parse_trial_preserves_failure_class_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            trial = Path(tmp) / "codewhale__qemu-alpine-ssh"
            agent_dir = trial / "agent"
            agent_dir.mkdir(parents=True)
            (trial / "result.json").write_text(
                json.dumps(
                    {
                        "task_name": "qemu-alpine-ssh",
                        "started_at": "2026-06-21T00:00:00Z",
                        "finished_at": "2026-06-21T00:01:00Z",
                        "agent_result": {"n_input_tokens": 10, "n_output_tokens": 2},
                        "verifier_result": {"rewards": {"reward": 0}},
                    }
                )
            )
            (agent_dir / "codewhale.txt").write_text(
                "\n".join(
                    [
                        "tool denied: grep_files is not available",
                        "tool denied: grep_files is not available",
                        "tool denied: grep_files is not available",
                    ]
                )
            )
            (agent_dir / "codewhale-artifact-preflight.txt").write_text(
                "codewhale 0.8.63\n"
            )
            (agent_dir / "codewhale-harness-note.txt").write_text("Benchmark harness note\n")

            row = runner.parse_trial(trial, "deepseek/deepseek-v4-flash")

        self.assertIsNotNone(row)
        assert row is not None
        self.assertEqual(row["failure_class"], "tool_policy_loop")
        self.assertEqual(row["denied_tool"], "grep_files")
        self.assertIn("login:", row["readiness_probe"])
        self.assertIsNotNone(row["artifact_preflight_path"])
        self.assertIsNotNone(row["harness_note_path"])

    def test_markdown_includes_failure_class_columns(self) -> None:
        rows = [
            {
                "model": "m",
                "reasoning_effort": None,
                "task": "t",
                "reward": 0,
                "failure_class": "background_not_ready",
                "denied_tool": None,
                "denied_tool_repeat_count": 0,
                "exception": None,
                "runtime_s": 1,
                "input_tokens": 1,
                "output_tokens": 1,
                "transcript_path": "log.txt",
            }
        ]
        text = runner.markdown(rows, runner.aggregate(rows))

        self.assertIn("failure classes", text)
        self.assertIn("failure class", text)
        self.assertIn("background_not_ready", text)


if __name__ == "__main__":
    unittest.main()
