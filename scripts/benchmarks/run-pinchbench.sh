#!/usr/bin/env bash
# run-pinchbench.sh — Run PinchBench benchmarks with Xiaomi MiMo v2.5.
#
# Defaults to direct Xiaomi API routing (no OpenRouter needed). Reads the
# API key from ~/.codewhale/config.toml if not set via environment variables.
#
# Usage:
#   ./scripts/benchmarks/run-pinchbench.sh --help
#   ./scripts/benchmarks/run-pinchbench.sh                    # direct MiMo (default)
#   ./scripts/benchmarks/run-pinchbench.sh --openrouter       # via OpenRouter
#   ./scripts/benchmarks/run-pinchbench.sh --suite task_calendar
#
# Prerequisites:
#   - PinchBench cloned (or use --install)
#   - Python 3.10+ with uv
#   - Xiaomi MiMo API key (in env or ~/.codewhale/config.toml)
#   - A running OpenClaw instance

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
CODEWHALE_CONFIG="${HOME}/.codewhale/config.toml"

# Defaults — direct MiMo v2.5 Pro (no OpenRouter)
MODEL="mimo-v2.5-pro"
SUITE="all"
PINCHBENCH_DIR="${PINCHBENCH_DIR:-/tmp/pinchbench}"
RESULTS_DIR="./results/pinchbench"
INSTALL_PINCHBENCH=false
RUNS=1
JUDGE_MODEL=""
NO_UPLOAD=true
DIRECT_MIMO=true
MIMO_BASE_URL=""
OPENROUTER_MODE=false
EXTRA_ARGS=()

usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Run PinchBench benchmarks. Defaults to Xiaomi MiMo v2.5 Pro via direct API.

Options:
  --model MODEL           Model ID (default: mimo-v2.5-pro)
                          Common values:
                            mimo-v2.5-pro       — MiMo Pro (direct Xiaomi API)
                            mimo-v2.5           — MiMo Omni (direct Xiaomi API)
  --suite SUITE           Task suite: all, automated-only, or comma-separated IDs
  --runs N                Runs per task for averaging (default: 1)
  --judge MODEL           Judge model for LLM grading (default: uses OpenClaw agent)
  --openrouter            Route via OpenRouter instead of direct Xiaomi API
                          Requires OPENROUTER_API_KEY. Uses xiaomi/mimo-v2.5-pro.
  --mimo-base-url URL     Override MiMo API base URL (default: from config or Token Plan SG)
  --pinchbench-dir DIR    PinchBench install directory (default: /tmp/pinchbench)
  --results-dir DIR       Local results directory (default: ./results/pinchbench)
  --install               Install/clone PinchBench before running
  --upload                Upload results to pinchbench.com leaderboard
  -- [EXTRA_ARGS...]      Additional arguments passed to PinchBench
  -h, --help              Show this help

Environment variables (direct mode):
  XIAOMI_MIMO_API_KEY     Xiaomi MiMo API key (or XIAOMI_API_KEY / MIMO_API_KEY)
                          Falls back to ~/.codewhale/config.toml if unset
  XIAOMI_MIMO_BASE_URL    Override MiMo API endpoint

Environment variables (OpenRouter mode):
  OPENROUTER_API_KEY      Required when using --openrouter

Examples:
  # Direct MiMo v2.5 Pro (default — no OpenRouter needed)
  $(basename "$0")

  # Install and run
  $(basename "$0") --install

  # Specific tasks
  $(basename "$0") --suite task_calendar,task_stock

  # Via OpenRouter instead
  $(basename "$0") --openrouter
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --model) MODEL="$2"; shift 2 ;;
        --suite) SUITE="$2"; shift 2 ;;
        --runs) RUNS="$2"; shift 2 ;;
        --judge) JUDGE_MODEL="$2"; shift 2 ;;
        --openrouter) OPENROUTER_MODE=true; DIRECT_MIMO=false; shift ;;
        --mimo-base-url) MIMO_BASE_URL="$2"; shift 2 ;;
        --pinchbench-dir) PINCHBENCH_DIR="$2"; shift 2 ;;
        --results-dir) RESULTS_DIR="$2"; shift 2 ;;
        --install) INSTALL_PINCHBENCH=true; shift ;;
        --upload) NO_UPLOAD=false; shift ;;
        --) shift; EXTRA_ARGS=("$@"); break ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Unknown option: $1" >&2; usage >&2; exit 1 ;;
    esac
