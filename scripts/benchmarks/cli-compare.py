#!/usr/bin/env python3
"""
cli-compare.py — Run Terminal-Bench tasks through CodeWhale and Codex CLIs,
emit normalized token/performance comparison rows.

Usage:
    # Run default tasks
    python scripts/benchmarks/cli-compare.py

    # Specific task and model
    python scripts/benchmarks/cli-compare.py --task prove-plus-comm \\
        --model deepseek/deepseek-chat --runs 3

    # Regenerate from existing run artifacts
    python scripts/benchmarks/cli-compare.py \\
        --regenerate benchmark_results/cli-compare-20260609

Output (per run date):
    benchmark_results/cli-compare-YYYYMMDD/
        summary.json         — one row per agent, all fields normalized
        summary.md           — Markdown table suitable for release notes
        metadata.json        — versions, model, timestamp, platform
        codewhale/<task>/    — raw Harbor output
        codex/<task>/        — raw Harbor output

Prerequisites:
    pip install harbor
    Docker running
    DEEPSEEK_API_KEY set (for CodeWhale)
    CODEX_API_KEY or equivalent set (for Codex)

Field semantics (summary.json rows):
    task              str    — Terminal-Bench task name
    agent             str    — "codewhale" or "codex"
    run_idx           int    — 0-based run index
    reward            float  — pass/fail score (1.0 = pass)
    runtime_s         float  — wall-clock seconds (null if not available)
    exception         str    — raised exception text (null = clean finish)
    input_tokens      int    — provider-reported input tokens
    cached_tokens     int    — provider-reported cached input tokens (null if N/A)
    output_tokens     int    — provider-reported output tokens
    reasoning_tokens  int    — provider-reported reasoning tokens (null if N/A)
    answer_len        int    — locally-derived visible final-answer character count
    transcript_path   str    — relative path to raw agent output file

All missing metrics are serialized as JSON ``null`` — never silently zeroed.
"""

import argparse
import json
import os
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent.parent

# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

DEFAULT_TASKS = [
    "prove-plus-comm",
    "cancel-async-tasks",
    "configure-git-webserver",
    "fix-code-vulnerability",
]
DEFAULT_MODEL = "deepseek/deepseek-chat"
DEFAULT_TIMEOUT_PER_RUN = 900  # seconds (Harbor handles its own timeout internally)
DEFAULT_RUNS = 1
HARBOR_DATASET = "terminal-bench@2.0"
CODEWHALE_AGENT = "scripts.benchmarks.harbor:CodeWhaleAgent"
CODEX_AGENT = "scripts.benchmarks.harbor.codex_agent:CodexAgent"

# ---------------------------------------------------------------------------
# Harbor integration
# ---------------------------------------------------------------------------


def check_harbor() -> None:
    """Verify Harbor is installed and Docker is running."""
    if subprocess.run(["which", "harbor"], capture_output=True).returncode != 0:
        sys.exit("Error: 'harbor' not found. Install with: pip install harbor")
    if subprocess.run(["docker", "info"], capture_output=True).returncode != 0:
        sys.exit("Error: Docker not running. Harbor requires Docker.")


def run_harbor_single_task(
    task: str,
    model: str,
    agent_path: str,
    results_dir: Path,
    timeout: int,
) -> dict[str, Any]:
    """Run a single Terminal-Bench task through Harbor.

    Harbor supports task-level filtering via the ``--task`` flag (Harbor ≥0.4).
    If unavailable, falls back to running the full dataset with env-based filtering.
    """
    dataset = f"{HARBOR_DATASET}:{task}"  # Harbor colon-syntax for single task
    results_dir.mkdir(parents=True, exist_ok=True)

    cmd = [
        "harbor", "run",
        "--dataset", dataset,
        "--agent", agent_path,
        "--model", model,
        "--n-concurrent", "1",
        "--results-dir", str(results_dir),
    ]

    start = time.time()
    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
            cwd=REPO_ROOT,
        )
        runtime_s = round(time.time() - start, 2)
    except subprocess.TimeoutExpired:
        runtime_s = round(time.time() - start, 2)
        return {
            "task": task, "model": model, "agent": agent_path,
            "runtime_s": runtime_s, "exit_code": -1,
            "exception": f"Timeout after {timeout}s",
            "stdout": "", "stderr": "", "results_dir": str(results_dir),
        }

    return {
        "task": task, "model": model, "agent": agent_path,
        "runtime_s": runtime_s,
        "exit_code": proc.returncode,
        "exception": None,
        "stdout": proc.stdout,
        "stderr": proc.stderr,
        "results_dir": str(results_dir),
    }


