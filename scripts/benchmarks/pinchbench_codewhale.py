#!/usr/bin/env python3
"""
CodeWhale-native PinchBench runner.

Loads PinchBench tasks, runs them through codewhale exec, and grades results.
No OpenClaw dependency.

Usage:
    python scripts/benchmarks/pinchbench_codewhale.py --help
    python scripts/benchmarks/pinchbench_codewhale.py --suite task_calendar
    python scripts/benchmarks/pinchbench_codewhale.py --suite task_calendar,task_stock
    python scripts/benchmarks/pinchbench_codewhale.py --suite all
"""
# /// script
# requires-python = ">=3.10"
# dependencies = [
#     "pyyaml>=6.0.1",
# ]
# ///

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional


def load_task(task_path: Path) -> dict[str, Any]:
    """Load a PinchBench task markdown file."""
    content = task_path.read_text(encoding="utf-8")

    fm_match = re.match(r"^---\s*\n(.*?)\n---\s*\n(.*)$", content, re.DOTALL)
    if not fm_match:
        raise ValueError(f"No YAML frontmatter in {task_path}")

    import yaml
    frontmatter = yaml.safe_load(fm_match.group(1))
    body = fm_match.group(2)

    sections: dict[str, str] = {}
    current_section = None
    current_content: list[str] = []
    for line in body.split("\n"):
        header = re.match(r"^##\s+(.+)$", line)
        if header:
            if current_section:
                sections[current_section] = "\n".join(current_content).strip()
            current_section = header.group(1)
            current_content = []
        else:
            current_content.append(line)
    if current_section:
        sections[current_section] = "\n".join(current_content).strip()

    return {
        "task_id": frontmatter.get("id", task_path.stem),
        "name": frontmatter.get("name", ""),
        "category": frontmatter.get("category", ""),
        "grading_type": frontmatter.get("grading_type", "automated"),
        "timeout_seconds": frontmatter.get("timeout_seconds", 120),
        "workspace_files": frontmatter.get("workspace_files", []),
        "prompt": sections.get("Prompt", "").strip(),
        "automated_checks": sections.get("Automated Checks", None),
        "llm_judge_rubric": sections.get("LLM Judge Rubric", None),
        "grading_criteria": sections.get("Grading Criteria", ""),
        "expected_behavior": sections.get("Expected Behavior", ""),
        "path": task_path,
    }


def prepare_workspace(task: dict, run_dir: Path, tasks_dir: Path) -> Path:
    """Create a temp workspace with any task-required files."""
    workspace = run_dir / task["task_id"]
    workspace.mkdir(parents=True, exist_ok=True)

    # Initialize git repo so codewhale works
    subprocess.run(["git", "init"], cwd=workspace, capture_output=True, check=False)
    subprocess.run(
        ["git", "config", "user.email", "bench@codewhale"],
        cwd=workspace, capture_output=True, check=False,
    )
    subprocess.run(
        ["git", "config", "user.name", "Benchmark"],
        cwd=workspace, capture_output=True, check=False,
    )

    # Copy workspace files — source paths may be relative to tasks/ or assets/
    assets_dir = tasks_dir.parent / "assets"
    for wf in task.get("workspace_files", []):
        if isinstance(wf, dict) and "source" in wf and "dest" in wf:
            # Try tasks_dir first, then assets_dir
            src = tasks_dir / wf["source"]
            if not src.exists():
                src = assets_dir / wf["source"]
            dst = workspace / wf["dest"]
            dst.parent.mkdir(parents=True, exist_ok=True)
            if src.exists():
                shutil.copy2(src, dst)
            else:
                print(f"  Warning: workspace file not found: {wf['source']}", file=sys.stderr)
        elif isinstance(wf, dict):
            # Legacy format: {path: content}
            for path, content in wf.items():
                fpath = workspace / path
                fpath.parent.mkdir(parents=True, exist_ok=True)
                fpath.write_text(str(content), encoding="utf-8")

    # Commit initial state
    subprocess.run(["git", "add", "-A"], cwd=workspace, capture_output=True, check=False)
    subprocess.run(
        ["git", "commit", "-m", "initial", "--allow-empty"],
        cwd=workspace, capture_output=True, check=False,
    )

    return workspace