done

# ── Read MiMo config from ~/.codewhale/config.toml ──────────────────────────
# Extracts api_key and base_url from [providers.xiaomi_mimo] section.
read_codewhale_mimo_config() {
    local config="$1"
    local key="" url=""
    if [[ -f "$config" ]]; then
        key=$(awk '/\[providers\.xiaomi_mimo\]/{f=1} f && /^api_key/{gsub(/.*= *"/,""); gsub(/".*/,""); print; exit}' "$config" 2>/dev/null || true)
        url=$(awk '/\[providers\.xiaomi_mimo\]/{f=1} f && /^base_url/{gsub(/.*= *"/,""); gsub(/".*/,""); print; exit}' "$config" 2>/dev/null || true)
    fi
    echo "$key|$url"
}

# ── OpenRouter mode ─────────────────────────────────────────────────────────
if [[ "$OPENROUTER_MODE" == true ]]; then
    MODEL="openrouter/xiaomi/mimo-v2.5-pro"
    if [[ -z "${OPENROUTER_API_KEY:-}" ]]; then
        echo "Error: --openrouter requires OPENROUTER_API_KEY" >&2
        exit 1
    fi
    echo "OpenRouter mode:"
    echo "  Model: $MODEL"
    echo ""

# ── Direct MiMo mode (default) ─────────────────────────────────────────────
elif [[ "$DIRECT_MIMO" == true ]]; then
    # Resolve API key: env var > codewhale config.toml
    MIMO_KEY="${XIAOMI_MIMO_API_KEY:-${XIAOMI_API_KEY:-${MIMO_API_KEY:-}}}"

    if [[ -z "$MIMO_KEY" ]]; then
        # Try reading from codewhale config
        IFS='|' read -r cfg_key cfg_url <<< "$(read_codewhale_mimo_config "$CODEWHALE_CONFIG")"
        if [[ -n "$cfg_key" ]]; then
            MIMO_KEY="$cfg_key"
            echo "Read MiMo API key from $CODEWHALE_CONFIG"
            # Use config base_url if not overridden
            if [[ -z "$MIMO_BASE_URL" && -n "$cfg_url" ]]; then
                MIMO_BASE_URL="$cfg_url"
            fi
        fi
    fi

    if [[ -z "$MIMO_KEY" ]]; then
        echo "Error: No MiMo API key found." >&2
        echo "  Set XIAOMI_MIMO_API_KEY env var, or configure [providers.xiaomi_mimo] in" >&2
        echo "  ~/.codewhale/config.toml" >&2
        exit 1
    fi

    # Determine base URL: flag > env > config > default (Token Plan Singapore)
    if [[ -z "$MIMO_BASE_URL" ]]; then
        MIMO_BASE_URL="${XIAOMI_MIMO_BASE_URL:-https://token-plan-sgp.xiaomimimo.com/v1}"
    fi

    # Detect key type and warn if mismatched
    if [[ "$MIMO_KEY" == tp-* && "$MIMO_BASE_URL" == *"api.xiaomimimo.com"* ]]; then
        echo "Warning: tp- key used with pay-as-you-go endpoint. Token Plan keys work with:" >&2
        echo "  https://token-plan-sgp.xiaomimimo.com/v1" >&2
    elif [[ "$MIMO_KEY" == sk-* && "$MIMO_BASE_URL" == *"token-plan"* ]]; then
        echo "Warning: sk- key used with Token Plan endpoint. Pay-as-you-go keys work with:" >&2
        echo "  https://api.xiaomimimo.com/v1" >&2
    fi

    echo "Direct MiMo mode:"
    echo "  Model:    $MODEL"
    echo "  Endpoint: $MIMO_BASE_URL"
    echo "  Key type: ${MIMO_KEY:0:3}..."
    echo ""

    # Export for PinchBench's lib_agent.py custom provider setup
    export OPENAI_API_KEY="$MIMO_KEY"
    export OPENAI_BASE_URL="$MIMO_BASE_URL"
