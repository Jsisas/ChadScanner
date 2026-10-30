#!/usr/bin/env bash
# Builds ChadOverlay for Windows from WSL2/Linux and publishes it with ChadVision's data.
# Rerun-safe: overwrites the exe, the ONNX Runtime DLL and data/ in the output folder.
#
# Usage: tools/deploy-wsl.sh [--out DIR] [--chadvision DIR]
#   --out         publish folder (default /mnt/c/ChadOverlay, i.e. C:\ChadOverlay)
#   --chadvision  ChadVision repo (default ../ChadVision next to this repo)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CV="$ROOT/../ChadVision"
OUT="/mnt/c/ChadOverlay"
TARGET=x86_64-pc-windows-msvc
# Windows build loads ONNX Runtime dynamically; the version must match ort-sys (1.22.x).
ORT_VER=1.22.0

while [[ $# -gt 0 ]]; do
	case "$1" in
		--out) OUT="$2"; shift 2 ;;
		--chadvision) CV="$2"; shift 2 ;;
		-h|--help) sed -n '2,7p' "$0"; exit 0 ;;
		*) echo "unknown argument: $1" >&2; exit 2 ;;
	esac
done

step() { printf '\n==> %s\n' "$*"; }
die() { echo "error: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null || die "$1 not found. $2"; }

step "Checking tools"
need cargo "Install Rust: https://rustup.rs"
need rustup "Install Rust via rustup (https://rustup.rs); distro cargo cannot add the Windows target"
need lld-link "sudo apt install lld"
need curl "sudo apt install curl"
need unzip "sudo apt install unzip"
rustup target list --installed | grep -qx "$TARGET" || rustup target add "$TARGET"
command -v cargo-xwin >/dev/null || cargo install --locked cargo-xwin
[[ -f "$CV/Cargo.toml" ]] || die "ChadVision not found at $CV (pass --chadvision)"
CV="$(cd "$CV" && pwd)"

step "Preparing ChadVision data"
cd "$CV"
if [[ ! -f data/refs/catalog.json ]]; then
	cargo run --release -- fetch-refs --out data/refs
	rm -rf data/index
fi
[[ -f data/models/dinov2-small.onnx && -f data/models/ppocrv5_latin_rec.onnx && -f data/models/ppocrv5_latin_dict.txt ]] \
	|| cargo run --release -- fetch-models
# The bench run builds data/index as a side effect; it is reused when the catalog is unchanged.
[[ -f data/index/embed_ids.txt ]] || cargo run --release -- bench --set data/real --methods fusion --out /tmp/tv-index-bench.md
ORT_DLL="$CV/data/ort-win-x64-$ORT_VER/onnxruntime.dll"
if [[ ! -f "$ORT_DLL" ]]; then
	tmp="$(mktemp -d)"
	curl -fsSL -o "$tmp/ort.zip" "https://github.com/microsoft/onnxruntime/releases/download/v$ORT_VER/onnxruntime-win-x64-$ORT_VER.zip"
	unzip -q -j "$tmp/ort.zip" "onnxruntime-win-x64-$ORT_VER/lib/onnxruntime.dll" -d "$tmp"
	mkdir -p "$(dirname "$ORT_DLL")" && mv "$tmp/onnxruntime.dll" "$ORT_DLL"
	rm -rf "$tmp"
fi

step "Building chadoverlay.exe ($TARGET)"
cd "$ROOT"
cargo xwin build --release --target "$TARGET"
EXE="$ROOT/target/$TARGET/release/chadoverlay.exe"

step "Publishing to $OUT"
TASKLIST="$(command -v tasklist.exe || echo /mnt/c/Windows/System32/tasklist.exe)"
if [[ -x "$TASKLIST" ]] && "$TASKLIST" 2>/dev/null | grep -qi '^chadoverlay.exe'; then
	die "chadoverlay.exe is running on Windows; quit it from the tray first (files would be locked)"
fi
mkdir -p "$OUT"
cp "$EXE" "$ORT_DLL" "$OUT/"
# Mirror (not merge) so icons removed upstream do not linger. Same files build-wsl.sh stages.
rm -rf "$OUT/data"
mkdir -p "$OUT/data/refs"
cp "$CV/data/refs/catalog.json" "$OUT/data/refs/"
cp -r "$CV/data/refs/base" "$OUT/data/refs/"
cp -r "$CV/data/models" "$CV/data/index" "$OUT/data/"

step "Verifying"
missing=0
for f in chadoverlay.exe onnxruntime.dll data/refs/catalog.json data/refs/base data/index/embed_ids.txt \
	data/models/dinov2-small.onnx data/models/ppocrv5_latin_rec.onnx data/models/ppocrv5_latin_dict.txt; do
	[[ -e "$OUT/$f" ]] || { echo "missing: $OUT/$f" >&2; missing=1; }
done
[[ $missing -eq 0 ]] || die "publish incomplete"

step "Done"
echo "Run on Windows: $(wslpath -w "$OUT" 2>/dev/null || echo "$OUT")\\chadoverlay.exe (EFT in borderless mode)"
