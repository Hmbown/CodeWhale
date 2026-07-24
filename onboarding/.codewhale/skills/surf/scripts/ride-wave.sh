#!/usr/bin/env bash
# ride-wave.sh — Update the testbed, build, verify, and generate a digest
# Reads .surf-config for repo URL and branch info

set -euo pipefail

# -- Load config if it exists --
if [ -f ".surf-config" ]; then
    source .surf-config
    echo "🌊 Surfing ${REPO_URL:-unknown} (branch: ${BRANCH:-main})"
else
    echo "⚠️  No .surf-config found. Using defaults."
    REPO_URL="${REPO_URL:-https://github.com/Hmbown/CodeWhale.git}"
    BRANCH="${BRANCH:-main}"
fi

# -- Check if this is a valid testbed --
if [ ! -f ".surf-config" ] || [ "${ONBOARDING_INIT:-false}" != "true" ]; then
    echo "❌ ERROR: Not a valid Surf testbed. Run /surf setup first."
    exit 1
fi

# -- Check if working directory is clean --
if [ -n "$(git status --porcelain)" ]; then
    echo "❌ ERROR: Working directory is dirty. Please commit or stash changes."
    exit 1
fi

# -- Ensure we're on the right branch --
CURRENT_BRANCH=$(git branch --show-current)
if [ "$CURRENT_BRANCH" != "$BRANCH" ]; then
    echo "⚠️  Currently on branch '$CURRENT_BRANCH', but config expects '$BRANCH'."
    echo "Switching to '$BRANCH'..."
    git checkout "$BRANCH"
fi

# -- Pull latest --
echo "📦 Pulling latest changes from $BRANCH..."
git pull --ff-only origin "$BRANCH"

# -- Show current state --
COMMIT=$(git rev-parse --short HEAD)
echo "📍 At commit: $COMMIT"

# -- Run verification --
echo ""
echo "🔧 Running verification..."
echo "----------------------------------------"

echo "▶️  cargo fmt --check"
cargo fmt --check

echo ""
echo "▶️  cargo clippy -- -D warnings"
cargo clippy -- -D warnings

echo ""
echo "▶️  cargo test --workspace"
cargo test --workspace

echo "----------------------------------------"
echo "✅ All checks passed."

# -- Generate digest from CHANGELOG --
echo ""
echo "📋 Digest (from CHANGELOG.md):"
echo "----------------------------------------"
if [ -f "CHANGELOG.md" ]; then
    # Extract the first version entry
    sed -n '/## \[/,/## \[/p' CHANGELOG.md | head -n -1 | tail -n +2 | head -n 10
else
    echo "⚠️  CHANGELOG.md not found."
fi
echo "----------------------------------------"

# -- Write receipt --
RECEIPTS_DIR="receipts"
mkdir -p "$RECEIPTS_DIR"
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

cat > "$RECEIPTS_DIR/latest_receipt.json" << EOF
{
  "timestamp": "$TIMESTAMP",
  "repo": "$REPO_URL",
  "branch": "$BRANCH",
  "commit": "$COMMIT",
  "status": "success",
  "message": "All checks passed."
}
EOF

echo "📄 Receipt written: $RECEIPTS_DIR/latest_receipt.json"
echo "🌊 Ride complete. 🏄"