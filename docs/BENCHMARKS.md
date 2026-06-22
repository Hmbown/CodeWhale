# Benchmarks

CodeWhale integrates with three external benchmarks to measure real-world
coding-agent performance. Each benchmark tests a different surface:

| Benchmark | What it tests | Harness | Output format |
|---|---|---|---|
| **SWE-bench** | Patch generation from GitHub issues | CodeWhale built-in (`codewhale swebench`) | `all_preds.jsonl` |
| **Terminal-Bench** | End-to-end terminal tasks (compile, deploy, configure) | Harbor framework adapter | Harbor result JSON |
| **PinchBench** | Real-world agent tasks (calendar, email, coding, research) | Standalone runner via OpenClaw-compatible adapter | PinchBench result JSON |

All three require Docker. SWE-bench and Terminal-Bench also need the official
evaluation harness installed separately.

## Prerequisites

```bash
# Docker (all benchmarks)
docker --version

# Python 3.10+ with uv (Terminal-Bench, PinchBench, SWE-bench eval)
python3 --version
uv --version

# CodeWhale v0.8.53+
codewhale --version

# API key
export DEEPSEEK_API_KEY="sk-..."
```

## SWE-bench

CodeWhale has built-in SWE-bench support via `codewhale swebench run` and
`codewhale swebench export`. See [docs/SWEBENCH.md](SWEBENCH.md) for the
single-instance workflow.

### Batch run

```bash
# Run all instances from a dataset split
./scripts/benchmarks/run-swebench.sh \
  --dataset princeton-nlp/SWE-bench_Lite \
  --split test \
  --predictions-path ./results/swebench_preds.jsonl

# Run a single instance
./scripts/benchmarks/run-swebench.sh \
  --instance-id django__django-12345 \
  --issue-file ./issue.md \
  --predictions-path ./results/swebench_preds.jsonl
```

### Evaluate

```bash
python -m swebench.harness.run_evaluation \
  --dataset_name princeton-nlp/SWE-bench_Lite \
  --predictions_path ./results/swebench_preds.jsonl \
  --max_workers 1 \
  --run_id codewhale-v0.8.53
```

## Terminal-Bench (via Harbor)

Terminal-Bench tests agents on real terminal tasks — compiling, deploying,
configuring servers, training models. The [Harbor framework](https://github.com/harbor-framework/harbor)
is the official harness.

CodeWhale plugs in via a Harbor adapter (`scripts/benchmarks/harbor/codewhale_agent.py`).

### Setup

```bash
pip install harbor
```

### Run

```bash
# Via the convenience script
./scripts/benchmarks/run-terminal-bench.sh \
  --dataset terminal-bench@2.0 \
  --model deepseek/deepseek-chat \
  --n-concurrent 4

# Or directly with harbor
harbor run \
  --dataset terminal-bench@2.0 \
  --agent codewhale \
  --model deepseek/deepseek-chat \
  --n-concurrent 4
```

### Custom agent path

If the adapter is not installed system-wide, point Harbor at it:

```bash
harbor run \
  --dataset terminal-bench@2.0 \
  --agent scripts.benchmarks.harbor.codewhale_agent:CodeWhaleAgent \
  --model deepseek/deepseek-chat
```

### Compare CodeWhale and Codex

Use the paired comparison harness when you need one normalized row per CLI for
the same task, model, timeout, and environment:

```bash
python scripts/benchmarks/cli-compare.py \
  --task prove-plus-comm \
  --model deepseek/deepseek-chat \
  --runs 3
```

The harness writes raw Harbor logs plus `summary.json`, `summary.md`, and
`metadata.json` under `benchmark_results/cli-compare-*`. Missing metrics are
reported as JSON `null`, and generated run directories are intentionally ignored
by git; keep only curated summaries in docs or release notes.

### Compare local release artifacts against baselines

Use the local-artifact runner when npm still points at the previous public
release and you need Terminal-Bench rows for a candidate branch. It uploads
explicit Linux `codewhale` and `codewhale-tui` binaries into each Harbor task
container, so the benchmark evidence is tied to the intended build instead of
whatever npm currently serves.

```bash
export CODEWHALE_LINUX_BIN=/path/to/codewhale-linux-x64-0.8.63
export CODEWHALE_TUI_LINUX_BIN=/path/to/codewhale-tui-linux-x64-0.8.63

python scripts/benchmarks/run-codewhale-terminal-bench.py \
  --task build-cython-ext \
  --model deepseek/deepseek-v4-flash \
  --reasoning-effort off
```

Run the thin direct DeepSeek baseline and stock mini-swe-agent baseline with
matching task/model settings when you need comparison rows:

```bash
python scripts/benchmarks/run-deepseek-direct-terminal-bench.py \
  --task build-cython-ext \
  --model deepseek/deepseek-v4-flash \
  --reasoning-effort off

python scripts/benchmarks/run-mini-swe-terminal-bench.py \
  --task build-cython-ext \
  --model deepseek/deepseek-v4-flash
```

All three runners support `--dry-run` to print the Harbor command and write
metadata scaffolding without launching task containers. Generated run
directories stay under `benchmark_results/` and remain ignored by git.

## PinchBench

PinchBench measures agent performance on real-world tasks — scheduling, email
triage, code generation, research, file management. It uses OpenClaw as the
agent runtime.

### Setup

```bash
./scripts/benchmarks/run-pinchbench.sh --install
```

### Run (MiMo v2.5 Pro — default)

```bash
# MiMo v2.5 Pro via OpenRouter (default)
./scripts/benchmarks/run-pinchbench.sh

# MiMo v2.5 Pro via direct Xiaomi API
./scripts/benchmarks/run-pinchbench.sh --direct-mimo

# Specific tasks
./scripts/benchmarks/run-pinchbench.sh --suite task_calendar,task_stock
```

### Run (other models)

```bash
./scripts/benchmarks/run-pinchbench.sh --model openrouter/deepseek/deepseek-v4-pro
```

### MiMo v2.5 notes

PinchBench routes through OpenRouter by default. MiMo models are available as
`openrouter/xiaomi/mimo-v2.5-pro` (Pro) and `openrouter/xiaomi/mimo-v2.5`
(Omni). For direct Xiaomi API access, use `--direct-mimo` with
`XIAOMI_MIMO_API_KEY` set.

See `scripts/benchmarks/run-pinchbench.sh --help` for full option reference.

## Reproducibility checklist

When publishing benchmark results, record:

- [ ] CodeWhale version: `codewhale --version`
- [ ] Git commit: `git rev-parse HEAD`
- [ ] Model and provider (e.g. `deepseek/deepseek-chat`)
- [ ] Benchmark dataset and version
- [ ] Docker platform (`linux/amd64` vs `linux/arm64`)
- [ ] Worker concurrency
- [ ] Timestamp (UTC)
- [ ] Full result file (`all_preds.jsonl`, Harbor result dir, or PinchBench results JSON)

## References

- SWE-bench: https://github.com/SWE-bench/SWE-bench
- Terminal-Bench: https://github.com/laude-institute/terminal-bench / https://www.tbench.ai
- Harbor: https://github.com/harbor-framework/harbor / https://harborframework.com
- PinchBench: https://github.com/pinchbench/skill / https://pinchbench.com