fi

# ── Install PinchBench ──────────────────────────────────────────────────────
if [[ "$INSTALL_PINCHBENCH" == true || ! -d "$PINCHBENCH_DIR" ]]; then
    echo "Installing PinchBench to $PINCHBENCH_DIR ..."
    if [[ -d "$PINCHBENCH_DIR" ]]; then
        cd "$PINCHBENCH_DIR" && git pull
    else
        git clone https://github.com/pinchbench/skill.git "$PINCHBENCH_DIR"
    fi
    cd "$PINCHBENCH_DIR"
    uv venv .venv 2>/dev/null || true
    source .venv/bin/activate
    uv pip install -e .
fi

if [[ ! -d "$PINCHBENCH_DIR" ]]; then
    echo "Error: PinchBench not found at $PINCHBENCH_DIR" >&2
    echo "Run with --install to clone it automatically." >&2
    exit 1
fi

cd "$PINCHBENCH_DIR"

if [[ -f ".venv/bin/activate" ]]; then
    source .venv/bin/activate
fi

mkdir -p "$RESULTS_DIR"

# ── Record metadata ─────────────────────────────────────────────────────────
METADATA_FILE="$RESULTS_DIR/run_metadata.json"
cat > "$METADATA_FILE" <<META
{
    "codewhale_version": "$(codewhale --version 2>/dev/null || echo unknown)",
    "git_commit": "$(cd "$REPO_ROOT" && git rev-parse HEAD 2>/dev/null || echo unknown)",
    "pinchbench_commit": "$(git -C "$PINCHBENCH_DIR" rev-parse HEAD 2>/dev/null || echo unknown)",
    "model": "$MODEL",
    "routing": "$(if [[ "$OPENROUTER_MODE" == true ]]; then echo "openrouter"; else echo "direct-xiaomi"; fi)",
    "suite": "$SUITE",
    "runs": $RUNS,
    "timestamp_utc": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
    "platform": "$(uname -s)/$(uname -m)"
}
META
echo "Run metadata: $METADATA_FILE"

# ── Build and run PinchBench ────────────────────────────────────────────────
PB_ARGS=("--model" "$MODEL" "--suite" "$SUITE" "--runs" "$RUNS" "--output-dir" "$RESULTS_DIR")

if [[ -n "$JUDGE_MODEL" ]]; then
    PB_ARGS+=("--judge" "$JUDGE_MODEL")
fi

if [[ "$NO_UPLOAD" == true ]]; then
    PB_ARGS+=("--no-upload")
fi

# Pass direct-mimo endpoint info for lib_agent.py's custom provider setup
if [[ "$DIRECT_MIMO" == true ]]; then
    PB_ARGS+=("--base-url" "$MIMO_BASE_URL")
fi

PB_ARGS+=("${EXTRA_ARGS[@]}")

echo "Running PinchBench..."
echo "  Model:    $MODEL"
echo "  Suite:    $SUITE"
echo "  Runs:     $RUNS"
echo "  Output:   $RESULTS_DIR"
if [[ "$OPENROUTER_MODE" == true ]]; then
    echo "  Routing:  OpenRouter"
else
    echo "  Routing:  Direct Xiaomi API ($MIMO_BASE_URL)"
fi
echo ""

./scripts/run.sh "${PB_ARGS[@]}"

echo ""
echo "Results written to $RESULTS_DIR"
