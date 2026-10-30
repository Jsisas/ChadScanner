#!/usr/bin/env bash
# Builds RatScanner for Windows from WSL2/Linux, including tarkov-vision (tv.dll) and its data.
# Rerun-safe: every step skips or overwrites; config.cfg and UserData/ in the output folder are kept.
#
# Usage: tools/build-wsl.sh [--out DIR] [--tv DIR] [--refresh-refs]
#   --out           publish folder (default /mnt/c/RatScanner, i.e. C:\RatScanner)
#   --tv            tarkov-vision repo (default ../tarkov-vision next to this repo)
#   --refresh-refs  re-download item icons from tarkov.dev (after a wipe/patch) and rebuild the index
set -euo pipefail

RS="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TV="$RS/../tarkov-vision"
OUT="/mnt/c/RatScanner"
REFRESH=0
TARGET=x86_64-pc-windows-msvc

while [[ $# -gt 0 ]]; do
	case "$1" in
		--out) OUT="$2"; shift 2 ;;
		--tv) TV="$2"; shift 2 ;;
		--refresh-refs) REFRESH=1; shift ;;
		-h|--help) sed -n '2,9p' "$0"; exit 0 ;;
		*) echo "unknown argument: $1" >&2; exit 2 ;;
	esac
done

step() { printf '\n==> %s\n' "$*"; }
die() { echo "error: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null || die "$1 not found. $2"; }

step "Checking tools"
need cargo "Install Rust: https://rustup.rs"
need rustup "Install Rust via rustup (https://rustup.rs); distro cargo cannot add the Windows target"
need dotnet "Install the .NET 10 SDK: https://learn.microsoft.com/dotnet/core/install/linux-ubuntu"
need clang "sudo apt install clang lld llvm"
need lld-link "sudo apt install lld"
need llvm-lib "sudo apt install llvm"
rustup target list --installed | grep -qx "$TARGET" || rustup target add "$TARGET"
command -v cargo-xwin >/dev/null || cargo install --locked cargo-xwin
[[ -f "$TV/Cargo.toml" ]] || die "tarkov-vision not found at $TV (clone git@github.com:Jsisas/ChadVision.git there or pass --tv)"
TV="$(cd "$TV" && pwd)"
[[ -f "$RS/RatEye/RatEye/RatEye.csproj" ]] || (cd "$RS" && git submodule update --init --recursive)

step "Building tv.dll ($TARGET)"
(cd "$TV" && cargo xwin build --release --lib --target "$TARGET")
DLL_DIR="$TV/target/$TARGET/release"
[[ -f "$DLL_DIR/tv.dll" ]] || die "tv.dll was not produced in $DLL_DIR"

step "Preparing tarkov-vision data"
cd "$TV"
if [[ $REFRESH -eq 1 || ! -f data/refs/catalog.json ]]; then
	cargo run --release -- fetch-refs --out data/refs
	rm -rf data/index
fi
[[ -f data/models/dinov2-small.onnx && -f data/models/ppocrv5_latin_rec.onnx && -f data/models/ppocrv5_latin_dict.txt ]] \
	|| cargo run --release -- fetch-models
# The bench run builds data/index as a side effect; it is reused when the catalog is unchanged.
[[ -f data/index/embed_ids.txt ]] || cargo run --release -- bench --set data/real --methods fusion --out /tmp/tv-index-bench.md

step "Staging tv.dll and data into RatScanner"
NATIVE="$RS/RatScanner/Native"
DATA="$RS/RatScanner/Data/tv"
mkdir -p "$NATIVE"
rm -f "$NATIVE"/*.dll
cp "$DLL_DIR/tv.dll" "$NATIVE/"
find "$DLL_DIR" -maxdepth 1 -name 'onnxruntime*.dll' -exec cp {} "$NATIVE/" \;
# Mirror (not merge) so icons removed upstream do not linger.
rm -rf "$DATA"
mkdir -p "$DATA/refs"
cp "data/refs/catalog.json" "$DATA/refs/"
cp -r data/refs/base "$DATA/refs/"
cp -r data/models data/index "$DATA/"

step "Publishing RatScanner to $OUT"
if command -v tasklist.exe >/dev/null && tasklist.exe 2>/dev/null | grep -qi '^RatScanner.exe'; then
	die "RatScanner.exe is running on Windows; close it first (files would be locked)"
fi
mkdir -p "$OUT"
cd "$RS"
dotnet publish RatScanner/RatScanner.csproj -c Release -r win-x64 --self-contained \
	-p:EnableWindowsTargeting=true -o "$OUT"
[[ -f "$OUT/tv.dll" ]] || die "tv.dll missing from $OUT"

step "Done"
echo "Run on Windows: $(wslpath -w "$OUT" 2>/dev/null || echo "$OUT")\\RatScanner.exe"
