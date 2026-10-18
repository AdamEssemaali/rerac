#!/usr/bin/env bash
# Dumps the same frames from rc-engine (Bevy/wgpu) and rc-metal and compares them pixel by pixel.
#
#   tools/metal/compare.sh <out dir> <first..last> [VAR=value ...]
#
# Both run with RC_AUDIO=0 RC_RESOLUTION=416 RC_DUMP_FRAMES=<first..last> and the extra variables (RC_LEVEL=3,
# RC_FRONTEND=0, RC_DUMP_TICKS=..., ...), into <out dir>/wgpu and <out dir>/metal; then compare.py prints, per run,
# the mean / minimum share of byte-identical pixels and of pixels within 2 levels, and the eight worst frames.
# Needs the extracted data (RC_EXTRACTED / RC_DATA_DIR as for the game) and Python 3 with Pillow.
set -euo pipefail
[ $# -ge 2 ] || { sed -n '2,9p' "$0"; exit 2; }
out="$(mkdir -p "$1" && cd "$1" && pwd)"; range="$2"; shift 2
repo="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo"
rm -rf "$out/wgpu" "$out/metal"
mkdir -p "$out/wgpu" "$out/metal"
cargo build -q -p rc-metal
env RC_AUDIO=0 RC_RESOLUTION=416 RC_DUMP_FRAMES="$range" RC_DUMP_DIR="$out/wgpu" "$@" \
  cargo run -q -p rc-engine --features dev > "$out/wgpu.log" 2>&1 || echo "rc-engine exited with $? (log: $out/wgpu.log)"
env RC_AUDIO=0 RC_RESOLUTION=416 RC_DUMP_FRAMES="$range" RC_DUMP_DIR="$out/metal" "$@" \
  "${CARGO_TARGET_DIR:-target}/debug/rerac-metal" > "$out/metal.log" 2>&1 || echo "rc-metal exited with $? (log: $out/metal.log)"
python3 "$repo/tools/metal/compare.py" "$out/wgpu" "$out/metal"
