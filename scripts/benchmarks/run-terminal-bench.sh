#!/usr/bin/env bash
# run-terminal-bench.sh — Run CodeWhale on Terminal-Bench via Harbor.
#
# Usage:
#   ./scripts/benchmarks/run-terminal-bench.sh --help
#   ./scripts/benchmarks/run-terminal-bench.sh --dataset terminal-bench@2.0 --model deepseek/deepseek-chat
#
# Prerequisites:
#   - pip install harbor
#   - Docker running
#   - DEEPSEEK_API_KEY or OPENROUTER_API_KEY set

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Defaults
DATASET="terminal-bench@2.0"
MODEL="deepseek/deepseek-chat"
N_CONCURRENT=4
AGENT_PATH="$SCRIPT_DIR/harbor/__init__.py:CodeWhaleAgent"
RESULTS_DIR="./results/terminal-bench"
EXTRA_ARGS=()

usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Run CodeWhale on Terminal-Bench tasks via the Harbor framework.

Options:
  --dataset DATASET       Harbor dataset (default: terminal-bench@2.0)
  --model MODEL           Model in provider/name format (default: deepseek/deepseek-chat)
  --agent PATH            Harbor agent import path (default: local CodeWhale adapter)
  --n-concurrent N        Parallel task workers (default: 4)
  --results-dir DIR       Results output directory (default: ./results/terminal-bench)
  -- [EXTRA_ARGS...]      Additional arguments passed to 'harbor run'
  -h, --help              Show this help

Examples:
  # Default run
  $(basename "$0")

  # Custom model and concurrency
  $(basename "$0") --model deepseek/deepseek-reasoner --n-concurrent 8

  # Pass extra flags to harbor
  $(basename "$0") -- --env daytona
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dataset) DATASET="$2"; shift 2 ;;
        --model) MODEL="$2"; shift 2 ;;
        --agent) AGENT_PATH="$2"; shift 2 ;;
        --n-concurrent) N_CONCURRENT="$2"; shift 2 ;;
        --results-dir) RESULTS_DIR="$2"; shift 2 ;;
        --) shift; EXTRA_ARGS=("$@"); break ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Unknown option: $1" >&2; usage >&2; exit 1 ;;
    esac
done

# Check prerequisites
if ! command -v harbor &>/dev/null; then
    echo "Error: 'harbor' not found. Install with: pip install harbor" >&2
    exit 1
fi

if ! command -v docker &>/dev/null; then
    echo "Error: Docker not found. Harbor requires Docker." >&2
    exit 1
fi

mkdir -p "$RESULTS_DIR"

# Record metadata
METADATA_FILE="$RESULTS_DIR/run_metadata.json"
cat > "$METADATA_FILE" <<META
{
    "codewhale_version": "$(codewhale --version 2>/dev/null || echo unknown)",
    "git_commit": "$(cd "$REPO_ROOT" && git rev-parse HEAD 2>/dev/null || echo unknown)",
    "harbor_version": "$(harbor --version 2>/dev/null || echo unknown)",
    "model": "$MODEL",
    "dataset": "$DATASET",
    "agent": "codewhale",
    "n_concurrent": $N_CONCURRENT,
    "timestamp_utc": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
    "platform": "$(uname -s)/$(uname -m)"
}
META
echo "Run metadata: $METADATA_FILE"

# Run Harbor
echo "Running Terminal-Bench via Harbor..."
echo "  Dataset:   $DATASET"
echo "  Model:     $MODEL"
echo "  Agent:     $AGENT_PATH"
echo "  Workers:   $N_CONCURRENT"
echo ""

harbor run \
    --dataset "$DATASET" \
    --agent "$AGENT_PATH" \
    --model "$MODEL" \
    --n-concurrent "$N_CONCURRENT" \
    --results-dir "$RESULTS_DIR" \
    "${EXTRA_ARGS[@]}"

echo ""
echo "Results written to $RESULTS_DIR"
