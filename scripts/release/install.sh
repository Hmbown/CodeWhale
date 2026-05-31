#!/usr/bin/env bash
set -euo pipefail
# CodeWhale Unix installer
# Copies codewhale and codewhale-tui to ~/.local/bin (or $PREFIX/bin)

PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="${PREFIX}/bin"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

mkdir -p "$BIN_DIR"

echo "Installing codewhale to $BIN_DIR ..."

for bin in codewhale codewhale-tui; do
    src="$SCRIPT_DIR/$bin"
    dst="$BIN_DIR/$bin"
    if [[ ! -f "$src" ]]; then
        echo "ERROR: $src not found in archive"
        exit 1
    fi
    cp "$src" "$dst"
    chmod +x "$dst"
    echo "  $dst"
done

echo ""
echo "Done. Both binaries installed to $BIN_DIR."

# Check if BIN_DIR is on PATH
if [[ ":$PATH:" != *":$BIN_DIR:"* ]]; then
    echo ""
    echo "Add $BIN_DIR to your PATH:"
    echo ""
    SHELL_NAME="$(basename "${SHELL:-$SHELL}")"
    case "$SHELL_NAME" in
        zsh)  RC="$HOME/.zshrc" ;;
        bash) RC="$HOME/.bashrc" ;;
        fish) RC="$HOME/.config/fish/config.fish" ;;
        *)    RC="your shell profile" ;;
    esac
    echo "  echo 'export PATH=\"$BIN_DIR:\$PATH\"' >> $RC"
    echo "  source $RC"
fi

echo ""
echo "Then run: codewhale"
