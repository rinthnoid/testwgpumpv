#!/bin/sh
# strip_mpv.sh — turn an mpv checkout into the wgpu-only build.
#
# Removes every legacy render backend (OpenGL/gpu, gpu_next/placebo,
# Vulkan-in-C, D3D11/D3D9, xv, vdpau, vaapi, vo_x11, drm, sdl, wlshm,
# dmabuf-wayland, Android media — see REMOVALS.md for the reasoning),
# rewrites video/out/vo.c so `wgpu` is the registered (default) VO, and
# prints the remaining manual meson steps.
#
# Usage: strip_mpv.sh /path/to/mpv [--dry-run]
#
# Requires: a git checkout of mpv (so removals are reviewable/revertible).
# License: GPL-2.0-or-later. See LICENSE.

set -eu

MPV=${1:?usage: strip_mpv.sh /path/to/mpv [--dry-run]}
DRY=${2:-}

if [ ! -f "$MPV/video/out/vo.c" ]; then
    echo "strip_mpv.sh: $MPV does not look like an mpv checkout" >&2
    exit 1
fi

cd "$(dirname "$0")/.."

REMOVED_FILES="
video/out/vo_gpu.c
video/out/vo_gpu_next.c
video/out/vo_direct3d.c
video/out/vo_vdpau.c
video/out/vo_xv.c
video/out/vo_x11.c
video/out/vo_vaapi.c
video/out/vo_vaapi_x11.c
video/out/vo_dmabuf_wayland.c
video/out/vo_wlshm.c
video/out/vo_drm.c
video/out/vo_sdl.c
video/out/vo_mediacodec_embed.c
video/out/android_common.c
video/out/android_common.h
video/out/gpu
video/out/gpu_next
video/out/placebo
video/out/opengl
video/out/vulkan
video/out/d3d11
video/out/hwdec
osdep/android
audio/out/ao_opensles.c
audio/out/ao_aaudio.c
libmpv/render_gl.h
"

run() {
    if [ "$DRY" = "--dry-run" ]; then
        echo "  (dry) $*"
    else
        "$@"
    fi
}

echo "== removing legacy render backends =="
for f in $REMOVED_FILES; do
    if [ -e "$MPV/$f" ]; then
        run git -C "$MPV" rm -r -q "$f" 2>/dev/null || rm -rf "$MPV/$f"
        echo "  removed $f"
    fi
done

echo "== rewriting video/out/vo.c =="
if [ "$DRY" = "--dry-run" ]; then
    echo "  (dry) python3 scripts/strip_mpv.py $MPV/video/out/vo.c"
else
    cp "$MPV/video/out/vo.c" "$MPV/video/out/vo.c.pre-strip"
    python3 scripts/strip_mpv.py "$MPV/video/out/vo.c"
fi

echo
echo "== manual follow-up (mpv's top-level meson.build) =="
cat <<'EOF'
1. Drop the feature switches and their sources from meson.build:
   gl, egl, glx, vulkan, d3d11, dxva2, vdpau, xv, vaapi (interop parts),
   android, mediacodec-embed, sdl2 (video), dmabuf-wayland, shaderc,
   libplacebo — and any `sources += files('video/out/<removed>')` lines
   left behind by the removals above.
2. Register the bwgpu renderer as shown in mpv-glue/INSTALL.md:
   - find_library('bwgpu') (or the static lib) added to `dependencies`
   - `sources += files('video/out/vo_wgpu.c')`
3. Copy mpv-glue/vo_wgpu.c into video/out/ and install include/bwgpu.h.
4. Build: meson setup build && ninja -C build
   Verify: mpv --vo=help | grep wgpu  (wgpu must be listed first)
EOF
echo
echo "Done. Review with: git -C $MPV status"
