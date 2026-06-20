#!/usr/bin/env bash
# DeepSeek GUI 一键 release 构建（macOS / Linux）
# 用法：chmod +x scripts/build-release.sh && ./scripts/build-release.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
GUI="$(cd "$(dirname "$0")/.." && pwd)"
TUI_OUT="${ROOT}/target/release/deepseek-tui"
SIDECAR_DIR="${GUI}/src-tauri/bin"
RELEASE_DIR="${GUI}/src-tauri/target/release"

echo "==> 构建 deepseek-tui (release)..."
(cd "${ROOT}" && cargo build --release -p deepseek-tui)

echo "==> 构建前端 dist..."
(cd "${GUI}" && npm run build)

echo "==> 复制 sidecar 到 src-tauri/bin..."
mkdir -p "${SIDECAR_DIR}"
cp -f "${TUI_OUT}" "${SIDECAR_DIR}/deepseek-tui"
chmod +x "${SIDECAR_DIR}/deepseek-tui"
mkdir -p "${RELEASE_DIR}"
cp -f "${TUI_OUT}" "${RELEASE_DIR}/deepseek-tui"
chmod +x "${RELEASE_DIR}/deepseek-tui"

echo "==> Tauri 打包..."
(cd "${GUI}" && npm run tauri:build)

echo ""
echo "构建完成。产物目录："
echo "  ${RELEASE_DIR}/bundle/"
