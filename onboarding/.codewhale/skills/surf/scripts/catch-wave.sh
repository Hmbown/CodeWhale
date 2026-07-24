#!/usr/bin/env bash
set -euo pipefail

if [ -n "$(ls -A)" ]; then
    echo "ERROR: Directory not empty. Refusing to install."
    exit 1
fi

# Ask for repo URL (with default)
read -p "Enter repository URL (default: https://github.com/Hmbown/CodeWhale.git): " REPO_URL
REPO_URL=${REPO_URL:-https://github.com/Hmbown/CodeWhale.git}

read -p "Enter branch (default: main): " BRANCH
BRANCH=${BRANCH:-main}

echo "Cloning $REPO_URL (branch: $BRANCH)..."
git clone --branch "$BRANCH" "$REPO_URL" .

# Create config file
cat > .surf-config << EOF
# Surf configuration
REPO_URL=$REPO_URL
BRANCH=$BRANCH
ONBOARDING_INIT=true
EOF

echo "Testbed initialized. Config written to .surf-config."