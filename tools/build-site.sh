#!/usr/bin/env bash
# Stage the GitHub Pages site into OUT: site/ plus the shared icons from assets/.
set -euo pipefail

if [ $# -ne 1 ]; then
  echo "usage: $0 OUT" >&2
  exit 2
fi

root="$(cd "$(dirname "$0")/.." && pwd)"
out="$1"

rm -rf "$out"
mkdir -p "$out"
cp -r "$root/site/." "$out/"
cp "$root/assets/icon.svg" "$root/assets/icon-32.png" "$root/assets/icon-256.png" "$out/"
