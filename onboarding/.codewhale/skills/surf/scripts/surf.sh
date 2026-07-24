#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RECEIPTS_DIR="receipts"
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

mkdir -p "$RECEIPTS_DIR"

echo "🌊 Checking the wave..."
STATE=$("$SCRIPT_DIR/check-wave.sh")
echo "$STATE"

STATUS=$(echo "$STATE" | grep "^STATUS=" | cut -d= -f2)
MESSAGE=$(echo "$STATE" | grep "^MESSAGE=" | cut -d= -f2-)
DIRTY=$(echo "$STATE" | grep "^DIRTY=" | cut -d= -f2 || echo "false")

case "$STATUS" in
    empty-or-no-git)
        echo "🌊 The water is calm. No wave detected."
        echo "📋 Run /surf setup to catch a wave."
        exit 0
        ;;
    testbed)
        if [ "$DIRTY" = "true" ]; then
            echo "⚠️  The wave is choppy. Uncommitted changes detected."
            echo "📋 Clean up or stash changes before riding."
            exit 1
        else
            echo "🌊 Wave is clean. Riding..."
            "$SCRIPT_DIR/ride-wave.sh" | tee "$RECEIPTS_DIR/latest_ride.log"
            COMMIT=$(git rev-parse --short HEAD)
            BRANCH=$(git branch --show-current)
            cat > "$RECEIPTS_DIR/latest_receipt.json" << EOF
{
  "timestamp": "$TIMESTAMP",
  "branch": "$BRANCH",
  "commit": "$COMMIT",
  "status": "success",
  "digest": "Ride complete. See latest_ride.log for details."
}
EOF
            echo "📄 Receipt written: $RECEIPTS_DIR/latest_receipt.json"
            echo "✅ Surf complete."
            exit 0
        fi
        ;;
    unknown-repo)
        echo "⚠️  Unknown territory: $MESSAGE"
        echo "📋 Not a recognized surf spot. Run /surf setup in an empty directory."
        exit 1
        ;;
    *)
        echo "❌ Unknown state: $STATUS"
        exit 1
        ;;
esac