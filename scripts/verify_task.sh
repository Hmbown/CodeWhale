#!/bin/bash
# verify_task.sh <task_id> <docker_image>
# Runs the DeepSWE verifier inside the task's Docker container.
# Expects model.patch at /tmp/deep-swe-verify/<task_id>/model.patch
TASK_ID="$1"
IMAGE="$2"
TASKS_DIR="/Volumes/VIXinSSD/whalebro/codewhale/deep-swe/tasks"
WORK_DIR="/tmp/deep-swe-verify/$TASK_ID"

mkdir -p "$WORK_DIR"
RESULT_FILE="$WORK_DIR/result.txt"

echo "[$TASK_ID] Pulling image..."
docker pull "$IMAGE" 2>&1 | tail -1

echo "[$TASK_ID] Running verifier..."
docker run --rm \
  --platform linux/amd64 \
  -v "$WORK_DIR/model.patch:/model.patch:ro" \
  -v "$TASKS_DIR/$TASK_ID/tests/test.patch:/tests/test.patch:ro" \
  -v "$TASKS_DIR/$TASK_ID/tests/test.sh:/verify.sh:ro" \
  "$IMAGE" \
  bash -c '
    set -e
    mkdir -p /logs/verifier /logs/artifacts
    cd /app
    git apply --whitespace=nowarn /model.patch 2>/dev/null || { echo "PATCH_FAILED"; exit 2; }
    bash /verify.sh > /logs/verifier/output.txt 2>&1
    EC=$?
    if [ -f /logs/verifier/reward.txt ]; then
      REWARD=$(cat /logs/verifier/reward.txt)
      echo "REWARD=$REWARD"
    else
      # Extract from output
      if grep -q "New tests exit code: 0" /logs/verifier/output.txt && \
         grep -q "Baseline exit code: 0" /logs/verifier/output.txt; then
        echo "REWARD=1"
      else
        echo "REWARD=0"
      fi
    fi
    echo "---OUTPUT_TAIL---"
    tail -30 /logs/verifier/output.txt
  ' > "$RESULT_FILE" 2>&1

echo "[$TASK_ID] Done. Result:"
cat "$RESULT_FILE" | grep -E 'REWARD|FAILED|PATCH_FAILED|passed'
echo ""
