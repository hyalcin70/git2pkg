#!/usr/bin/env bash
# git2pkg-ai - clean uninstallation.
# Reads the installation path from the start-menu entry, if present.
set -euo pipefail

BOLD=$'\033[1m'
RESET=$'\033[0m'
DESKTOP_DIR="$HOME/.local/share/applications"
DESKTOP_FILE="$DESKTOP_DIR/git2pkg-ai.desktop"
ICON_FILE="$HOME/.local/share/icons/hicolor/256x256/apps/git2pkg-ai.png"

say() { printf '%s\n' "$*"; }
bold() { printf '%s%s%s\n' "$BOLD" "$1" "$RESET"; }
is_yes() {
  case "${1,,}" in
    j|ja|y|yes) return 0 ;;
    *) return 1 ;;
  esac
}

BINARY=""
if [[ -f "$DESKTOP_FILE" ]]; then
  exec_line=$(grep -m1 '^Exec=' "$DESKTOP_FILE" || true)
  if [[ -n "$exec_line" ]]; then
    BINARY=${exec_line#Exec=}
    BINARY=${BINARY#\"}
    BINARY=${BINARY%\"}
    BINARY=$(printf '%s' "$BINARY" | sed -E 's/[[:space:]]+%[A-Za-z].*$//; s/[[:space:]]+$//')
  fi
fi

if [[ -z "$BINARY" ]]; then
  say "No start-menu entry with a path was found."
  printf "Please enter the path of the binary to remove (default ./git2pkg-ai): "
  read -r BINARY
  BINARY=${BINARY:-./git2pkg-ai}
  case "$BINARY" in
    "~/"*) BINARY="$HOME/${BINARY#\~/}" ;;
  esac
  if [[ "$BINARY" != /* ]]; then
    BINARY="$PWD/$BINARY"
  fi
fi

say ""
say "Found:"
say "  Binary:       $BINARY"
if [[ -f "$DESKTOP_FILE" ]]; then
  say "  Menu entry:   $DESKTOP_FILE"
else
  say "  Menu entry:   (none found)"
fi
say ""
printf "Remove these files? [y/N] "
read -r confirm
if ! is_yes "$confirm"; then
  say "Aborted - nothing was removed."
  exit 0
fi

if [[ -f "$BINARY" ]]; then
  rm -f -- "$BINARY"
  say "Removed: $BINARY"
else
  say "Binary was already gone: $BINARY"
fi

if [[ -f "$DESKTOP_FILE" ]]; then
  rm -f -- "$DESKTOP_FILE"
  say "Removed: $DESKTOP_FILE"
  if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$DESKTOP_DIR" >/dev/null 2>&1 || true
  fi
fi

if [[ -f "$ICON_FILE" ]]; then
  rm -f -- "$ICON_FILE"
  say "Removed: $ICON_FILE"
fi

say ""
bold "Uninstallation complete."