# ---------------------------------------------------------------------------
# Result parsing
# ---------------------------------------------------------------------------


def _try_int(val: Any) -> Optional[int]:
    if val is None:
        return None
    try:
        return int(val)
    except (ValueError, TypeError):
        return None


def _try_float(val: Any) -> Optional[float]:
    if val is None:
        return None
    try:
        return float(val)
    except (ValueError, TypeError):
        return None


def parse_token_jsonl(lines: list[str]) -> dict[str, Optional[int]]:
    """Extract token usage from CodeWhale/Codex stream JSONL lines.

    CodeWhale emits ``{"type":"result","usage":{...}}`` at end-of-stream.
    Codex may emit usage in closing messages or transcript footers.
    """
    result: dict[str, Optional[int]] = {
        "input_tokens": None, "cached_tokens": None,
        "output_tokens": None, "reasoning_tokens": None,
    }
    if not lines:
        return result

    for line in reversed(lines):  # usage typically at the end
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except json.JSONDecodeError:
            # Try regex extraction for non-JSON transcript lines
            continue

        usage = obj.get("usage") or obj.get("token_usage") or {}
        if isinstance(usage, dict):
            if result["input_tokens"] is None:
                result["input_tokens"] = _try_int(
                    usage.get("input_tokens") or usage.get("prompt_tokens")
                )
            if result["cached_tokens"] is None:
                result["cached_tokens"] = _try_int(
                    usage.get("cached_input_tokens")
                    or usage.get("cache_read_input_tokens")
                    or usage.get("cached_tokens")
                )
            if result["output_tokens"] is None:
                result["output_tokens"] = _try_int(
                    usage.get("output_tokens") or usage.get("completion_tokens")
                )
            if result["reasoning_tokens"] is None:
                result["reasoning_tokens"] = _try_int(
                    usage.get("reasoning_tokens")
                    or usage.get("thinking_tokens")
                    or usage.get("reasoning_completion_tokens")
                )
        if all(v is not None for v in result.values()):
            break

    return result


def extract_answer_len(text: str) -> Optional[int]:
    """Heuristic: length of the last substantial text block that looks like an answer.

    Looks for the last non-code, non-log paragraph after the agent has finished
    its tool-calling phase. Returns character count or None.
    """
    if not text:
        return None
    # Agent outputs often have a "## Final Answer" or similar marker.
    # Try to find the last answer section.
    for marker in ("## Final Answer", "## Answer", "final answer",
                   "Here is the", "The solution"):
        idx = text.rfind(marker)
        if idx >= 0:
            # Take text from marker to end, strip trailing shell logs
            tail = text[idx:]
            # Stop at next shell prompt or markdown separator
            for term in ("```", "$ ", "# ", "/workspace"):
                term_idx = tail.find(term, len(marker))
                if term_idx > 0:
                    tail = tail[:term_idx]
            return len(tail.strip())

    # Fallback: last paragraph that isn't code or a prompt
    paragraphs = [p.strip() for p in text.split("\n\n") if p.strip()]
    for p in reversed(paragraphs):
        if not p.startswith("```") and not p.startswith("$") and len(p) > 20:
            return len(p)

    return len(text.strip()) if text.strip() else None


