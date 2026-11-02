# REMOVALS — the legacy render stack, deleted

`mpv-wgpu` keeps mpv's C core (demux, decode, audio, subs, input, CLI)
and replaces **every** rendering backend with the one Rust/wgpu renderer
(`bwgpu`). This file is the authoritative list of what is removed from an
mpv tree and why. `scripts/strip_mpv.sh` applies it automatically.

Everything removed here is replaced by exactly one thing: **vo_wgpu**
(`mpv-glue/vo_wgpu.c`) rendering through `bwgpu`, where wgpu itself picks
the platform backend — DX12 on Windows, Vulkan on Linux, Metal on macOS.

## 1. The OpenGL stack (RA + contexts)

| Path | What it is | Replaced by |
| --- | --- | --- |
| `video/out/gpu/` | the vo_gpu renderer engine: RA abstraction (`ra.c/h`), GLSL pipeline, OSD compositing, ICC, hwdec interop core | bwgpu WGSL pipeline |
| `video/out/vo_gpu.c` | the vo_gpu driver wrapper | `vo_wgpu.c` |
| `video/out/opengl/` | EGL/GLX/WGL/ANGLE context bootstrap | wgpu surface creation |
| `video/out/gpu/context.c/h` | ra_ctx backend chooser (gl / vulkan / d3d11 / placebo) | deleted concept — one backend |

Why: a hand-written C GL renderer with four context bootstrappers, a
second shader language (GLSL), and per-driver workaround lists. wgpu
provides contexts, shader translation and validation for every platform.

## 2. libplacebo / gpu-next

| Path | What it is | Replaced by |
| --- | --- | --- |
| `video/out/gpu_next/` | vo_gpu_next engine | bwgpu (HDR output tracked as a milestone in PORTING.md) |
| `video/out/placebo/` | libplacebo context glue | — |
| `video/out/vo_gpu_next.c` | the driver wrapper | `vo_wgpu.c` |

Why: a second, parallel renderer with its own dependency. mpv-wgpu is
one renderer, one dependency path.

## 3. Vulkan-in-C and D3D11

| Path | What it is | Replaced by |
| --- | --- | --- |
| `video/out/vulkan/` | Vulkan context + instance/placement helpers for vo_gpu | wgpu's Vulkan backend |
| `video/out/d3d11/` | D3D11 context + helpers | wgpu's DX12 backend |
| `video/out/vo_direct3d.c` | the legacy Direct3D 9 VO | wgpu DX12 |

Why: raw C Vulkan/D3D behind RA duplicates what wgpu already does
correctly, with lifetime and synchronization wgpu validates.

## 4. Hardware-output VOs (xv, vdpau, vaapi, x11, drm, sdl, wayland-software)

| Path | What it is | Replaced by |
| --- | --- | --- |
| `video/out/vo_xv.c` | XVideo (1980s-era overlay scaling) | vo_wgpu |
| `video/out/vo_vdpau.c` | VDPAU presentation + deinterlace | vo_wgpu |
| `video/out/vo_vaapi.c` | VA-API direct output | vo_wgpu |
| `video/out/vo_x11.c` | software XPutImage output | vo_wgpu |
| `video/out/vo_drm.c` | manual KMS/DRM dumb-buffer output | wgpu headless + compositor surfaces (KMS-direct is a wgpu ecosystem gap, see PORTING.md) |
| `video/out/vo_sdl.c` | SDL2 windowed output | vo_wgpu (+ winit example) |
| `video/out/vo_wlshm.c` | Wayland SHM software output | vo_wgpu |
| `video/out/vo_dmabuf_wayland.c` | Wayland direct dmabuf scanout | M2 zero-copy import (DMABUF → wgpu texture) |

Why: each is a separate hand-maintained blit/overlay path with its own
scaling, colorspace and event handling — the exact "manual per-platform
media backends" mpv-wgpu exists to delete.

## 5. Android (all of it)

| Path | What it is | Replaced by |
| --- | --- | --- |
| `video/out/vo_mediacodec_embed.c` | MediaCodec surface embedding VO | vo_wgpu (on Android, wgpu's GLES/Vulkan backend) |
| `video/out/android_common.c/h` | Android JNI window glue | deleted |
| `osdep/android/` | JNI helpers, MediaCodec NDK glue | deleted |
| `audio/out/ao_opensles.c` | OpenSL ES audio output | platform audio APIs (out of render scope) |
| `audio/out/ao_aaudio.c` | AAudio output | platform audio APIs (out of render scope) |

Why: explicitly requested — no manual Android media path. If Android
matters again, it arrives as a wgpu backend (Android GLES/Vulkan is
already a supported wgpu target), not as hand-written JNI plumbing.

## 6. Render-side hwdec interop

| Path | What it is | Replaced by |
| --- | --- | --- |
| `video/out/hwdec/` | GL/Vulkan/D3D interop adapters (vaapi-egl, dxva2, d3d11va, mediacodec, vulkan, ...) | M2: DMABUF / DXGI / IOSurface imported directly as wgpu textures |

Why: these interop adapters exist to move decoder memory into GL/Vulkan/
D3D objects. With wgpu, decoder buffers are imported as wgpu textures —
the adapters' job disappears. Decode-side hwaccel (FFmpeg) is untouched.

## 7. libmpv GL render API

| Path | What it is | Replaced by |
| --- | --- | --- |
| `libmpv/render_gl.h` + GL render glue | clients render mpv video with their own OpenGL context | vo_wgpu inside libmpv; offscreen/readback over the bwgpu ABI |

## 8. Kept on purpose (not render backends)

`vo_null` (dummy), `vo_image` (file writer), `vo_lavc` (encoding), `vo_libmpv`
(render API host), the terminal VOs (`tct`, `kitty`, `sixel`, `caca`),
`vo_x11`'s window layer (`x11_common.c` — events/input only), and all audio
outputs except the Android ones. These neither render video through a GPU
API nor carry per-platform media plumbing.

## 9. Build-system fallout (manual, listed by `strip_mpv.sh`)

In mpv's top-level `meson.build`, drop the features and their sources:
`gl`, `egl-*`, `glx`, `vulkan`, `d3d11`, `dxva2`, `vdpau`, `xv`,
`vaapi-*` (VO/hwdec interop parts), `android`, `mediacodec-embed`,
`sdl2` (video output), `dmabuf-wayland`, plus the `shaderc`/`placebo`
dependencies. Then register vo_wgpu as in `mpv-glue/INSTALL.md`.
