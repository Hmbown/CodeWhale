#!/usr/bin/env bash
# run-swebench.sh — Batch driver for CodeWhale SWE-bench runs.
#
# Usage:
#   ./scripts/benchmarks/run-swebench.sh --help
#   ./scripts/benchmarks/run-swebench.sh --dataset princeton-nlp/SWE-bench_Lite --split test
#   ./scripts/benchmarks/run-swebench.sh --instance-id django__django-12345 --issue-file issue.md
#
# Prerequisites:
#   - codewhale installed and on PATH
#   - DEEPSEEK_API_KEY set (or appropriate provider key)
#   - swebench pip package installed (for evaluation step)
#   - Docker running (for evaluation step)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Defaults
DATASET=""
SPLIT="test"
INSTANCE_ID=""
ISSUE_FILE=""
PREDICTIONS_PATH="./results/swebench_preds.jsonl"
MODEL=""
WORKSPACE_BASE="/tmp/swebench-workspaces"
EVAL_ONLY=false
MAX_WORKERS=1

usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Run CodeWhale on SWE-bench instances and produce prediction JSONL.

Options:
  --dataset DATASET       HuggingFace dataset name (e.g. princeton-nlp/SWE-bench_Lite)
  --split SPLIT           Dataset split (default: test)
  --instance-id ID        Run a single instance by ID
  --issue-file PATH       Issue text file for single-instance mode
  --predictions-path PATH Output JSONL file (default: ./results/swebench_preds.jsonl)
  --model MODEL           Model override for CodeWhale
  --workspace-base DIR    Base dir for instance workspaces (default: /tmp/swebench-workspaces)
  --eval-only             Skip runs; just evaluate existing predictions file
  --max-workers N         Parallel workers for evaluation (default: 1)
  -h, --help              Show this help

Examples:
  # Run all instances from SWE-bench Lite
  $(basename "$0") --dataset princeton-nlp/SWE-bench_Lite --split test

  # Run a single instance
  $(basename "$0") --instance-id django__django-12345 --issue-file ./issue.md

  # Evaluate existing predictions
  $(basename "$0") --eval-only --predictions-path ./results/swebench_preds.jsonl
EOF
}

# Parse args
while [[ $# -gt 0 ]]; do
    case "$1" in
        --dataset) DATASET="$2"; shift 2 ;;
        --split) SPLIT="$2"; shift 2 ;;
        --instance-id) INSTANCE_ID="$2"; shift 2 ;;
        --issue-file) ISSUE_FILE="$2"; shift 2 ;;
        --predictions-path) PREDICTIONS_PATH="$2"; shift 2 ;;
        --model) MODEL="$2"; shift 2 ;;
        --workspace-base) WORKSPACE_BASE="$2"; shift 2 ;;
        --eval-only) EVAL_ONLY=true; shift ;;
        --max-workers) MAX_WORKERS="$2"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Unknown option: $1" >&2; usage >&2; exit 1 ;;
    esac
done

mkdir -p "$(dirname "$PREDICTIONS_PATH")" "$WORKSPACE_BASE"

# Record run metadata
METADATA_FILE="$(dirname "$PREDICTIONS_PATH")/run_metadata.json"
cat > "$METADATA_FILE" <<META
{
    "codewhale_version": "$(codewhale --version 2>/dev/null || echo unknown)",
    "git_commit": "$(cd "$REPO_ROOT" && git rev-parse HEAD 2>/dev/null || echo unknown)",
    "model": "${MODEL:-default}",
    "dataset": "${DATASET:-single-instance}",
    "split": "${SPLIT}",
    "timestamp_utc": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
    "platform": "$(uname -s)/$(uname -m)"
}
META
echo "Run metadata written to $METADATA_FILE"

run_single_instance() {
    local id="$1"
    local workspace="$WORKSPACE_BASE/$id"

    echo "=== Running instance: $id ==="

    # Clone or checkout the instance workspace
    if [[ ! -d "$workspace" ]]; then
        echo "  Workspace not found at $workspace"
        echo "  For batch mode, pre-clone instance repos into $WORKSPACE_BASE/"
        echo "  For single instance, use --issue-file with an existing workspace"
        return 1
    fi

    cd "$workspace"

    # Write issue file if provided
    if [[ -n "$ISSUE_FILE" && -f "$ISSUE_FILE" ]]; then
        cp "$ISSUE_FILE" "$workspace/issue.md"
    fi

    # Build the codewhale command
    local cw_args=("swebench" "run"
        "--instance-id" "$id"
        "--predictions-path" "$PREDICTIONS_PATH"
    )

    if [[ -n "$MODEL" ]]; then
        cw_args+=("--model" "$MODEL")
    fi

    codewhale "${cw_args[@]}"
    echo "  Prediction written for $id"
}

if [[ "$EVAL_ONLY" == true ]]; then
    echo "Evaluating existing predictions at $PREDICTIONS_PATH ..."
    python -m swebench.harness.run_evaluation \
        --dataset_name "${DATASET:-princeton-nlp/SWE-bench_Lite}" \
        --predictions_path "$PREDICTIONS_PATH" \
        --max_workers "$MAX_WORKERS" \
        --run_id "codewhale-$(date -u +%Y%m%d-%H%M%S)"
    exit 0
fi

if [[ -n "$INSTANCE_ID" ]]; then
    # Single-instance mode
    run_single_instance "$INSTANCE_ID"
elif [[ -n "$DATASET" ]]; then
    # Batch mode: requires a pre-prepared workspace directory structure
    echo "Batch mode for dataset: $DATASET (split: $SPLIT)"
    echo ""
    echo "To run batch SWE-bench:"
    echo "  1. Install swebench: pip install swebench"
    echo "  2. Prepare instance workspaces in $WORKSPACE_BASE/"
    echo "  3. For each instance, run:"
    echo "     $0 --instance-id <ID> --predictions-path $PREDICTIONS_PATH"
    echo "  4. Then evaluate:"
    echo "     $0 --eval-only --dataset $DATASET --predictions-path $PREDICTIONS_PATH"
    echo ""
    echo "Automated batch orchestration is planned for v0.9.0."
    echo "For now, use the SWE-bench docker harness to prepare workspaces."
else
    echo "Error: specify --dataset or --instance-id" >&2
    usage >&2
    exit 1
fi