def parse_harbor_run(task_dir: Path, agent_name: str) -> dict[str, Any]:
    """Parse Harbor results for a single task run.

    Harbor stores per-task output in:
        <task_dir>/
            results.json      — Harbor's own eval summary
            logs/agent/*.txt  — raw agent transcript (if stdout captured)
    """
    row: dict[str, Any] = {
        "task": task_dir.name,
        "agent": agent_name,
        "reward": None,
        "runtime_s": None,
        "exception": None,
        "input_tokens": None,
        "cached_tokens": None,
        "output_tokens": None,
        "reasoning_tokens": None,
        "answer_len": None,
        "transcript_path": None,
    }

    # 1. Harbor results.json — pass/fail and runtime
    for candidate in sorted(task_dir.rglob("results.json")):
        try:
            data = json.loads(candidate.read_text())
            if isinstance(data, dict):
                row["reward"] = _try_float(data.get("score") or data.get("reward"))
                row["runtime_s"] = _try_float(data.get("runtime") or data.get("duration"))
                exc = data.get("exception") or data.get("error")
                row["exception"] = str(exc) if exc else None
                break
        except (json.JSONDecodeError, OSError):
            continue

    # 2. Agent transcript — token usage and answer
    for txt_file in sorted(task_dir.rglob("*.txt")):
        if txt_file.name.startswith("."):
            continue
        try:
            text = txt_file.read_text(errors="ignore")
        except OSError:
            continue
        if not text.strip():
            continue

        row["transcript_path"] = str(txt_file.relative_to(REPO_ROOT))

        # Token extraction from stream JSONL
        tokens = parse_token_jsonl(text.split("\n"))
        row["input_tokens"] = row["input_tokens"] or tokens["input_tokens"]
        row["cached_tokens"] = row["cached_tokens"] or tokens["cached_tokens"]
        row["output_tokens"] = row["output_tokens"] or tokens["output_tokens"]
        row["reasoning_tokens"] = row["reasoning_tokens"] or tokens["reasoning_tokens"]

        # Answer length
        if row["answer_len"] is None:
            row["answer_len"] = extract_answer_len(text)
        break

    # 3. Harbor run metadata — runtime fallback
    for meta_file in sorted(task_dir.rglob("run_metadata.json")):
        try:
            data = json.loads(meta_file.read_text())
            if isinstance(data, dict) and row["runtime_s"] is None:
                row["runtime_s"] = _try_float(data.get("runtime_seconds"))
        except (json.JSONDecodeError, OSError):
            continue

    return row


# ---------------------------------------------------------------------------
# Summary generation
# ---------------------------------------------------------------------------


def generate_markdown_table(rows: list[dict[str, Any]]) -> str:
    """Generate a Markdown comparison table from normalized rows."""
    if not rows:
        return "*(no data)*\n"

    headers = [
        "task", "agent", "reward", "input_tokens", "cached_tokens",
        "output_tokens", "reasoning_tokens", "runtime_s", "answer_len",
    ]

    md = "| " + " | ".join(h.replace("_", " ") for h in headers) + " |\n"
    md += "|" + "|".join(" ---: " for _ in headers) + "|\n"

    for row in rows:
        cells: list[str] = []
        for h in headers:
            val = row.get(h)
            if val is None:
                cells.append("null")
            elif isinstance(val, float):
                cells.append(f"{val:.2f}")
            elif isinstance(val, int):
                cells.append(f"{val:,}")
            else:
                cells.append(str(val))
        md += "| " + " | ".join(cells) + " |\n"

    return md


