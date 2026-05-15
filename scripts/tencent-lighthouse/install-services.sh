#!/usr/bin/env bash
set -euo pipefail

if [[ "${EUID}" -ne 0 ]]; then
  echo "Run as root: sudo bash scripts/tencent-lighthouse/install-services.sh" >&2
  exit 1
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DEEPSEEK_USER="${DEEPSEEK_USER:-deepseek}"
DEEPSEEK_ROOT="${DEEPSEEK_ROOT:-/opt/deepseek}"

install -d -o "${DEEPSEEK_USER}" -g "${DEEPSEEK_USER}" "${DEEPSEEK_ROOT}/bridge"
rsync -a --delete \
  --exclude node_modules \
  "${REPO_ROOT}/integrations/feishu-bridge/" \
  "${DEEPSEEK_ROOT}/bridge/"
chown -R "${DEEPSEEK_USER}:${DEEPSEEK_USER}" "${DEEPSEEK_ROOT}/bridge"

if [[ -f "${DEEPSEEK_ROOT}/bridge/package-lock.json" ]]; then
  sudo -u "${DEEPSEEK_USER}" npm --prefix "${DEEPSEEK_ROOT}/bridge" ci --omit=dev
else
  sudo -u "${DEEPSEEK_USER}" npm --prefix "${DEEPSEEK_ROOT}/bridge" install --omit=dev
fi

install -m 0644 "${REPO_ROOT}/deploy/tencent-lighthouse/systemd/deepseek-runtime.service" /etc/systemd/system/deepseek-runtime.service
install -m 0644 "${REPO_ROOT}/deploy/tencent-lighthouse/systemd/deepseek-feishu-bridge.service" /etc/systemd/system/deepseek-feishu-bridge.service

systemctl daemon-reload
systemctl enable deepseek-runtime deepseek-feishu-bridge

cat <<'EOF'
Services installed but not started.

Before starting, verify:
  /etc/deepseek/runtime.env
  /etc/deepseek/feishu-bridge.env
  sudo -u deepseek node /opt/deepseek/bridge/scripts/validate-config.mjs --env /etc/deepseek/feishu-bridge.env --runtime-env /etc/deepseek/runtime.env --workspace-root /opt/whalebro --check-filesystem

Then run:
  sudo systemctl start deepseek-runtime
  sudo systemctl start deepseek-feishu-bridge
  sudo bash /opt/whalebro/deepseek-tui/scripts/tencent-lighthouse/doctor.sh
  sudo journalctl -u deepseek-feishu-bridge -f
EOF
