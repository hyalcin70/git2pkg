#!/usr/bin/env bash
# Manual smoke corpus for git2pkg-ai.
# Runs a handful of small, neutral projects through the CLI for one target
# format. Network and a container engine are required.
#
# Usage: scripts/test-corpus.sh [auto|arch|debian|fedora|opensuse]
set -uo pipefail

TARGET="${1:-auto}"
BIN="./target/release/git2pkg-ai"

REPOS=(
  "https://github.com/jqlang/jq"              # Autotools / C
  "https://github.com/libcheck/check"         # CMake / C
  "https://github.com/sharkdp/hyperfine"      # Cargo / Rust
  "https://github.com/junegunn/fzf"           # Go
  "https://github.com/pallets/click"          # Python
  "https://github.com/karlstav/cava"          # CMake + system libs
)

if [[ ! -x "$BIN" ]]; then
  echo "Build first: cargo build --release" >&2
  exit 1
fi

failures=0
for repo in "${REPOS[@]}"; do
  echo "=== $repo ($TARGET)"
  if ! "$BIN" --cli "$repo" --format=package --target="$TARGET"; then
    echo "FAILED: $repo" >&2
    failures=$((failures + 1))
  fi
done

echo "Finished: $failures failure(s)"
exit "$failures"
