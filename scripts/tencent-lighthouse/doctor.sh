#!/usr/bin/env bash
set -euo pipefail

CODEWHALE_USER="${CODEWHALE_USER:-${DEEPSEEK_USER:-codewhale}}"
CODEWHALE_ROOT="${CODEWHALE_ROOT:-${DEEPSEEK_ROOT:-/opt/codewhale}}"
WHALEBRO_ROOT="${WHALEBRO_ROOT:-/opt/whalebro}"
if [[ -z "${RUNTIME_ENV:-}" ]]; then
  if [[ -f /etc/codewhale/runtime.env || ! -f /etc/deepseek/runtime.env ]]; then
    RUNTIME_ENV="/etc/codewhale/runtime.env"
  else
    RUNTIME_ENV="/etc/deepseek/runtime.env"
  fi
fi
REPO_ROOT="${REPO_ROOT:-${WHALEBRO_ROOT}/codewhale}"
BRIDGE_KIND="${CODEWHALE_BRIDGE:-${DEEPSEEK_BRIDGE:-feishu}}"

case "${BRIDGE_KIND}" in
  feishu|lark)
    if [[ -z "${BRIDGE_ENV:-}" ]]; then
      if [[ -f /etc/codewhale/feishu-bridge.env || ! -f /etc/deepseek/feishu-bridge.env ]]; then
        BRIDGE_ENV="/etc/codewhale/feishu-bridge.env"
      else
        BRIDGE_ENV="/etc/deepseek/feishu-bridge.env"
      fi
    fi
    BRIDGE_DIR="${BRIDGE_DIR:-${CODEWHALE_ROOT}/bridge}"
    BRIDGE_UNIT="${BRIDGE_UNIT:-codewhale-feishu-bridge}"
    BRIDGE_PACKAGE="${BRIDGE_PACKAGE:-integrations/feishu-bridge}"
    ;;
  telegram)
    if [[ -z "${BRIDGE_ENV:-}" ]]; then
      if [[ -f /etc/codewhale/telegram-bridge.env || ! -f /etc/deepseek/telegram-bridge.env ]]; then
        BRIDGE_ENV="/etc/codewhale/telegram-bridge.env"
      else
        BRIDGE_ENV="/etc/deepseek/telegram-bridge.env"
      fi
    fi
    BRIDGE_DIR="${BRIDGE_DIR:-${CODEWHALE_ROOT}/telegram-bridge}"
    BRIDGE_UNIT="${BRIDGE_UNIT:-codewhale-telegram-bridge}"
    BRIDGE_PACKAGE="${BRIDGE_PACKAGE:-integrations/telegram-bridge}"
    ;;
  *)
    echo "Unknown bridge '${BRIDGE_KIND}'. Use CODEWHALE_BRIDGE=feishu or CODEWHALE_BRIDGE=telegram." >&2
    exit 1
    ;;
esac

failures=0
warnings=0

section() {
  printf '\n== %s ==\n' "$1"
}

pass() {
  printf '[ok] %s\n' "$1"
}

warn() {
  warnings=$((warnings + 1))
  printf '[warn] %s\n' "$1"
}

fail() {
  failures=$((failures + 1))
  printf '[fail] %s\n' "$1"
}

have_command() {
  command -v "$1" >/dev/null 2>&1
}

env_value() {
  local file="$1"
  local key="$2"
  [[ -f "${file}" ]] || return 0
  grep -E "^[[:space:]]*(export[[:space:]]+)?${key}=" "${file}" \
    | tail -n 1 \
    | sed -E "s/^[[:space:]]*(export[[:space:]]+)?${key}=//; s/^[[:space:]]+//; s/[[:space:]]+$//; s/^['\"]//; s/['\"]$//" \
    || true
}

env_value_any() {
  local file="$1"
  shift
  local value
  for key in "$@"; do
    value="$(env_value "${file}" "${key}")"
    if [[ -n "${value}" ]]; then
      printf '%s\n' "${value}"
      return 0
    fi
  done
  return 0
}

