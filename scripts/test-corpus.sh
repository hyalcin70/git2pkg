#!/usr/bin/env bash
# Manual smoke corpus for git2pkg-ai.
#
# Pass one or more repository URLs to build them one after another. The
# script contains no project names on purpose: the corpus is yours to choose
# (for example one small project per build system you care about). Network
# and a container engine are required.
#
# Usage: scripts/test-corpus.sh <target> <repo-url> [<repo-url> ...]
#        scripts/test-corpus.sh auto https://example.invalid/project.git
#        GIT2PKG_TEST_REPOS="url1 url2" scripts/test-corpus.sh debian
#
# <target> is one of: auto, arch, debian, fedora, opensuse.
set -uo pipefail

TARGET="${1:-auto}"
shift || true
BIN="./target/release/git2pkg-ai"

if [[ $# -gt 0 ]]; then
  REPOS=("$@")
elif [[ -n "${GIT2PKG_TEST_REPOS:-}" ]]; then
  read -r -a REPOS <<<"$GIT2PKG_TEST_REPOS"
else
  echo "Usage: $0 <target> <repo-url> [<repo-url> ...]" >&2
  echo "   or: GIT2PKG_TEST_REPOS=\"url1 url2\" $0 <target>" >&2
  exit 2
fi

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
