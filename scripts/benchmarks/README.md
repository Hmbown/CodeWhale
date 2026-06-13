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

# PinchBench (auto-install + run)
./scripts/benchmarks/run-pinchbench.sh \
  --install \
  --model deepseek/deepseek-chat
```

## Files

- `run-swebench.sh` — SWE-bench batch driver and evaluator
- `run-terminal-bench.sh` — Terminal-Bench runner via Harbor
- `run-pinchbench.sh` — PinchBench runner with auto-install
- `harbor/__init__.py` — Harbor adapter for CodeWhale (Python)
- `harbor/codewhale_agent.py` — Adapter entry point

## Documentation

See [docs/BENCHMARKS.md](../../docs/BENCHMARKS.md) for full setup instructions,
reproducibility checklists, and references.