is_placeholder() {
  local value
  value="$(printf '%s' "${1:-}" | tr '[:upper:]' '[:lower:]')"
  [[ -z "${value}" || "${value}" == *replace-with* || "${value}" == *xxxxxxxx* || "${value}" == "changeme" ]]
}

file_mode() {
  if stat -c '%a' "$1" >/dev/null 2>&1; then
    stat -c '%a' "$1"
  else
    stat -f '%Lp' "$1"
  fi
}

check_commands() {
  section "Runtime tools"
  for cmd in git curl node npm systemctl ss; do
    if have_command "${cmd}"; then
      pass "${cmd} is installed"
    else
      warn "${cmd} is not on PATH"
    fi
  done
}

check_node() {
  section "Node"
  if ! have_command node; then
    fail "node is required for the phone bridge"
    return
  fi
  local major
  major="$(node -p "Number(process.versions.node.split('.')[0])" 2>/dev/null || echo 0)"
  if [[ "${major}" =~ ^[0-9]+$ ]] && (( major >= 18 )); then
    pass "Node.js major version is ${major}"
  else
    fail "Node.js 18+ is required; found ${major}"
  fi
}

check_workspace() {
  section "Workspace"
  [[ -d "${WHALEBRO_ROOT}" ]] && pass "${WHALEBRO_ROOT} exists" || fail "${WHALEBRO_ROOT} is missing"
  [[ -d "${REPO_ROOT}/.git" ]] && pass "${REPO_ROOT} is a git checkout" || fail "${REPO_ROOT} is not a git checkout"
  [[ -d "${WHALEBRO_ROOT}/worktrees" ]] && pass "${WHALEBRO_ROOT}/worktrees exists" || warn "${WHALEBRO_ROOT}/worktrees is missing"
  if [[ -f "${WHALEBRO_ROOT}/AGENTS.md" ]]; then
    pass "${WHALEBRO_ROOT}/AGENTS.md exists"
  else
    warn "${WHALEBRO_ROOT}/AGENTS.md is missing"
  fi
}

check_binaries() {
  section "CodeWhale binaries"
  local cargo_bin="/home/${CODEWHALE_USER}/.cargo/bin"
  local codewhale="${cargo_bin}/codewhale"
  local tui="${cargo_bin}/codewhale-tui"
  if [[ -x "${codewhale}" ]]; then
    pass "${codewhale} is executable"
    "${codewhale}" --version 2>/dev/null | sed 's/^/[info] codewhale version: /' || warn "codewhale --version failed"
  else
    fail "${codewhale} is missing or not executable"
  fi
  if [[ -x "${tui}" ]]; then
    pass "${tui} is executable"
    "${tui}" --version 2>/dev/null | sed 's/^/[info] codewhale-tui version: /' || warn "codewhale-tui --version failed"
  else
    fail "${tui} is missing or not executable"
  fi
}

check_env_file() {
  local file="$1"
  local label="$2"
  if [[ ! -f "${file}" ]]; then
    fail "${label} env file is missing: ${file}"
    return
  fi
  pass "${label} env file exists"
  local mode
  mode="$(file_mode "${file}")"
  local world="${mode: -1}"
  if [[ "${world}" =~ ^[0-9]+$ ]] && (( world > 0 )); then
    fail "${label} env file is world-readable (${mode})"
  else
    pass "${label} env file is not world-readable (${mode})"
  fi
}

