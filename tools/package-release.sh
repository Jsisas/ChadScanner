#!/usr/bin/env bash
# Packages a deploy-wsl.sh output folder into GitHub Release assets.
#
# Usage: tools/package-release.sh PUB TAG REPO DIST [PREV_MANIFEST]
#   PUB            folder filled by `tools/deploy-wsl.sh --out PUB`
#   TAG            release tag, e.g. v0.2.0
#   REPO           GitHub repo, e.g. Jsisas/ChadScanner
#   DIST           output folder: app zip, data zip (if changed), manifest.json, notes.md
#   PREV_MANIFEST  previous release's manifest.json; its data entry is reused when the data key matches
# Env: CHADVISION_SHA (optional) is recorded in notes.md.
set -euo pipefail

die() { echo "error: $*" >&2; exit 1; }

case "${1:-}" in -h|--help) sed -n '2,10p' "$0"; exit 0 ;; esac
[[ $# -ge 4 && $# -le 5 ]] || { sed -n '2,10p' "$0" >&2; exit 2; }
PUB="$1" TAG="$2" REPO="$3" DIST="$4" PREV="${5:-}"

for t in zip jq sha256sum; do command -v "$t" >/dev/null || die "$t not found"; done
[[ -d "$PUB/data" && -f "$PUB/chadscanner.exe" && -f "$PUB/onnxruntime.dll" ]] || die "$PUB is not a deploy-wsl.sh output folder"

VERSION=${TAG#v}
mkdir -p "$DIST"
DIST="$(cd "$DIST" && pwd)"
BASE="https://github.com/$REPO/releases/download/$TAG"

# The key covers only the catalog and the models: rebuilt index floats can differ bit-wise
# between CPUs and would change the hash for no reason.
KEY=$(cd "$PUB" && find data/refs/catalog.json data/models -type f | LC_ALL=C sort | xargs sha256sum | sha256sum | cut -c1-12)
printf '%s\n' "$KEY" > "$PUB/data/key.txt"

APP_ZIP="chadscanner-win-x64.zip"
rm -f "$DIST/$APP_ZIP"
(cd "$PUB" && zip -q -X -9 "$DIST/$APP_ZIP" chadscanner.exe onnxruntime.dll)
APP_JSON=$(jq -n --arg url "$BASE/$APP_ZIP" --arg sha "$(sha256sum "$DIST/$APP_ZIP" | cut -d' ' -f1)" \
	--argjson size "$(stat -c %s "$DIST/$APP_ZIP")" '{url:$url, sha256:$sha, size:$size}')

if [[ -n "$PREV" && -f "$PREV" && "$(jq -r '.data.key' "$PREV")" == "$KEY" ]]; then
	echo "data unchanged (key $KEY); reusing the previous release's data zip"
	DATA_JSON=$(jq -c .data "$PREV")
else
	DATA_ZIP="chadscanner-data-$KEY.zip"
	rm -f "$DIST/$DATA_ZIP"
	(cd "$PUB" && zip -q -X -r "$DIST/$DATA_ZIP" data)
	DATA_JSON=$(jq -n -c --arg key "$KEY" --arg url "$BASE/$DATA_ZIP" --arg sha "$(sha256sum "$DIST/$DATA_ZIP" | cut -d' ' -f1)" \
		--argjson size "$(stat -c %s "$DIST/$DATA_ZIP")" '{key:$key, url:$url, sha256:$sha, size:$size}')
fi

jq -n --arg version "$VERSION" --argjson app "$APP_JSON" --argjson data "$DATA_JSON" \
	'{version:$version, app:$app, data:$data}' > "$DIST/manifest.json"

{
	cat <<EOF
## ⬇ [Download ChadScanner for Windows](https://github.com/$REPO/releases/download/$TAG/$APP_ZIP)

1. Right-click the downloaded **$APP_ZIP** → **Extract All…** → choose a folder you own, e.g. \`C:\\ChadScanner\`.
2. Open that folder and double-click **chadscanner.exe**. If Windows SmartScreen warns: **More info → Run anyway**.
3. The first start downloads the item data (~180 MB) by itself.

Already installed? Right-click the tray icon → **Update**. Nothing to download here.

The other files below are used by the app's updater; you do not need them.
EOF
	if [[ -n "${CHADVISION_SHA:-}" ]]; then printf '\nChadVision: %s\n' "$CHADVISION_SHA"; fi
} > "$DIST/notes.md"

echo "Release assets in $DIST:"
ls -l "$DIST"
