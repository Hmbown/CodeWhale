#!/usr/bin/env bash
# Source into an interactive agent shell (tmux, ssh) to export the provider
# key and set defaults that systemd normally handles via EnvironmentFile=.
#
# Usage (as the codewhale user):
#   . /opt/whalebro/codewhale/scripts/remote-smoke/agent-session.sh
#   codewhale models           # should list deepseek-v4-pro
#   gh auth status             # should show the fine-grained PAT
#
# The runtime.env file is 0640 root:codewhale, readable by the codewhale user.
set -a
# shellcheck disable=SC1091
. /etc/codewhale/runtime.env
set +a
export CODEWHALE_MODEL="${CODEWHALE_MODEL:-deepseek-v4-pro}"
