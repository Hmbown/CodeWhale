#!/usr/bin/env python3
"""Measure the paused-consumer persistence-channel backlog hermetically."""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path


RECEIPT_ENV = "CODEWHALE_TEST_PERSISTENCE_BACKLOG_RECEIPT_PATH"
SOURCE_SHA_ENV = "CODEWHALE_TEST_PERSISTENCE_BACKLOG_SOURCE_SHA"
SOURCE_DIRTY_ENV = "CODEWHALE_TEST_PERSISTENCE_BACKLOG_SOURCE_DIRTY"
RUSTC_VERSION_ENV = "CODEWHALE_TEST_PERSISTENCE_BACKLOG_RUSTC_VERSION"
CARGO_VERSION_ENV = "CODEWHALE_TEST_PERSISTENCE_BACKLOG_CARGO_VERSION"
ROOT = Path(__file__).resolve().parent.parent
TEST_NAME = (
    "tui::persistence_actor::backlog_measurement_tests::"
    "write_paused_persistence_backlog_measurement_receipt"
)


class PersistenceBacklogMeasurementError(RuntimeError):
    """The exact measurement test did not produce a trustworthy receipt."""


def measurement_command() -> list[str]:
    return [
        "cargo",
        "test",
        "--locked",
        # Match CI's `cargo nextest run --workspace --all-features` feature
        # unification. `-p codewhale-tui --all-features` is not the same:
        # sibling crates can enable extra features on shared deps, so this
        # step relinked codewhale-tui (and telemetry/build-support) for ~13
        # minutes on macos-latest and then timed enqueue on a just-linked
        # binary. That sample was enqueue_elapsed_ns=43684497 against a
        # 25ms ceiling after the suite itself had already passed.
        "--workspace",
        "--all-features",
        "--lib",
        TEST_NAME,
        "--",
        "--exact",
        "--ignored",
        "--test-threads=1",
    ]


def run_measurement(receipt_path: Path, env: dict[str, str]) -> dict:
    result = subprocess.run(
        measurement_command(),
        cwd=ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )
    sys.stderr.write(result.stderr)
    if result.returncode != 0:
        sys.stdout.write(result.stdout)
        result.check_returncode()

    combined = "\n".join(result.stdout.splitlines() + result.stderr.splitlines())
    combined = re.sub(r"\x1b\[[0-9;]*m", "", combined)
    # `--workspace --lib` runs every crate's library tests. Packages that
    # do not contain TEST_NAME print "running 0 tests"; that is expected
    # and must not be treated as a missed measurement.
    test_status = re.search(
        rf"test {re.escape(TEST_NAME)} \.\.\. (ok|FAILED|ignored)\b",
        combined,
    )
    if test_status is None or test_status.group(1) != "ok":
        sys.stdout.write(result.stdout)
        raise PersistenceBacklogMeasurementError(
            f"exact library measurement test {TEST_NAME} ran zero tests"
        )
    try:
        return json.loads(receipt_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise PersistenceBacklogMeasurementError(
            f"exact library measurement test {TEST_NAME} emitted no valid receipt: {error}"
        ) from error


def main() -> int:
    source_sha = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=True,
    ).stdout.strip()
    source_dirty = bool(
        subprocess.run(
            ["git", "status", "--porcelain", "--untracked-files=normal"],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=True,
        ).stdout
    )
    rustc_version = subprocess.run(
        ["rustc", "--version"], text=True, capture_output=True, check=True
    ).stdout.strip()
    cargo_version = subprocess.run(
        ["cargo", "--version"], text=True, capture_output=True, check=True
    ).stdout.strip()
    with tempfile.TemporaryDirectory(prefix="codewhale-persistence-backlog-") as root:
        receipt_path = Path(root) / "receipt.json"
        env = os.environ.copy()
        env["CARGO_NET_OFFLINE"] = "true"
        env[RECEIPT_ENV] = str(receipt_path)
        env[SOURCE_SHA_ENV] = source_sha
        env[SOURCE_DIRTY_ENV] = str(source_dirty).lower()
        env[RUSTC_VERSION_ENV] = rustc_version
        env[CARGO_VERSION_ENV] = cargo_version
        try:
            receipt = run_measurement(receipt_path, env)
        except subprocess.CalledProcessError as error:
            return error.returncode
        except PersistenceBacklogMeasurementError as error:
            sys.stderr.write(f"invalid persistence backlog receipt: {error}\n")
            return 1

    print(json.dumps(receipt, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
