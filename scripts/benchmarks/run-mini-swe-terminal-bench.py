#!/usr/bin/env python3
"""Run Harbor's stock mini-swe-agent baseline on Terminal-Bench."""

from __future__ import annotations

import argparse
import importlib.util
import json
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

SCRIPT = Path(__file__).resolve()
REPO_ROOT = SCRIPT.parents[2]
CODEWHALE_RUNNER = REPO_ROOT / "scripts" / "benchmarks" / "run-codewhale-terminal-bench.py"

DEFAULT_DATASET = "terminal-bench-sample@2.0"
DEFAULT_AGENT = "mini-swe-agent"
DEFAULT_RESULTS_ROOT = REPO_ROOT / "benchmark_results" / "tbench-mini-swe-default"
DEFAULT_MODEL = "deepseek/deepseek-v4-flash"
EXPLICIT_REASONING_EFFORTS = ("off", "high", "max")


def load_codewhale_runner() -> Any:
    spec = importlib.util.spec_from_file_location("codewhale_tbench_runner", CODEWHALE_RUNNER)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"unable to load {CODEWHALE_RUNNER}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def run_command(cmd: list[str], env: dict[str, str], timeout: int | None) -> int:
    printable = ["<DEEPSEEK_API_BASE>" if part.startswith("DEEPSEEK_API_BASE=") else part for part in cmd]
    print("$ " + " ".join(printable))
    start = time.time()
    try:
        proc = subprocess.run(cmd, cwd=REPO_ROOT, env=env, timeout=timeout)
        elapsed = time.time() - start
        print(f"exit={proc.returncode} elapsed_s={elapsed:.1f}")
        return proc.returncode
    except subprocess.TimeoutExpired:
        elapsed = time.time() - start
        print(f"timeout elapsed_s={elapsed:.1f}", file=sys.stderr)
        return 124


def validate_prereqs(env: dict[str, str]) -> None:
    missing: list[str] = []
    if not env.get("DEEPSEEK_API_KEY"):
        missing.append("DEEPSEEK_API_KEY")
    if missing:
        for item in missing:
            print(f"missing prerequisite: {item}", file=sys.stderr)
        raise SystemExit(2)
    if subprocess.run(["docker", "info"], capture_output=True).returncode != 0:
        raise SystemExit("Docker is not running")
    if subprocess.run(["harbor", "--version"], capture_output=True).returncode != 0:
        raise SystemExit("harbor is not installed")


def main() -> None:
    common = load_codewhale_runner()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dataset", default=DEFAULT_DATASET)
    parser.add_argument("--task", dest="tasks", action="append", default=[])
    parser.add_argument("--model", default=DEFAULT_MODEL)
    parser.add_argument(
        "--reasoning-effort",
        dest="reasoning_effort",
        choices=EXPLICIT_REASONING_EFFORTS,
        default=None,
        help="Optional mini-swe-agent reasoning effort override. Omit for stock defaults.",
    )
    parser.add_argument("--agent", default=DEFAULT_AGENT)
    parser.add_argument("--results-root", type=Path, default=DEFAULT_RESULTS_ROOT)
    parser.add_argument("--concurrency", type=int, default=1)
    parser.add_argument("--max-retries", type=int, default=0)
    parser.add_argument("--timeout-multiplier", type=float, default=1.0)
    parser.add_argument("--wall-timeout", type=int, default=None)
    parser.add_argument("--cost-limit", default="0")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--regenerate", type=Path)
    args = parser.parse_args()

    if args.regenerate:
        common.write_summaries(args.regenerate)
        return

    args.tasks = args.tasks or common.DEFAULT_TASKS
    env = common.build_env([args.model], None, None)
    deepseek_base = env.get("DEEPSEEK_API_BASE") or env.get("DEEPSEEK_BASE_URL") or env.get("CODEWHALE_BASE_URL")
    if deepseek_base:
        env.setdefault("DEEPSEEK_API_BASE", deepseek_base)
    validate_prereqs(env)

    timestamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    safe_model = args.model.replace("/", "_").replace(":", "_")
    effort_label = args.reasoning_effort or "stock"
    job_name = f"mini-swe-{safe_model}-thinking-{effort_label}-{timestamp}"
    run_dir = args.results_root / job_name
    run_dir.mkdir(parents=True, exist_ok=False)

    metadata = {
        "created_at_utc": datetime.now(timezone.utc).isoformat(),
        "dataset": args.dataset,
        "tasks": args.tasks,
        "models": [args.model],
        "reasoning_effort": args.reasoning_effort,
        "agent": args.agent,
        "model_by_job": {job_name: common.label_for_model(args.model, args.reasoning_effort)},
        "reasoning_effort_by_job": {job_name: args.reasoning_effort},
        "credential_env_present": {"DEEPSEEK_API_KEY": bool(env.get("DEEPSEEK_API_KEY"))},
    }
    (run_dir / "metadata.json").write_text(json.dumps(metadata, indent=2, sort_keys=True))

    cmd = [
        "harbor",
        "run",
        "-d",
        args.dataset,
        "--agent",
        args.agent,
        "-m",
        args.model,
        "-n",
        str(args.concurrency),
        "--job-name",
        job_name,
        "-o",
        str(run_dir),
        "--agent-include-logs",
        "mini-swe-agent.txt",
        "--agent-include-logs",
        "mini-swe-agent.trajectory.json",
        "--agent-kwarg",
        f"cost_limit={args.cost_limit}",
        "--yes",
    ]
    if deepseek_base:
        cmd.extend(["--agent-env", f"DEEPSEEK_API_BASE={deepseek_base}"])
    if args.reasoning_effort:
        cmd.extend(["--agent-kwarg", f"reasoning_effort={args.reasoning_effort}"])
    for task in args.tasks:
        cmd.extend(["--include-task-name", task])
    if args.max_retries:
        cmd.extend(["--max-retries", str(args.max_retries)])
    if args.timeout_multiplier != 1.0:
        cmd.extend(["--timeout-multiplier", str(args.timeout_multiplier)])

    if args.dry_run:
        print("$ " + " ".join(cmd))
        return

    exit_code = run_command(cmd, env=env, timeout=args.wall_timeout)
    common.write_summaries(run_dir)
    print(f"results_dir={run_dir}")
    if exit_code != 0:
        raise SystemExit(exit_code)


if __name__ == "__main__":
    main()
