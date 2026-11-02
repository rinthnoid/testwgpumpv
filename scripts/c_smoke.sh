#!/bin/sh
# c_smoke.sh — build bwgpu as a C library and run the C smoke test.
#
# This is the "does the ABI mpv links against actually work" gate:
#   cargo build (cdylib) -> cc c_smoke.c against include/bwgpu.h -> run.
#
# Exit codes: 0 = pass (a GPU-less machine counts as a pass, printed as
# SKIP), 1 = failure. Set BWGPU_REQUIRE_GPU=1 to turn the GPU-less skip
# into a hard failure — CI does this on Linux, where lavapipe makes a
# real adapter deterministic.
#
# License: GPL-2.0-or-later. See LICENSE.

set -eu
cd "$(dirname "$0")/.."

[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

OUT=build/c_smoke
mkdir -p "$OUT"

echo "== cargo build --release (cdylib) =="
cargo build --release

echo "== compiling c_smoke.c =="
CC=${CC:-cc}
case "$(uname -s)" in
    Darwin)
        SO=libbwgpu.dylib
        $CC -std=c11 -Wall -Wextra -O2 -Iinclude scripts/c_smoke.c \
            -o "$OUT/c_smoke" -Ltarget/release -lbwgpu \
            -Wl,-rpath,"$(pwd)/target/release"
        ;;
    Linux)
        SO=libbwgpu.so
        $CC -std=c11 -Wall -Wextra -O2 -Iinclude scripts/c_smoke.c \
            -o "$OUT/c_smoke" -Ltarget/release -lbwgpu \
            -Wl,-rpath,"$(pwd)/target/release"
        ;;
    *)
        # Windows (MSVC cdylib): link the import library explicitly and
        # ship the DLL next to the executable so it is found at run time.
        SO=bwgpu.dll
        $CC -std=c11 -Wall -Wextra -O2 -Iinclude scripts/c_smoke.c \
            -o "$OUT/c_smoke.exe" target/release/bwgpu.dll.lib
        cp target/release/bwgpu.dll "$OUT/"
        ;;
esac

[ -f "target/release/$SO" ] || { echo "FAIL - target/release/$SO missing" >&2; exit 1; }

echo "== running c_smoke =="
RC=0
"$OUT/c_smoke" "$OUT/c_smoke_out.ppm" || RC=$?
case "$RC" in
    0)
        echo "== c_smoke PASSED =="
        ;;
    77)
        if [ "${BWGPU_REQUIRE_GPU:-0}" = "1" ]; then
            echo "FAIL - no GPU adapter and BWGPU_REQUIRE_GPU=1" >&2
            exit 1
        fi
        echo "== c_smoke SKIPPED (no GPU on this machine) =="
        ;;
    *)
        echo "FAIL - c_smoke exited with $RC" >&2
        exit "$RC"
        ;;
esac
