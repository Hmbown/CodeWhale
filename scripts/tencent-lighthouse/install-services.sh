#!/usr/bin/env bash
set -euo pipefail

if [[ "${EUID}" -ne 0 ]]; then
  echo "Run as root: sudo bash scripts/tencent-lighthouse/install-services.sh" >&2
  exit 1
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CODEWHALE_USER="${CODEWHALE_USER:-${DEEPSEEK_USER:-codewhale}}"
CODEWHALE_ROOT="${CODEWHALE_ROOT:-${DEEPSEEK_ROOT:-/opt/codewhale}}"
BRIDGE_KIND="${CODEWHALE_BRIDGE:-${DEEPSEEK_BRIDGE:-feishu}}"

case "${BRIDGE_KIND}" in
  feishu|lark)
    BRIDGE_SRC="integrations/feishu-bridge"
    BRIDGE_DST="${CODEWHALE_ROOT}/bridge"
    BRIDGE_UNIT="codewhale-feishu-bridge.service"
    BRIDGE_ENV="/etc/codewhale/feishu-bridge.env"
    BRIDGE_ENV_EXAMPLE="deploy/tencent-lighthouse/examples/feishu-bridge.env.example"
    BRIDGE_STATE_DIR="/var/lib/codewhale-feishu-bridge"
    VALIDATOR="integrations/feishu-bridge/scripts/validate-config.mjs"
    ;;
  telegram)
    BRIDGE_SRC="integrations/telegram-bridge"
    BRIDGE_DST="${CODEWHALE_ROOT}/telegram-bridge"
    BRIDGE_UNIT="codewhale-telegram-bridge.service"
    BRIDGE_ENV="/etc/codewhale/telegram-bridge.env"
    BRIDGE_ENV_EXAMPLE="deploy/tencent-lighthouse/examples/telegram-bridge.env.example"
    BRIDGE_STATE_DIR="/var/lib/codewhale-telegram-bridge"
    VALIDATOR="integrations/telegram-bridge/scripts/validate-config.mjs"
    ;;
  *)
    echo "Unknown bridge '${BRIDGE_KIND}'. Use CODEWHALE_BRIDGE=feishu or CODEWHALE_BRIDGE=telegram." >&2
    exit 1
    ;;
esac

install -d -m 0750 -o root -g "${CODEWHALE_USER}" /etc/codewhale
install -d -m 0700 -o "${CODEWHALE_USER}" -g "${CODEWHALE_USER}" "${BRIDGE_STATE_DIR}"
install -d -o "${CODEWHALE_USER}" -g "${CODEWHALE_USER}" "${BRIDGE_DST}"

if [[ ! -f /etc/codewhale/runtime.env && -f "${REPO_ROOT}/deploy/tencent-lighthouse/examples/runtime.env.example" ]]; then
  install -m 0640 -o root -g "${CODEWHALE_USER}" \
    "${REPO_ROOT}/deploy/tencent-lighthouse/examples/runtime.env.example" \
    /etc/codewhale/runtime.env
fi

if [[ ! -f "${BRIDGE_ENV}" && -f "${REPO_ROOT}/${BRIDGE_ENV_EXAMPLE}" ]]; then
  install -m 0640 -o root -g "${CODEWHALE_USER}" \
    "${REPO_ROOT}/${BRIDGE_ENV_EXAMPLE}" \
    "${BRIDGE_ENV}"
fi
rsync -a --delete \
  --exclude node_modules \
  "${REPO_ROOT}/${BRIDGE_SRC}/" \
  "${BRIDGE_DST}/"
chown -R "${CODEWHALE_USER}:${CODEWHALE_USER}" "${BRIDGE_DST}"

if [[ -f "${BRIDGE_DST}/package-lock.json" ]]; then
  sudo -u "${CODEWHALE_USER}" npm --prefix "${BRIDGE_DST}" ci --omit=dev
else
  sudo -u "${CODEWHALE_USER}" npm --prefix "${BRIDGE_DST}" install --omit=dev
fi

install -m 0644 "${REPO_ROOT}/deploy/tencent-lighthouse/systemd/codewhale-runtime.service" /etc/systemd/system/codewhale-runtime.service
install -m 0644 "${REPO_ROOT}/deploy/tencent-lighthouse/systemd/${BRIDGE_UNIT}" "/etc/systemd/system/${BRIDGE_UNIT}"

systemctl daemon-reload
systemctl enable codewhale-runtime "${BRIDGE_UNIT}"

cat <<'EOF'
Services installed but not started.

Before starting, verify:
  /etc/codewhale/runtime.env
EOF
cat <<EOF
  ${BRIDGE_ENV}
  sudo -u ${CODEWHALE_USER} node ${REPO_ROOT}/${VALIDATOR} --env ${BRIDGE_ENV} --runtime-env /etc/codewhale/runtime.env --workspace-root /opt/whalebro --check-filesystem
Then run:
  sudo systemctl start codewhale-runtime
  sudo systemctl start ${BRIDGE_UNIT}
  sudo CODEWHALE_BRIDGE=${BRIDGE_KIND} bash /opt/whalebro/codewhale/scripts/tencent-lighthouse/doctor.sh
  sudo journalctl -u ${BRIDGE_UNIT} -f
EOF
