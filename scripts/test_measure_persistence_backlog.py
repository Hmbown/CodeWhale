#!/usr/bin/env python3
"""Hermetic tests for scripts/measure-persistence-backlog.py."""

from __future__ import annotations

import importlib.util
import io
import json
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "measure-persistence-backlog.py"

SPEC = importlib.util.spec_from_file_location("measure_persistence_backlog", SCRIPT)
assert SPEC and SPEC.loader
mod = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = mod
SPEC.loader.exec_module(mod)


class PersistenceBacklogMeasurementTests(unittest.TestCase):
    def test_runner_targets_exact_library_test(self) -> None:
        completed = subprocess.CompletedProcess(
            args=[],
            returncode=0,
            stdout=(
                "running 1 test\n"
                f"test {mod.TEST_NAME} ... ok\n\n"
                "test result: ok. 1 passed; 0 failed; 1 ignored; 0 measured; "
                "0 filtered out\n"
            ),
            stderr="",
        )
        receipt = {"document_kind": "fixture"}

        with tempfile.TemporaryDirectory() as root:
            receipt_path = Path(root) / "receipt.json"
            receipt_path.write_text(json.dumps(receipt), encoding="utf-8")
            with mock.patch.object(
                mod.subprocess, "run", return_value=completed
            ) as run:
                measured = mod.run_measurement(receipt_path, {"FIXTURE": "1"})

        self.assertEqual(measured, receipt)
        self.assertEqual(
            run.call_args.args[0],
            [
                "cargo",
                "test",
                "--locked",
                "--workspace",
                "--all-features",
                "--lib",
                mod.TEST_NAME,
                "--",
                "--exact",
                "--ignored",
                "--test-threads=1",
            ],
        )
        self.assertEqual(run.call_args.kwargs["cwd"], mod.ROOT)

    def test_workspace_zero_test_packages_do_not_hide_the_exact_test(self) -> None:
        completed = subprocess.CompletedProcess(
            args=[],
            returncode=0,
            stdout=(
                "running 0 tests\n\n"
                "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; "
                "3 filtered out\n\n"
                "running 1 test\n"
                f"test {mod.TEST_NAME} ... ok\n\n"
                "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; "
                "12 filtered out\n"
            ),
            stderr="",
        )
        receipt = {"document_kind": "workspace-fixture"}

        with tempfile.TemporaryDirectory() as root:
            receipt_path = Path(root) / "receipt.json"
            receipt_path.write_text(json.dumps(receipt), encoding="utf-8")
            with mock.patch.object(mod.subprocess, "run", return_value=completed):
                measured = mod.run_measurement(receipt_path, {})

        self.assertEqual(measured, receipt)

    def test_successful_zero_test_run_is_rejected(self) -> None:
        completed = subprocess.CompletedProcess(
            args=[],
            returncode=0,
            stdout=(
                "running 0 tests\n\n"
                "test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; "
                "1 filtered out\n"
            ),
            stderr="",
        )

        with tempfile.TemporaryDirectory() as root:
            receipt_path = Path(root) / "receipt.json"
            receipt_path.write_text("{}", encoding="utf-8")
            with (
                mock.patch.object(mod.subprocess, "run", return_value=completed),
                redirect_stdout(io.StringIO()),
                self.assertRaisesRegex(
                    mod.PersistenceBacklogMeasurementError, "ran zero tests"
                ),
            ):
                mod.run_measurement(receipt_path, {})

    def test_successful_test_without_receipt_is_rejected(self) -> None:
        completed = subprocess.CompletedProcess(
            args=[],
            returncode=0,
            stdout=(
                "running 1 test\n"
                f"test {mod.TEST_NAME} ... ok\n\n"
                "test result: ok. 1 passed; 0 failed; 1 ignored; 0 measured; "
                "0 filtered out\n"
            ),
            stderr="",
        )

        with tempfile.TemporaryDirectory() as root:
            with (
                mock.patch.object(mod.subprocess, "run", return_value=completed),
                self.assertRaisesRegex(
                    mod.PersistenceBacklogMeasurementError,
                    "emitted no valid receipt",
                ),
            ):
                mod.run_measurement(Path(root) / "missing.json", {})


if __name__ == "__main__":
    raise SystemExit(unittest.main())
