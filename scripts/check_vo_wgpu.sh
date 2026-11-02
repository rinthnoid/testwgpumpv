#!/bin/sh
# check_vo_wgpu.sh — syntax-check mpv-glue/vo_wgpu.c against stub mpv
# headers (scripts/mpv_stub) for all three window layers. This catches
# typos, signature drift and #if mixups without a full mpv checkout; the
# stubs mirror mpv master (Nov 2025) signatures.
#
# License: GPL-2.0-or-later. See LICENSE.

set -eu
cd "$(dirname "$0")/.."

STUB=scripts/mpv_stub
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

check() {
    printf '%s\n' "$1" > "$TMP/config.h"
    printf '== vo_wgpu syntax check: %s ==\n' "$2"
    CC=${CC:-cc}
    $CC -std=c11 -Wall -Wextra -Wno-unused-parameter \
        -fsyntax-only -I"$TMP" -I"$STUB" -Iinclude mpv-glue/vo_wgpu.c
    printf '   ok\n'
}

check '#define HAVE_X11 1
#define HAVE_WAYLAND 0
#define HAVE_WIN32_DESKTOP 0' x11

check '#define HAVE_X11 0
#define HAVE_WAYLAND 1
#define HAVE_WIN32_DESKTOP 0' wayland

check '#define HAVE_X11 0
#define HAVE_WAYLAND 0
#define HAVE_WIN32_DESKTOP 1' win32

printf '#define HAVE_X11 0\n#define HAVE_WAYLAND 0\n#define HAVE_WIN32_DESKTOP 0\n' \
    > "$TMP/config.h"
echo "== vo_wgpu syntax check: none (must fail) =="
CC=${CC:-cc}
if $CC -std=c11 -Wall -Wextra -Wno-unused-parameter \
      -fsyntax-only -I"$TMP" -I"$STUB" -Iinclude mpv-glue/vo_wgpu.c 2>/dev/null; then
    echo "FAIL - the 'no window layer' build should not compile" >&2
    exit 1
fi
echo "   ok (correctly rejected)"
