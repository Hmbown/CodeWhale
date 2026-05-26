#!/usr/bin/env bash
set -euo pipefail

if [[ "${EUID}" -ne 0 ]]; then
  echo "Run as root: sudo bash scripts/tencent-lighthouse/install-services.sh" >&2
  exit 1
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DEEPSEEK_USER="${DEEPSEEK_USER:-codewhale}"
DEEPSEEK_ROOT="${DEEPSEEK_ROOT:-/opt/codewhale}"

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

install -m 0644 "${REPO_ROOT}/deploy/tencent-lighthouse/systemd/codewhale-runtime.service" /etc/systemd/system/codewhale-runtime.service
install -m 0644 "${REPO_ROOT}/deploy/tencent-lighthouse/systemd/codewhale-feishu-bridge.service" /etc/systemd/system/codewhale-feishu-bridge.service

systemctl daemon-reload
systemctl enable codewhale-runtime codewhale-feishu-bridge

cat <<'EOF'
Services installed but not started.

Before starting, verify:
  /etc/deepseek/runtime.env
  /etc/deepseek/feishu-bridge.env
  sudo -u codewhale node /opt/codewhale/bridge/scripts/validate-config.mjs --env /etc/deepseek/feishu-bridge.env --runtime-env /etc/deepseek/runtime.env --workspace-root /opt/whalebro --check-filesystem

Then run:
  sudo systemctl start codewhale-runtime
  sudo systemctl start codewhale-feishu-bridge
  sudo bash /opt/whalebro/codewhale/scripts/tencent-lighthouse/doctor.sh
  sudo journalctl -u codewhale-feishu-bridge -f
EOF