def run_codewhale(
    workspace: Path,
    prompt: str,
    timeout_seconds: int,
    model: Optional[str] = None,
) -> dict[str, Any]:
    """Run codewhale exec on a task and return the result."""
    cmd = [
        "codewhale", "exec",
        "--auto",
        "--workspace", str(workspace),
    ]
    if model:
        cmd.extend(["--model", model])
    cmd.append(prompt)

    start = time.time()
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout_seconds,
            cwd=workspace,
            check=False,
        )
        elapsed = time.time() - start
        return {
            "exit_code": result.returncode,
            "stdout": result.stdout,
            "stderr": result.stderr,
            "elapsed_seconds": elapsed,
            "timed_out": False,
        }
    except subprocess.TimeoutExpired:
        elapsed = time.time() - start
        return {
            "exit_code": -1,
            "stdout": "",
            "stderr": "TIMEOUT",
            "elapsed_seconds": elapsed,
            "timed_out": True,
        }


def grade_automated(task: dict, workspace: Path, transcript: list) -> dict[str, Any]:
    """Run the automated grading check from the task definition."""
    checks_code = task.get("automated_checks")
    if not checks_code:
        return {"score": 0.0, "reason": "no automated checks defined"}

    code_match = re.search(r"```python\n(.*?)```", checks_code, re.DOTALL)
    if not code_match:
        return {"score": 0.0, "reason": "no python code block in automated checks"}

    code = code_match.group(1)
    namespace: dict[str, Any] = {}
    try:
        exec(code, namespace)
    except Exception as e:
        return {"score": 0.0, "reason": f"grading code failed to load: {e}"}

    grade_fn = namespace.get("grade")
    if not grade_fn:
        return {"score": 0.0, "reason": "no grade() function in automated checks"}

    try:
        result = grade_fn(transcript, str(workspace))
        if isinstance(result, dict):
            numeric = [v for v in result.values() if isinstance(v, (int, float))]
            avg = sum(numeric) / len(numeric) if numeric else 0.0
            result["score"] = avg
            return result
        return {"score": float(result) if result else 0.0}
    except Exception as e:
        return {"score": 0.0, "reason": f"grading failed: {e}"}


def grade_llm_judge(task: dict, workspace: Path, transcript: list, model: Optional[str] = None) -> dict[str, Any]:
    """Use codewhale as an LLM judge to grade a task."""
    rubric = task.get("llm_judge_rubric")
    if not rubric:
        return {"score": 0.0, "reason": "no LLM judge rubric"}

    criteria = task.get("grading_criteria", "")
    expected = task.get("expected_behavior", "")

    # Collect workspace files for context
    ws_files = []
    for f in workspace.rglob("*"):
        if f.is_file() and ".git" not in str(f):
            try:
                content = f.read_text(encoding="utf-8", errors="replace")[:3000]
                ws_files.append(f"--- {f.name} ---\n{content}")
            except Exception:
                ws_files.append(f"--- {f.name} --- (binary/unreadable)")

    ws_content = "\n\n".join(ws_files[:10])  # Limit to 10 files

    judge_prompt = f"""You are a grading judge. Evaluate whether the agent's output meets the task requirements.

TASK: {task['name']}

EXPECTED BEHAVIOR:
{expected}

GRADING CRITERIA:
{criteria}

LLM JUDGE RUBRIC:
{rubric}

AGENT'S WORKSPACE FILES:
{ws_content}

Score the task on a scale of 0.0 to 1.0. Respond with ONLY a JSON object:
{{"score": <float>, "reason": "<brief explanation>"}}

Be strict but fair. Partial credit is OK."""

    cmd = ["codewhale", "exec", "--auto", "--workspace", str(workspace)]
    if model:
        cmd.extend(["--model", model])
    cmd.append(judge_prompt)

    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=120,
            cwd=workspace,
            check=False,
        )
        # Extract JSON from response — strip control chars that break json.loads
        output = result.stdout
        # Remove ANSI escape codes
        output = re.sub(r'\x1b\[[0-9;]*[a-zA-Z]', '', output)
        output = re.sub(r'\x1b\][^\x07]*\x07', '', output)
        json_match = re.search(r'\{[^{}]*"score"[^{}]*\}', output)
        if json_match:
            raw = json_match.group()
            # Strip control characters except newline/tab
            raw = re.sub(r'[\x00-\x08\x0b\x0c\x0e-\x1f]', '', raw)
            parsed = json.loads(raw)
            return {
                "score": float(parsed.get("score", 0.0)),
                "reason": parsed.get("reason", "llm judge"),
                "judge": "llm",
            }
        return {"score": 0.0, "reason": "llm judge returned unparseable response", "judge": "llm"}
    except Exception as e:
        return {"score": 0.0, "reason": f"llm judge failed: {e}", "judge": "llm"}


