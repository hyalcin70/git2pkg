#!/usr/bin/env bash
# Baut die optionalen Builder-Images fuer git2pkg.
#
#   ./build.sh            # alle Images
#   ./build.sh arch        # nur eines
#
# git2pkg erkennt vorhandene Images automatisch (`git2pkg-builder:<familie>`)
# und laesst dann die Paketinstallation im Build entfallen.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
engine=${GIT2PKG_ENGINE:-docker}

targets=("$@")
if [ ${#targets[@]} -eq 0 ]; then
  mapfile -t targets < <(find "$here" -maxdepth 1 -name '*.Dockerfile' -printf '%f\n' \
    | sed 's/\.Dockerfile$//' | sort)
fi

for name in "${targets[@]}"; do
  dockerfile="$here/$name.Dockerfile"
  if [ ! -f "$dockerfile" ]; then
    echo "unbekanntes Builder-Image: $name" >&2
    exit 1
  fi
  echo "==> git2pkg-builder:$name"
  "$engine" build -t "git2pkg-builder:$name" -f "$dockerfile" "$here"
done