check_env() {
  section "Environment"
  check_env_file "${RUNTIME_ENV}" "runtime"
  check_env_file "${BRIDGE_ENV}" "bridge"

  local runtime_token bridge_token workspace domain allow_groups allow_unlisted provider
  runtime_token="$(env_value_any "${RUNTIME_ENV}" CODEWHALE_RUNTIME_TOKEN DEEPSEEK_RUNTIME_TOKEN)"
  bridge_token="$(env_value_any "${BRIDGE_ENV}" CODEWHALE_RUNTIME_TOKEN DEEPSEEK_RUNTIME_TOKEN)"
  workspace="$(env_value_any "${BRIDGE_ENV}" CODEWHALE_WORKSPACE DEEPSEEK_WORKSPACE)"
  provider="$(env_value_any "${RUNTIME_ENV}" CODEWHALE_PROVIDER DEEPSEEK_PROVIDER)"

  if [[ "${BRIDGE_KIND}" == "telegram" ]]; then
    allow_groups="$(env_value "${BRIDGE_ENV}" TELEGRAM_ALLOW_GROUPS)"
    allow_unlisted="$(env_value_any "${BRIDGE_ENV}" TELEGRAM_ALLOW_UNLISTED CODEWHALE_ALLOW_UNLISTED DEEPSEEK_ALLOW_UNLISTED)"
  else
    domain="$(env_value "${BRIDGE_ENV}" FEISHU_DOMAIN)"
    allow_groups="$(env_value "${BRIDGE_ENV}" FEISHU_ALLOW_GROUPS)"
    allow_unlisted="$(env_value_any "${BRIDGE_ENV}" CODEWHALE_ALLOW_UNLISTED DEEPSEEK_ALLOW_UNLISTED)"
  fi

  if is_placeholder "${runtime_token}"; then
    fail "runtime token is missing or still a placeholder"
  else
    pass "runtime token is set"
  fi
  if is_placeholder "${bridge_token}"; then
    fail "bridge token is missing or still a placeholder"
  else
    pass "bridge token is set"
  fi
  if [[ -n "${runtime_token}" && -n "${bridge_token}" && "${runtime_token}" != "${bridge_token}" ]]; then
    fail "runtime and bridge tokens do not match"
  elif [[ -n "${runtime_token}" && -n "${bridge_token}" ]]; then
    pass "runtime and bridge tokens match"
  fi
  if is_placeholder "${provider}"; then
    warn "runtime provider is missing or still a placeholder"
  else
    pass "runtime provider is ${provider}"
  fi
  [[ "${workspace}" == "${WHALEBRO_ROOT}" || "${workspace}" == "${WHALEBRO_ROOT}/"* ]] \
    && pass "bridge workspace is under ${WHALEBRO_ROOT}" \
    || warn "bridge workspace is outside ${WHALEBRO_ROOT}: ${workspace:-unset}"
  if [[ "${BRIDGE_KIND}" != "telegram" ]]; then
    [[ "${domain:-feishu}" == "feishu" || "${domain:-feishu}" == "lark" || "${domain:-feishu}" == https://open.* ]] \
      && pass "FEISHU_DOMAIN is ${domain:-feishu}" \
      || fail "FEISHU_DOMAIN must be feishu, lark, or an https://open.* URL"
  fi
  [[ "${allow_groups:-false}" == "true" && "${allow_unlisted:-false}" == "true" ]] \
    && fail "group control cannot run with allow-unlisted=true" \
    || pass "group/unlisted mode is not openly combined"
}

check_validator() {
  section "Bridge config validator"
  local validator="${BRIDGE_DIR}/scripts/validate-config.mjs"
  if [[ ! -f "${validator}" ]]; then
    validator="${REPO_ROOT}/${BRIDGE_PACKAGE}/scripts/validate-config.mjs"
  fi
  if [[ ! -f "${validator}" ]]; then
    warn "bridge config validator is not installed"
    return
  fi
  local runner=(node)
  if [[ "${EUID}" -eq 0 ]] && id -u "${CODEWHALE_USER}" >/dev/null 2>&1 && have_command sudo; then
    runner=(sudo -u "${CODEWHALE_USER}" node)
  fi
  if "${runner[@]}" "${validator}" --env "${BRIDGE_ENV}" --runtime-env "${RUNTIME_ENV}" --workspace-root "${WHALEBRO_ROOT}" --check-filesystem; then
    pass "bridge config validator passed"
  else
    fail "bridge config validator reported blocking issues"
  fi
}

check_systemd() {
  section "systemd"
  if ! have_command systemctl || [[ ! -d /run/systemd/system ]]; then
    warn "systemd is not available in this environment"
    return
  fi
  for unit in codewhale-runtime "${BRIDGE_UNIT}"; do
    [[ -f "/etc/systemd/system/${unit}.service" ]] \
      && pass "${unit}.service is installed" \
      || fail "${unit}.service is missing"
    systemctl is-enabled --quiet "${unit}" \
      && pass "${unit} is enabled" \
      || warn "${unit} is not enabled"
    systemctl is-active --quiet "${unit}" \
      && pass "${unit} is active" \
      || fail "${unit} is not active"
  done
}

check_bridge_install() {
  section "Bridge install"
  [[ -f "${BRIDGE_DIR}/package.json" ]] && pass "${BRIDGE_DIR}/package.json exists" || fail "bridge package.json is missing"
  [[ -f "${BRIDGE_DIR}/src/index.mjs" ]] && pass "${BRIDGE_DIR}/src/index.mjs exists" || fail "bridge entrypoint is missing"
  if [[ "${BRIDGE_KIND}" == "telegram" ]]; then
    pass "Telegram bridge has no required production npm dependencies"
  elif [[ -d "${BRIDGE_DIR}/node_modules/@larksuiteoapi/node-sdk" ]]; then
    pass "Lark SDK dependency is installed"
  else
    warn "Lark SDK dependency is not installed under ${BRIDGE_DIR}/node_modules"
  fi
}

check_localhost_health() {
  section "Localhost health"
  local port token
  port="$(env_value_any "${RUNTIME_ENV}" CODEWHALE_RUNTIME_PORT DEEPSEEK_RUNTIME_PORT)"
  port="${port:-7878}"
  token="$(env_value_any "${BRIDGE_ENV}" CODEWHALE_RUNTIME_TOKEN DEEPSEEK_RUNTIME_TOKEN)"

  if have_command ss; then
    local listeners
    listeners="$(ss -ltn 2>/dev/null | awk -v port=":${port}" '$4 ~ port {print $4}' || true)"
    if grep -qE "^127\\.0\\.0\\.1:${port}$|^\\[::1\\]:${port}$" <<<"${listeners}"; then
      pass "runtime port ${port} is bound to localhost"
    elif [[ -n "${listeners}" ]]; then
      fail "runtime port ${port} is listening on a non-local address: ${listeners//$'\n'/, }"
    else
      fail "runtime port ${port} is not listening"
    fi
  else
    warn "ss is unavailable; skipping bind-address check"
  fi

  if ! have_command curl; then
    warn "curl is unavailable; skipping HTTP checks"
    return
  fi

  if curl -fsS --max-time 3 "http://127.0.0.1:${port}/health" >/dev/null; then
    pass "/health responds on localhost"
  else
    fail "/health did not respond on localhost:${port}"
  fi

  if is_placeholder "${token}"; then
    warn "runtime token is not usable; skipping /v1/runtime/info auth check"
    return
  fi

  local tmp
  tmp="$(mktemp)"
  if curl -fsS --max-time 3 -H "Authorization: Bearer ${token}" \
    "http://127.0.0.1:${port}/v1/runtime/info" >"${tmp}"; then
    if node -e '
      const fs = require("fs");
      const data = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
      if (data.bind_host !== "127.0.0.1") process.exit(2);
      if (data.auth_required !== true) process.exit(3);
    ' "${tmp}"; then
      pass "/v1/runtime/info reports localhost bind and auth_required=true"
    else
      fail "/v1/runtime/info did not report localhost bind with auth enabled"
    fi
  else
    fail "/v1/runtime/info did not respond with bearer auth"
  fi
  rm -f "${tmp}"
}

main() {
  printf 'Tencent Lighthouse CodeWhale doctor (%s bridge)\n' "${BRIDGE_KIND}"
  check_commands
  check_node
  check_workspace
  check_binaries
  check_env
  check_bridge_install
  check_validator
  check_systemd
  check_localhost_health

  section "Summary"
  printf '%s failure(s), %s warning(s)\n' "${failures}" "${warnings}"
  (( failures == 0 ))
}

main "$@"