def run_benchmark(
    tasks_dir: Path,
    suite: str,
    results_dir: Path,
    model: Optional[str] = None,
    timeout_multiplier: float = 1.0,
) -> dict[str, Any]:
    """Run the benchmark suite."""
    all_tasks: list[dict] = []

    if suite == "all":
        task_files = sorted(tasks_dir.glob("task_*.md"))
        for tf in task_files:
            try:
                all_tasks.append(load_task(tf))
            except Exception as e:
                print(f"  Skip {tf.name}: {e}", file=sys.stderr)
    else:
        task_ids = [t.strip() for t in suite.split(",")]
        for tid in task_ids:
            tf = tasks_dir / f"{tid}.md"
            if not tf.exists():
                print(f"  Task not found: {tf}", file=sys.stderr)
                continue
            all_tasks.append(load_task(tf))

    if not all_tasks:
        print("No tasks loaded.", file=sys.stderr)
        sys.exit(1)

    print(f"Loaded {len(all_tasks)} tasks")

    results_dir.mkdir(parents=True, exist_ok=True)
    run_id = datetime.now(timezone.utc).strftime("%Y%m%d-%H%M%S")
    run_dir = results_dir / run_id
    run_dir.mkdir()

    cw_version = "unknown"
    try:
        vr = subprocess.run(["codewhale", "--version"], capture_output=True, text=True)
        if vr.returncode == 0:
            cw_version = vr.stdout.strip()
    except FileNotFoundError:
        pass

    metadata = {
        "codewhale_version": cw_version,
        "model": model or "default",
        "suite": suite,
        "task_count": len(all_tasks),
        "run_id": run_id,
        "timestamp_utc": datetime.now(timezone.utc).isoformat(),
    }
    (run_dir / "metadata.json").write_text(json.dumps(metadata, indent=2))

    results: list[dict] = []
    total_score = 0.0

    for i, task in enumerate(all_tasks, 1):
        task_id = task["task_id"]
        print(f"\n{'='*60}")
        print(f"Task {i}/{len(all_tasks)}: {task_id} — {task['name']}")
        print(f"  Category: {task['category']} | Grading: {task['grading_type']}")
        print(f"{'='*60}")

        workspace = prepare_workspace(task, run_dir, tasks_dir)
        timeout = int(task["timeout_seconds"] * timeout_multiplier)

        # Run codewhale
        print(f"  Running codewhale exec (timeout: {timeout}s)...")
        result = run_codewhale(workspace, task["prompt"], timeout, model=model)
        print(f"  Completed in {result['elapsed_seconds']:.1f}s (exit {result['exit_code']})")

        if result["timed_out"]:
            print(f"  ⏰ TIMED OUT")

        # Build transcript for grading
        transcript = [{"role": "user", "content": task["prompt"]}]
        if result["stdout"]:
            transcript.append({"role": "assistant", "content": result["stdout"]})

        # Grade based on type
        grading_type = task.get("grading_type", "automated")
        has_automated = task.get("automated_checks") and "```python" in (task.get("automated_checks") or "")
        has_llm_rubric = bool(task.get("llm_judge_rubric"))

        grade_result = {"score": 0.0, "reason": "not graded"}

        if has_automated:
            grade_result = grade_automated(task, workspace, transcript)

        # If automated score is 0 and there's an LLM rubric, try LLM judge
        if grade_result.get("score", 0.0) == 0.0 and has_llm_rubric:
            print(f"  Running LLM judge...")
            llm_result = grade_llm_judge(task, workspace, transcript, model=model)
            # Use LLM judge score if it's better, or if no automated checks
            if not has_automated or llm_result.get("score", 0.0) > 0.0:
                grade_result = llm_result

        if not has_automated and not has_llm_rubric:
            grade_result = {"score": 0.0, "reason": "no grading method defined"}

        score = grade_result.get("score", 0.0)
        total_score += score

        status = "✅" if score >= 1.0 else "🔶" if score > 0 else "❌"
        print(f"  {status} Score: {score:.1%} — {grade_result.get('reason', '')}")

        task_result = {
            "task_id": task_id,
            "name": task["name"],
            "category": task["category"],
            "score": score,
            "grade": grade_result,
            "elapsed_seconds": result["elapsed_seconds"],
            "timed_out": result["timed_out"],
            "exit_code": result["exit_code"],
        }
        results.append(task_result)

        (run_dir / f"{task_id}.json").write_text(json.dumps(task_result, indent=2))

    # Summary
    avg_score = total_score / len(results) if results else 0.0

    categories: dict[str, list[dict]] = {}
    for r in results:
        cat = r["category"]
        categories.setdefault(cat, []).append(r)

    summary = {
        "run_id": run_id,
        "total_score": total_score,
        "task_count": len(results),
        "average_score": avg_score,
        "categories": {
            cat: {
                "score": sum(r["score"] for r in tasks) / len(tasks) if tasks else 0,
                "tasks": len(tasks),
            }
            for cat, tasks in categories.items()
        },
        "results": results,
        "metadata": metadata,
    }

    (run_dir / "summary.json").write_text(json.dumps(summary, indent=2))

    print(f"\n{'='*60}")
    print(f"PINCHBENCH SCORE SUMMARY (CodeWhale)")
    print(f"{'='*60}")
    print(f"\n  Overall: {avg_score:.1%} ({total_score:.1f}/{len(results)})\n")
    print(f"  {'CATEGORY':<25} {'SCORE':>8}  {'TASKS':>5}")
    print(f"  {'-'*45}")
    for cat, info in sorted(summary["categories"].items()):
        pct = info["score"] * 100
        marker = "🔴" if pct < 25 else "🟡" if pct < 75 else "🟢"
        print(f"  {marker} {cat:<23} {pct:>6.1f}%  {info['tasks']:>5}")
    print(f"  {'-'*45}")
    print(f"\nResults: {run_dir}")

    return summary


def main():
    parser = argparse.ArgumentParser(
        description="Run PinchBench tasks through CodeWhale (no OpenClaw)"
    )
    parser.add_argument(
        "--tasks-dir",
        type=Path,
        default=Path("/tmp/pinchbench/tasks"),
        help="PinchBench tasks directory",
    )
    parser.add_argument(
        "--suite",
        default="task_calendar",
        help="Comma-separated task IDs, or 'all'",
    )
    parser.add_argument(
        "--results-dir",
        type=Path,
        default=Path("./results/pinchbench-codewhale"),
        help="Results output directory",
    )
    parser.add_argument("--model", default=None, help="Model override for codewhale")
    parser.add_argument(
        "--timeout-multiplier",
        type=float,
        default=1.0,
        help="Scale task timeouts",
    )
    args = parser.parse_args()

    run_benchmark(
        tasks_dir=args.tasks_dir,
        suite=args.suite,
        results_dir=args.results_dir,
        model=args.model,
        timeout_multiplier=args.timeout_multiplier,
    )


if __name__ == "__main__":
    main()