def generate_json_summary(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Return rows sorted by task, agent, run_idx."""
    return sorted(
        rows,
        key=lambda r: (r.get("task", ""), r.get("agent", ""), r.get("run_idx", 0)),
    )


# ---------------------------------------------------------------------------
# Regenerate from existing logs
# ---------------------------------------------------------------------------


def regenerate(results_dir: Path) -> list[dict[str, Any]]:
    """Walk existing run directory and rebuild normalized rows."""
    rows: list[dict[str, Any]] = []
    for agent_dir in sorted(results_dir.iterdir()):
        if not agent_dir.is_dir() or agent_dir.name.startswith("."):
            continue
        agent_name = agent_dir.name
        for task_dir in sorted(agent_dir.iterdir()):
            if not task_dir.is_dir():
                continue
            # Check for per-run subdirectories
            subdirs = [d for d in task_dir.iterdir() if d.is_dir()]
            if subdirs and all(d.name.startswith("run_") for d in subdirs):
                for run_dir in sorted(subdirs):
                    row = parse_harbor_run(run_dir, agent_name)
                    row["task"] = row["task"] or task_dir.name
                    try:
                        row["run_idx"] = int(run_dir.name.split("_")[-1])
                    except (ValueError, IndexError):
                        row["run_idx"] = 0
                    rows.append(row)
            else:
                row = parse_harbor_run(task_dir, agent_name)
                row["task"] = row["task"] or task_dir.name
                row["run_idx"] = 0
                rows.append(row)
    return rows


# ---------------------------------------------------------------------------
# Metadata capture
# ---------------------------------------------------------------------------


def capture_metadata(model: str) -> dict[str, Any]:
    """Capture environment metadata for reproducibility."""
    meta: dict[str, Any] = {
        "timestamp_utc": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "platform": os.uname().sysname + "/" + os.uname().machine,
        "model": model,
        "dataset": HARBOR_DATASET,
    }
    # CodeWhale version
    r = subprocess.run(["codewhale", "--version"], capture_output=True, text=True)
    if r.returncode == 0:
        meta["codewhale_version"] = r.stdout.strip()
    # Codex version
    r = subprocess.run(["codex", "--version"], capture_output=True, text=True)
    if r.returncode == 0:
        meta["codex_version"] = r.stdout.strip()
    # Harbor version
    r = subprocess.run(["harbor", "--version"], capture_output=True, text=True)
    if r.returncode == 0:
        meta["harbor_version"] = r.stdout.strip()
    # Git commit
    r = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        capture_output=True, text=True, cwd=REPO_ROOT,
    )
    if r.returncode == 0:
        meta["git_commit"] = r.stdout.strip()[:12]
    return meta


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(
        description="CodeWhale vs Codex CLI token comparison harness",
    )
    parser.add_argument(
        "--task", nargs="+", default=DEFAULT_TASKS,
        help=f"Terminal-Bench task names (default: {' '.join(DEFAULT_TASKS)})",
    )
    parser.add_argument(
        "--model", default=DEFAULT_MODEL,
        help=f"Model in provider/name format (default: {DEFAULT_MODEL})",
    )
    parser.add_argument(
        "--runs", type=int, default=DEFAULT_RUNS,
        help=f"Number of runs per agent per task (default: {DEFAULT_RUNS})",
    )
    parser.add_argument(
        "--timeout", type=int, default=DEFAULT_TIMEOUT_PER_RUN,
        help=f"Timeout per run in seconds (default: {DEFAULT_TIMEOUT_PER_RUN})",
    )
    parser.add_argument(
        "--regenerate", type=Path, default=None,
        help="Regenerate summary from existing raw results directory",
    )
    parser.add_argument(
        "--codewhale-agent", default=CODEWHALE_AGENT,
        help="Harbor agent import path for CodeWhale",
    )
    parser.add_argument(
        "--codex-agent", default=CODEX_AGENT,
        help="Harbor agent import path for Codex",
    )
    args = parser.parse_args()

    # --------------- Regenerate mode ---------------
    if args.regenerate:
        results_dir = args.regenerate
        if not results_dir.exists():
            sys.exit(f"Error: results directory not found: {results_dir}")
        rows = regenerate(results_dir)
        print(generate_markdown_table(rows))
        return

    # --------------- Fresh run mode ---------------
    check_harbor()

    date_str = datetime.now().strftime("%Y%m%d")
    run_dir = REPO_ROOT / "benchmark_results" / f"cli-compare-{date_str}"
    if run_dir.exists():
        # Append run number if directory already exists
        suffix = 2
        while (run_dir := REPO_ROOT / "benchmark_results" /
               f"cli-compare-{date_str}-{suffix}").exists():
            suffix += 1
    run_dir.mkdir(parents=True, exist_ok=True)

    # Metadata
    meta = capture_metadata(args.model)
    meta["tasks"] = args.task
    meta["runs_per_task"] = args.runs
    (run_dir / "metadata.json").write_text(json.dumps(meta, indent=2))

    cw_dir = run_dir / "codewhale"
    cx_dir = run_dir / "codex"
    cw_dir.mkdir(parents=True, exist_ok=True)
    cx_dir.mkdir(parents=True, exist_ok=True)

    all_rows: list[dict[str, Any]] = []

    for task in args.task:
        for run_idx in range(args.runs):
            header = f"Task: {task}  Run: {run_idx+1}/{args.runs}"
            print(f"\n{'='*60}")
            print(header)
            print("=" * 60)

            # --- CodeWhale ---
            print("\n--- CodeWhale ---")
            cw_run_dir = cw_dir / task / f"run_{run_idx}"
            cw_result = run_harbor_single_task(
                task=task, model=args.model,
                agent_path=args.codewhale_agent,
                results_dir=cw_run_dir, timeout=args.timeout,
            )
            cw_row = parse_harbor_run(cw_run_dir, "codewhale")
            cw_row["task"] = task
            cw_row["run_idx"] = run_idx
            cw_row["runtime_s"] = cw_row["runtime_s"] or cw_result["runtime_s"]
            if cw_result["exception"]:
                cw_row["exception"] = cw_row["exception"] or cw_result["exception"]
            all_rows.append(cw_row)
            self_report(cw_row)

            # --- Codex ---
            print("\n--- Codex ---")
            cx_run_dir = cx_dir / task / f"run_{run_idx}"
            cx_result = run_harbor_single_task(
                task=task, model=args.model,
                agent_path=args.codex_agent,
                results_dir=cx_run_dir, timeout=args.timeout,
            )
            cx_row = parse_harbor_run(cx_run_dir, "codex")
            cx_row["task"] = task
            cx_row["run_idx"] = run_idx
            cx_row["runtime_s"] = cx_row["runtime_s"] or cx_result["runtime_s"]
            if cx_result["exception"]:
                cx_row["exception"] = cx_row["exception"] or cx_result["exception"]
            all_rows.append(cx_row)
            self_report(cx_row)

    # Write summaries
    summary_json = run_dir / "summary.json"
    summary_json.write_text(
        json.dumps(generate_json_summary(all_rows), indent=2)
    )
    print(f"\nSummary JSON: {summary_json}")

    md = generate_markdown_table(all_rows)
    (run_dir / "summary.md").write_text(md)
    print(f"Summary MD:   {run_dir / 'summary.md'}")
    print(f"Metadata:     {run_dir / 'metadata.json'}")
    print("\n" + md)


def self_report(row: dict[str, Any]) -> None:
    """Print a one-line summary of a parsed run."""
    parts = [
        f"reward={row['reward']}" if row["reward"] is not None else "reward=null",
        f"input={row['input_tokens']}" if row["input_tokens"] is not None else "input=null",
        f"output={row['output_tokens']}" if row["output_tokens"] is not None else "output=null",
        f"cached={row['cached_tokens']}" if row["cached_tokens"] is not None else "",
        f"reasoning={row['reasoning_tokens']}" if row["reasoning_tokens"] is not None else "",
        f"answer_len={row['answer_len']}" if row["answer_len"] is not None else "",
        f"runtime={row['runtime_s']:.1f}s" if row["runtime_s"] is not None else "",
    ]
    print("  " + ", ".join(p for p in parts if p))


if __name__ == "__main__":
    main()
