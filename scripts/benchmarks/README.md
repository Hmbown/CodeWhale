# Benchmark Scripts

Convenience runners for evaluating CodeWhale against external benchmarks.

## Quick Start

```bash
# Set your API key
export DEEPSEEK_API_KEY="sk-..."

# SWE-bench (single instance)
./scripts/benchmarks/run-swebench.sh \
  --instance-id django__django-12345 \
  --issue-file ./issue.md

# Terminal-Bench (via Harbor)
./scripts/benchmarks/run-terminal-bench.sh \
  --model deepseek/deepseek-chat

# CodeWhale vs Codex comparison rows
python scripts/benchmarks/cli-compare.py \
  --task prove-plus-comm \
  --model deepseek/deepseek-chat

# Local release artifact vs direct baselines on Terminal-Bench sample
export CODEWHALE_LINUX_BIN=/path/to/codewhale-linux-x64-0.8.63
export CODEWHALE_TUI_LINUX_BIN=/path/to/codewhale-tui-linux-x64-0.8.63
python scripts/benchmarks/run-codewhale-terminal-bench.py \
  --dry-run \
  --task build-cython-ext \
  --model deepseek/deepseek-v4-flash

# PinchBench (auto-install + run)
./scripts/benchmarks/run-pinchbench.sh \
  --install \
  --model deepseek/deepseek-chat
```

## Files

- `run-swebench.sh` — SWE-bench batch driver and evaluator
- `run-terminal-bench.sh` — Terminal-Bench runner via Harbor
- `run-codewhale-terminal-bench.py` — Terminal-Bench runner for explicit
  local Linux CodeWhale release artifacts
- `run-deepseek-direct-terminal-bench.py` — thin direct DeepSeek API baseline
- `run-mini-swe-terminal-bench.py` — stock mini-swe-agent Terminal-Bench
  baseline
- `run-pinchbench.sh` — PinchBench runner with auto-install
- `cli-compare.py` — CodeWhale/Codex Terminal-Bench comparison harness
- `harbor/__init__.py` — Harbor adapter for CodeWhale (Python)
- `harbor/codewhale_agent.py` — Adapter entry point
- `harbor/codewhale_local_agent.py` — Adapter that uploads explicit local
  Linux CodeWhale artifacts into Harbor task containers
- `harbor/deepseek_direct_agent.py` — Direct DeepSeek chat-completions
  baseline with minimal shell/file tools
- `harbor/codex_agent.py` — Codex adapter for paired CLI comparisons

## Documentation

See [docs/BENCHMARKS.md](../../docs/BENCHMARKS.md) for full setup instructions,
reproducibility checklists, and references.

## Terminal-Bench Harness Diagnostics

The local CodeWhale Terminal-Bench adapter runs an artifact preflight inside
each task container before the agent starts:

```bash
codewhale --version
ldd "$(command -v codewhale)"
/lib/x86_64-linux-gnu/libc.so.6 || true
```

Rows with loader, glibc, OpenSSL, or related library failures are classified as
`artifact_incompatible` instead of model failures. The adapter also injects a
compact harness note listing detected verifier surfaces, task-specific
readiness probes when known, background service helpers, and timeout classes.

Summary rows include one primary `failure_class`:

```text
solved
model_wrong_answer
tool_policy_loop
artifact_incompatible
setup_timeout
background_not_ready
verifier_environment_failure
context_exhaustion
harness_exception
```
