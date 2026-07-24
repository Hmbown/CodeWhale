#!/usr/bin/env bash
set -euo pipefail

if [ ! -d ".git" ]; then
    echo "STATUS=empty-or-no-git"
    echo "MESSAGE=No Git repository found."
    exit 0
fi

if [ -f ".surf-config" ]; then
    # Load config
    source .surf-config
    if [ "${ONBOARDING_INIT:-false}" = "true" ]; then
        echo "STATUS=testbed"
        echo "MESSAGE=Testbed detected (${REPO_URL:-unknown})"
    else
        echo "STATUS=unknown-repo"
        echo "MESSAGE=.surf-config found but ONBOARDING_INIT=false"
    fi
else
    echo "STATUS=unknown-repo"
    echo "MESSAGE=Git repository without .surf-config marker."
fi

if [ -n "$(git status --porcelain)" ]; then
    echo "DIRTY=true"
else
    echo "DIRTY=false"
fi