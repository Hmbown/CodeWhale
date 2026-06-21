#!/usr/bin/env python3
"""Run CodeWhale local artifacts on Terminal-Bench through Harbor.

This harness is intentionally local and evidence-oriented:

- it benchmarks explicit Linux CodeWhale binaries, not the npm package;
- it loads provider credentials into the Harbor subprocess environment only;
- it writes compact summaries from Harbor result JSON and CodeWhale stream logs.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
import tomllib
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

SCRIPT = Path(__file__).resolve()
REPO_ROOT = SCRIPT.parents[2]

DEFAULT_DATASET = "terminal-bench-sample@2.0"
DEFAULT_AGENT = "scripts.benchmarks.harbor.codewhale_local_agent:CodeWhaleLocalAgent"
DEFAULT_RESULTS_ROOT = REPO_ROOT / "benchmark_results" / "tbench-codewhale"
CODEWHALE_LINUX_BIN_ENV = "CODEWHALE_LINUX_BIN"
CODEWHALE_TUI_LINUX_BIN_ENV = "CODEWHALE_TUI_LINUX_BIN"
DEFAULT_MODELS = ["deepseek/deepseek-v4-flash", "deepseek/deepseek-v4-pro"]
DEFAULT_TASKS = [
    "build-cython-ext",
    "chess-best-move",
    "configure-git-webserver",
    "fix-code-vulnerability",
    "log-summary-date-ranges",
    "polyglot-c-py",
    "qemu-alpine-ssh",
    "qemu-startup",
    "regex-log",
    "sqlite-with-gcov",
]
DEFAULT_DEEPSEEK_BASE_URL = "https://api.deepseek.com/beta"
EXPLICIT_REASONING_EFFORTS = ("off", "high", "max")
FAILURE_CLASSES = (
    "solved",
    "model_wrong_answer",
    "tool_policy_loop",
    "artifact_incompatible",
    "setup_timeout",
    "background_not_ready",
    "verifier_environment_failure",
    "context_exhaustion",
    "harness_exception",
)
HARNESS_TIMEOUTS = {
    "default_command_s": 30,
    "build_command_s": 300,
    "background_start_s": 600,
    "readiness_probe_s": 120,
    "verifier_s": 900,
}
ARTIFACT_PREFLIGHT_COMMANDS = [
    "codewhale --version",
    'ldd "$(command -v codewhale)"',
    "/lib/x86_64-linux-gnu/libc.so.6 || true",
]
TASK_READINESS_PROBES = {
    "configure-git-webserver": (
        "curl -fsS http://127.0.0.1:8080/ >/dev/null && "
        "rm -rf /tmp/codewhale-readiness-git-probe && "
        "git clone http://127.0.0.1:8080/repo.git /tmp/codewhale-readiness-git-probe"
    ),
    "qemu-alpine-ssh": (
        "timeout 20 bash -lc 'printf \"\\n\" | nc -w 5 127.0.0.1 6665 | "
        "grep -Ei \"login:|localhost login\"'"
    ),
    "qemu-startup": (
        "timeout 20 bash -lc 'printf \"\\n\" | nc -w 5 127.0.0.1 6665 | "
        "grep -Ei \"login:|localhost login\"'"
    ),
}
KNOWN_MODEL_TOOLS = (
    "grep_files",
    "read_file",
    "write_file",
    "edit_file",
    "exec_shell",
    "apply_patch",
    "list_dir",
    "find_files",
)
TOOL_POLICY_LOOP_THRESHOLD = 3
DENIAL_TERMS = (
    "denied",
    "not allowed",
    "not available",
    "blocked",
    "forbidden",
    "tool policy",
    "use a different tool",
    "stop using",
)
ARTIFACT_INCOMPATIBLE_RE = re.compile(
    r"artifact_incompatible|error while loading shared libraries|"
    r"glibc_[0-9]|version `?glibc|version .* not found|"
    r"libssl[^\\n]*not found|libcrypto[^\\n]*not found|libdbus[^\\n]*not found|"
    r"openssl[^\\n]*(?:not found|incompatible)",
    re.IGNORECASE,
)
BACKGROUND_NOT_READY_RE = re.compile(
    r"background_not_ready|readiness probe failed|timed out waiting for .*ready|"
    r"connection refused|service .*not ready",
    re.IGNORECASE,
)
VERIFIER_ENVIRONMENT_RE = re.compile(
    r"verifier_environment_failure|verifier .*environment|grader .*environment|"
    r"tests?/verify\\.sh: .*not found|pytest: command not found",
    re.IGNORECASE,
)
CONTEXT_EXHAUSTION_RE = re.compile(
    r"context_exhaustion|context window|maximum context|token limit|context length",
    re.IGNORECASE,
)


def stable_path(path: Path) -> str:
    try:
        return str(path.relative_to(REPO_ROOT))
    except ValueError:
        return str(path)


def provider_from_model(model: str) -> str:
    return model.split("/", 1)[0] if "/" in model else "deepseek"


def label_for_model(model: str, reasoning_effort: str | None) -> str:
    return f"{model}@{reasoning_effort or 'default'}"


def readiness_probe_for_task(task: str | None) -> str | None:
    if not task:
        return None
    normalized = task.strip().lower()
    for task_key, probe in TASK_READINESS_PROBES.items():
        if task_key in normalized:
            return probe
    return None


def task_harness_metadata(tasks: list[str]) -> dict[str, dict[str, Any]]:
    return {
        task: {
            "readiness_probe": readiness_probe_for_task(task),
            "timeout_policy": HARNESS_TIMEOUTS,
        }
        for task in tasks
    }


def env_key_for_provider(provider: str) -> str:
    return {
        "deepseek": "DEEPSEEK_API_KEY",
        "openrouter": "OPENROUTER_API_KEY",
        "openai": "OPENAI_API_KEY",
        "zai": "ZAI_API_KEY",
        "z-ai": "ZAI_API_KEY",
    }.get(provider, f"{provider.replace('-', '_').upper()}_API_KEY")


def resolve_artifact_path(cli_path: Path | None, env_key: str) -> Path | None:
    if cli_path is not None:
        return cli_path.expanduser()
    value = os.environ.get(env_key)
    if value and value.strip():
        return Path(value.strip()).expanduser()
    return None


def load_codewhale_config() -> dict[str, Any]:
    path = Path.home() / ".codewhale" / "config.toml"
    if not path.exists():
        return {}
    return tomllib.loads(path.read_text())


def config_provider_table(config: dict[str, Any]) -> dict[str, Any]:
    providers = config.get("providers")
    return providers if isinstance(providers, dict) else {}


def config_api_key(config: dict[str, Any], provider: str) -> str | None:
    providers = config_provider_table(config)
    provider_cfg = providers.get(provider, {})
    if isinstance(provider_cfg, dict):
        key = provider_cfg.get("api_key")
        if isinstance(key, str) and key.strip():
            return key.strip()
    key = config.get("api_key")
    if provider == "deepseek" and isinstance(key, str) and key.strip():
        return key.strip()
    return None


def config_base_url(config: dict[str, Any], provider: str) -> str | None:
    providers = config_provider_table(config)
    provider_cfg = providers.get(provider, {})
    if isinstance(provider_cfg, dict):
        base_url = provider_cfg.get("base_url")
        if isinstance(base_url, str) and base_url.strip():
            return base_url.strip()
    base_url = config.get("base_url")
    if provider == "deepseek" and isinstance(base_url, str) and base_url.strip():
        return base_url.strip()
    if provider == "deepseek":
        return DEFAULT_DEEPSEEK_BASE_URL
    return None


def build_env(
    models: list[str],
    linux_bin: Path | None,
    tui_linux_bin: Path | None,
) -> dict[str, str]:
    config = load_codewhale_config()
    env = os.environ.copy()
    if linux_bin is not None:
        env[CODEWHALE_LINUX_BIN_ENV] = str(linux_bin)
    if tui_linux_bin is not None:
        env[CODEWHALE_TUI_LINUX_BIN_ENV] = str(tui_linux_bin)
    python_path = env.get("PYTHONPATH")
    env["PYTHONPATH"] = (
        str(REPO_ROOT) if not python_path else f"{REPO_ROOT}{os.pathsep}{python_path}"
    )

    providers = sorted({provider_from_model(model) for model in models})
    for provider in providers:
        key_env = env_key_for_provider(provider)
        if not env.get(key_env):
            key = config_api_key(config, provider)
            if key:
                env[key_env] = key
        base_url = config_base_url(config, provider)
        if base_url:
            base_env = f"{provider.replace('-', '_').upper()}_BASE_URL"
            env.setdefault(base_env, base_url)
            if provider == "deepseek":
                env.setdefault("CODEWHALE_BASE_URL", base_url)
    return env


def validate_prereqs(args: argparse.Namespace, env: dict[str, str]) -> None:
    missing: list[str] = []
    artifacts = [
        ("CodeWhale Linux binary", args.linux_bin, "--linux-bin", CODEWHALE_LINUX_BIN_ENV),
        (
            "CodeWhale TUI Linux binary",
            args.tui_linux_bin,
            "--tui-linux-bin",
            CODEWHALE_TUI_LINUX_BIN_ENV,
        ),
    ]
    for label, path, flag, env_key in artifacts:
        if path is None:
            missing.append(f"{label} ({flag} or {env_key})")
        elif not path.is_file():
            missing.append(f"{label} not found: {path}")
    for provider in sorted({provider_from_model(model) for model in args.models}):
        key_env = env_key_for_provider(provider)
        if not env.get(key_env):
            missing.append(key_env)
    if missing:
        for item in missing:
            print(f"missing prerequisite: {item}", file=sys.stderr)
        raise SystemExit(2)
    if subprocess.run(["docker", "info"], capture_output=True).returncode != 0:
        raise SystemExit("Docker is not running")
    if subprocess.run(["harbor", "--version"], capture_output=True).returncode != 0:
        raise SystemExit("harbor is not installed")


def run_command(cmd: list[str], env: dict[str, str], timeout: int | None) -> int:
    print("$ " + " ".join(cmd))
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


def json_load(path: Path) -> dict[str, Any] | None:
    try:
        data = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError):
        return None
    return data if isinstance(data, dict) else None


def seconds_between(started_at: str | None, finished_at: str | None) -> float | None:
    if not started_at or not finished_at:
        return None
    try:
        start = datetime.fromisoformat(started_at.replace("Z", "+00:00"))
        finish = datetime.fromisoformat(finished_at.replace("Z", "+00:00"))
    except ValueError:
        return None
    return round((finish - start).total_seconds(), 3)


def first_number(mapping: dict[str, Any], keys: tuple[str, ...]) -> int | float | None:
    for key in keys:
        value = mapping.get(key)
        if isinstance(value, (int, float)):
            return value
    return None


def merge_usage(target: dict[str, Any], usage: dict[str, Any]) -> None:
    mapping = {
        "input_tokens": ("input_tokens", "prompt_tokens", "n_input_tokens"),
        "cached_tokens": ("cached_input_tokens", "cache_read_input_tokens", "cached_tokens", "n_cache_tokens"),
        "output_tokens": ("output_tokens", "completion_tokens", "n_output_tokens"),
        "reasoning_tokens": ("reasoning_tokens", "thinking_tokens", "reasoning_completion_tokens"),
        "cost_usd": ("cost_usd", "cost"),
    }
    for out_key, keys in mapping.items():
        if target.get(out_key) is None:
            value = first_number(usage, keys)
            if value is not None:
                target[out_key] = value


def walk_usage(obj: Any, row: dict[str, Any]) -> None:
    if isinstance(obj, dict):
        if any(key in obj for key in ("input_tokens", "prompt_tokens", "n_input_tokens", "cost_usd")):
            merge_usage(row, obj)
        for key in ("usage", "token_usage", "metrics", "agent_result"):
            child = obj.get(key)
            if isinstance(child, dict):
                walk_usage(child, row)
        for value in obj.values():
            if isinstance(value, (dict, list)):
                walk_usage(value, row)
    elif isinstance(obj, list):
        for item in obj:
            walk_usage(item, row)


def denied_tool_counts(text: str) -> dict[str, int]:
    counts = {tool: 0 for tool in KNOWN_MODEL_TOOLS}
    for line in text.splitlines():
        lowered = line.lower()
        if not any(term in lowered for term in DENIAL_TERMS):
            continue
        for tool in KNOWN_MODEL_TOOLS:
            if tool in lowered:
                counts[tool] += 1
    return {tool: count for tool, count in counts.items() if count > 0}


def merge_denied_tool_counts(row: dict[str, Any], counts: dict[str, int]) -> None:
    if not counts:
        return
    existing = row.get("denied_tool_counts")
    if not isinstance(existing, dict):
        existing = {}
        row["denied_tool_counts"] = existing
    for tool, count in counts.items():
        existing[tool] = int(existing.get(tool, 0)) + count


def read_text_if_exists(path: Path) -> str:
    try:
        return path.read_text(errors="replace")
    except OSError:
        return ""


def parse_agent_log(path: Path, row: dict[str, Any]) -> None:
    try:
        text = path.read_text(errors="replace")
    except OSError:
        return
    row["transcript_path"] = stable_path(path)
    row["transcript_bytes"] = len(text.encode("utf-8", errors="replace"))
    merge_denied_tool_counts(row, denied_tool_counts(text))
    for line in text.splitlines():
        stripped = line.strip()
        json_start = stripped.find("{")
        if json_start < 0:
            continue
        stripped = stripped[json_start:]
        try:
            obj = json.loads(stripped)
        except json.JSONDecodeError:
            continue
        walk_usage(obj, row)


def parse_exception(exception_info: Any) -> str | None:
    if not exception_info:
        return None
    if isinstance(exception_info, dict):
        typ = exception_info.get("type") or exception_info.get("exception_type")
        message = exception_info.get("message") or exception_info.get("exception_message")
        if typ and message:
            return f"{typ}: {message}"
        if typ:
            return str(typ)
        if message:
            return str(message)
    return str(exception_info)


def classify_failure(row: dict[str, Any]) -> str:
    reward = row.get("reward")
    if isinstance(reward, (int, float)) and reward >= 1.0:
        return "solved"

    evidence = "\n".join(
        str(row.get(key) or "")
        for key in (
            "exception",
            "verifier_exception",
            "artifact_preflight_excerpt",
            "background_error",
            "transcript_excerpt",
        )
    )
    if ARTIFACT_INCOMPATIBLE_RE.search(evidence):
        return "artifact_incompatible"

    denied_counts = row.get("denied_tool_counts")
    if isinstance(denied_counts, dict):
        repeated = [
            (tool, int(count))
            for tool, count in denied_counts.items()
            if isinstance(count, int) and count >= TOOL_POLICY_LOOP_THRESHOLD
        ]
        if repeated:
            tool, count = sorted(repeated, key=lambda item: (-item[1], item[0]))[0]
            row["denied_tool"] = tool
            row["denied_tool_repeat_count"] = count
            return "tool_policy_loop"

    if BACKGROUND_NOT_READY_RE.search(evidence):
        return "background_not_ready"
    if VERIFIER_ENVIRONMENT_RE.search(evidence):
        return "verifier_environment_failure"
    if CONTEXT_EXHAUSTION_RE.search(evidence):
        return "context_exhaustion"
    if "timeout" in evidence.lower() or "timed out" in evidence.lower():
        return "setup_timeout"
    if row.get("exception") or row.get("verifier_exception"):
        return "harness_exception"
    return "model_wrong_answer"


def short_excerpt(text: str, max_chars: int = 1200) -> str | None:
    clean = text.strip()
    if not clean:
        return None
    if len(clean) <= max_chars:
        return clean
    return clean[: max_chars - 3] + "..."


def parse_trial(trial_dir: Path, model: str, reasoning_effort: str | None = None) -> dict[str, Any] | None:
    data = json_load(trial_dir / "result.json")
    if data is None or "task_name" not in data:
        return None
    agent_result = data.get("agent_result") if isinstance(data.get("agent_result"), dict) else {}
    verifier = data.get("verifier_result") if isinstance(data.get("verifier_result"), dict) else {}
    rewards = verifier.get("rewards") if isinstance(verifier.get("rewards"), dict) else {}
    row: dict[str, Any] = {
        "model": model,
        "reasoning_effort": reasoning_effort,
        "task": data.get("task_name"),
        "trial_dir": stable_path(trial_dir),
        "reward": rewards.get("reward"),
        "exception": parse_exception(data.get("exception_info")),
        "verifier_exception": parse_exception(verifier.get("exception_info")),
        "failure_class": None,
        "readiness_probe": readiness_probe_for_task(str(data.get("task_name") or "")),
        "denied_tool": None,
        "denied_tool_repeat_count": 0,
        "denied_tool_counts": {},
        "runtime_s": seconds_between(data.get("started_at"), data.get("finished_at")),
        "input_tokens": agent_result.get("n_input_tokens"),
        "cached_tokens": agent_result.get("n_cache_tokens"),
        "output_tokens": agent_result.get("n_output_tokens"),
        "reasoning_tokens": None,
        "cost_usd": agent_result.get("cost_usd"),
        "transcript_path": None,
        "transcript_bytes": None,
        "artifact_preflight_path": None,
        "artifact_preflight_excerpt": None,
        "harness_note_path": None,
    }
    for log_name in (
        "codewhale.txt",
        "direct-deepseek.jsonl",
        "mini-swe-agent.txt",
        "codex.txt",
        "oracle.txt",
    ):
        log_path = trial_dir / "agent" / log_name
        if log_path.exists():
            parse_agent_log(log_path, row)
            break
    preflight_path = trial_dir / "agent" / "codewhale-artifact-preflight.txt"
    preflight_text = read_text_if_exists(preflight_path)
    if preflight_text:
        row["artifact_preflight_path"] = stable_path(preflight_path)
        row["artifact_preflight_excerpt"] = short_excerpt(preflight_text)
    harness_note_path = trial_dir / "agent" / "codewhale-harness-note.txt"
    if harness_note_path.exists():
        row["harness_note_path"] = stable_path(harness_note_path)
    metadata = agent_result.get("metadata")
    if isinstance(metadata, dict) and row.get("reasoning_tokens") is None:
        reasoning_tokens = metadata.get("reasoning_tokens")
        if isinstance(reasoning_tokens, (int, float)):
            row["reasoning_tokens"] = reasoning_tokens
        if row.get("readiness_probe") is None and isinstance(metadata.get("readiness_probe"), str):
            row["readiness_probe"] = metadata.get("readiness_probe")
    row["failure_class"] = classify_failure(row)
    return row


def parse_job(job_dir: Path, model: str, reasoning_effort: str | None = None) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for result_path in sorted(job_dir.glob("*__*/result.json")):
        trial = parse_trial(result_path.parent, model, reasoning_effort)
        if trial:
            rows.append(trial)
    return rows


def parse_run_dir(run_dir: Path) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    metadata = json_load(run_dir / "metadata.json") or {}
    model_by_job = metadata.get("model_by_job", {})
    if not isinstance(model_by_job, dict):
        model_by_job = {}
    effort_by_job = metadata.get("reasoning_effort_by_job", {})
    if not isinstance(effort_by_job, dict):
        effort_by_job = {}
    for job_dir in sorted(run_dir.iterdir()):
        if not job_dir.is_dir():
            continue
        model = model_by_job.get(job_dir.name)
        if not model:
            config = json_load(job_dir / "config.json") or {}
            models = config.get("models") or config.get("model")
            if isinstance(models, list) and models:
                model = str(models[0])
            elif isinstance(models, str):
                model = models
            else:
                model = job_dir.name
        effort = effort_by_job.get(job_dir.name)
        rows.extend(parse_job(job_dir, str(model), str(effort) if effort else None))
    return rows


def aggregate(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    groups: dict[str, list[dict[str, Any]]] = {}
    for row in rows:
        groups.setdefault(str(row.get("model")), []).append(row)
    out: list[dict[str, Any]] = []
    for model, model_rows in sorted(groups.items()):
        rewards = [float(r["reward"]) for r in model_rows if isinstance(r.get("reward"), (int, float))]
        runtimes = [float(r["runtime_s"]) for r in model_rows if isinstance(r.get("runtime_s"), (int, float))]
        failure_classes: dict[str, int] = {}
        for row in model_rows:
            failure_class = str(row.get("failure_class") or "harness_exception")
            failure_classes[failure_class] = failure_classes.get(failure_class, 0) + 1
        out.append(
            {
                "model": model,
                "trials": len(model_rows),
                "solved": sum(1 for reward in rewards if reward >= 1.0),
                "mean_reward": round(sum(rewards) / len(rewards), 4) if rewards else None,
                "exceptions": sum(1 for row in model_rows if row.get("exception")),
                "failure_classes": failure_classes,
                "mean_runtime_s": round(sum(runtimes) / len(runtimes), 2) if runtimes else None,
                "input_tokens": sum(int(r.get("input_tokens") or 0) for r in model_rows) or None,
                "cached_tokens": sum(int(r.get("cached_tokens") or 0) for r in model_rows) or None,
                "output_tokens": sum(int(r.get("output_tokens") or 0) for r in model_rows) or None,
                "reasoning_tokens": sum(int(r.get("reasoning_tokens") or 0) for r in model_rows) or None,
                "cost_usd": round(sum(float(r.get("cost_usd") or 0.0) for r in model_rows), 6) or None,
            }
        )
    return out


def markdown(rows: list[dict[str, Any]], aggregates: list[dict[str, Any]]) -> str:
    lines = ["# CodeWhale Terminal-Bench Summary", ""]
    lines.append("## Aggregate")
    lines.append("")
    lines.append("| model | trials | solved | mean reward | exceptions | failure classes | mean runtime s | input tokens | output tokens | reasoning tokens | cost usd |")
    lines.append("| --- | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: |")
    for row in aggregates:
        rendered = {k: ("null" if v is None else v) for k, v in row.items()}
        rendered["failure_classes"] = json.dumps(
            row.get("failure_classes") or {},
            sort_keys=True,
            separators=(",", ":"),
        )
        lines.append(
            "| {model} | {trials} | {solved} | {mean_reward} | {exceptions} | {failure_classes} | {mean_runtime_s} | {input_tokens} | {output_tokens} | {reasoning_tokens} | {cost_usd} |".format(
                **rendered
            )
        )
    lines.extend(["", "## Per Task", ""])
    lines.append("| model | effort | task | reward | failure class | denied tool | exception | runtime s | input tokens | output tokens | transcript |")
    lines.append("| --- | --- | --- | ---: | --- | --- | --- | ---: | ---: | ---: | --- |")
    for row in sorted(rows, key=lambda r: (str(r.get("model")), str(r.get("task")))):
        exception = str(row.get("exception") or "")
        if len(exception) > 90:
            exception = exception[:87] + "..."
        denied_tool = row.get("denied_tool") or ""
        repeat_count = row.get("denied_tool_repeat_count") or 0
        if denied_tool and repeat_count:
            denied_tool = f"{denied_tool} x{repeat_count}"
        lines.append(
            "| {model} | {reasoning_effort} | {task} | {reward} | {failure_class} | {denied_tool} | {exception} | {runtime_s} | {input_tokens} | {output_tokens} | {transcript_path} |".format(
                model=row.get("model"),
                reasoning_effort=row.get("reasoning_effort") or "default",
                task=row.get("task"),
                reward="null" if row.get("reward") is None else row.get("reward"),
                failure_class=row.get("failure_class") or "",
                denied_tool=str(denied_tool).replace("|", "\\|"),
                exception=exception.replace("|", "\\|"),
                runtime_s="null" if row.get("runtime_s") is None else row.get("runtime_s"),
                input_tokens="null" if row.get("input_tokens") is None else row.get("input_tokens"),
                output_tokens="null" if row.get("output_tokens") is None else row.get("output_tokens"),
                transcript_path=row.get("transcript_path") or "",
            )
        )
    lines.append("")
    return "\n".join(lines)


def write_summaries(run_dir: Path) -> None:
    rows = parse_run_dir(run_dir)
    aggregates = aggregate(rows)
    (run_dir / "summary.json").write_text(
        json.dumps({"aggregate": aggregates, "rows": rows}, indent=2, sort_keys=True)
    )
    (run_dir / "summary.md").write_text(markdown(rows, aggregates))
    print(markdown(rows, aggregates))


def run_matrix(args: argparse.Namespace, env: dict[str, str]) -> Path:
    timestamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    run_dir = args.results_root / timestamp
    run_dir.mkdir(parents=True, exist_ok=False)
    model_by_job: dict[str, str] = {}
    effort_by_job: dict[str, str | None] = {}
    metadata = {
        "created_at_utc": datetime.now(timezone.utc).isoformat(),
        "dataset": args.dataset,
        "tasks": args.tasks,
        "models": args.models,
        "reasoning_efforts": args.reasoning_efforts or ["default"],
        "agent_import_path": args.agent_import_path,
        "linux_bin": str(args.linux_bin) if args.linux_bin else None,
        "tui_linux_bin": str(args.tui_linux_bin) if args.tui_linux_bin else None,
        "artifact_preflight_commands": ARTIFACT_PREFLIGHT_COMMANDS,
        "failure_classes": list(FAILURE_CLASSES),
        "harness_timeouts": HARNESS_TIMEOUTS,
        "task_harness": task_harness_metadata(args.tasks),
        "credential_env_present": {
            env_key_for_provider(provider_from_model(model)): bool(env.get(env_key_for_provider(provider_from_model(model))))
            for model in args.models
        },
        "model_by_job": model_by_job,
        "reasoning_effort_by_job": effort_by_job,
    }

    for model in args.models:
        for reasoning_effort in (args.reasoning_efforts or [None]):
            safe_model = model.replace("/", "_").replace(":", "_")
            safe_effort = reasoning_effort or "default"
            job_name = f"codewhale-{safe_model}-thinking-{safe_effort}-{timestamp}"
            model_by_job[job_name] = label_for_model(model, reasoning_effort)
            effort_by_job[job_name] = reasoning_effort
            (run_dir / "metadata.json").write_text(json.dumps(metadata, indent=2, sort_keys=True))
            cmd = [
                "harbor",
                "run",
                "-d",
                args.dataset,
                "--agent-import-path",
                args.agent_import_path,
                "-m",
                model,
                "-n",
                str(args.concurrency),
                "--job-name",
                job_name,
                "-o",
                str(run_dir),
                "--agent-include-logs",
                "codewhale.txt",
                "--agent-include-logs",
                "codewhale-artifact-preflight.txt",
                "--agent-include-logs",
                "codewhale-harness-note.txt",
                "--yes",
            ]
            if reasoning_effort:
                cmd.extend(["--agent-kwarg", f"reasoning_effort={reasoning_effort}"])
            for task in args.tasks:
                cmd.extend(["--include-task-name", task])
            if args.max_retries:
                cmd.extend(["--max-retries", str(args.max_retries)])
            if args.timeout_multiplier != 1.0:
                cmd.extend(["--timeout-multiplier", str(args.timeout_multiplier)])
            if args.dry_run:
                print("$ " + " ".join(cmd))
                continue
            exit_code = run_command(cmd, env=env, timeout=args.wall_timeout)
            write_summaries(run_dir)
            if exit_code != 0:
                raise SystemExit(exit_code)

    (run_dir / "metadata.json").write_text(json.dumps(metadata, indent=2, sort_keys=True))
    return run_dir


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dataset", default=DEFAULT_DATASET)
    parser.add_argument("--task", dest="tasks", action="append", default=[])
    parser.add_argument("--model", dest="models", action="append", default=[])
    parser.add_argument(
        "--reasoning-effort",
        dest="reasoning_efforts",
        action="append",
        choices=EXPLICIT_REASONING_EFFORTS,
        default=[],
        help="Explicit CodeWhale reasoning tier to benchmark; repeat for a matrix.",
    )
    parser.add_argument("--agent-import-path", default=DEFAULT_AGENT)
    parser.add_argument("--results-root", type=Path, default=DEFAULT_RESULTS_ROOT)
    parser.add_argument(
        "--linux-bin",
        type=Path,
        default=None,
        help=f"Host path to the Linux codewhale binary; defaults to {CODEWHALE_LINUX_BIN_ENV}.",
    )
    parser.add_argument(
        "--tui-linux-bin",
        type=Path,
        default=None,
        help=(
            "Host path to the Linux codewhale-tui binary; defaults to "
            f"{CODEWHALE_TUI_LINUX_BIN_ENV}."
        ),
    )
    parser.add_argument("--concurrency", type=int, default=1)
    parser.add_argument("--max-retries", type=int, default=0)
    parser.add_argument("--timeout-multiplier", type=float, default=1.0)
    parser.add_argument("--wall-timeout", type=int, default=None)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--regenerate", type=Path)
    args = parser.parse_args()

    args.tasks = args.tasks or DEFAULT_TASKS
    args.models = args.models or DEFAULT_MODELS
    args.linux_bin = resolve_artifact_path(args.linux_bin, CODEWHALE_LINUX_BIN_ENV)
    args.tui_linux_bin = resolve_artifact_path(
        args.tui_linux_bin,
        CODEWHALE_TUI_LINUX_BIN_ENV,
    )

    if args.regenerate:
        write_summaries(args.regenerate)
        return

    env = build_env(args.models, args.linux_bin, args.tui_linux_bin)
    validate_prereqs(args, env)
    run_dir = run_matrix(args, env)
    write_summaries(run_dir)
    print(f"results_dir={run_dir}")


if __name__ == "__main__":
    main()
