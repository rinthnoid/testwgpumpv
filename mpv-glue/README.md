# mpv-glue — landing `vo_wgpu` inside the mpv tree

This directory contains the M1 deliverable: the mpv-tree video output
driver that plugs the bwgpu renderer into mpv. It is written against
mpv master (`a687796`, 2026-10-30) and uses only the public VO driver
interface plus mpv's existing per-platform windowing helpers.

Everything in here depends on exactly one rendering library: `libbwgpu`
(the wgpu renderer built from this repo). No OpenGL, no C Vulkan, no
D3D11, no Android media path.

## Files

| File | Destination in the mpv tree |
| --- | --- |
| `vo_wgpu.c` | `video/out/vo_wgpu.c` |

## Step by step

1. **Build + install the renderer**

   ```sh
   cargo build --release          # this repo
   # produces target/release/libbwgpu.so|dylib|dll and include/bwgpu.h
   ```

2. **Copy the driver**

   ```sh
   cp mpv-glue/vo_wgpu.c <mpv-tree>/video/out/
   cp include/bwgpu.h <mpv-tree>/video/out/   # or a shared include dir
   ```

3. **Register it in `video/out/vo.c`**

   ```c
   extern const struct vo_driver video_out_wgpu;   // next to the others
   // ...
   &video_out_wgpu,                                // in the driver list
   ```

4. **Register it in `video/out/meson.build`**

   ```meson
   vo_wgpu = declare_dependency(
     compile_args: ['-I' + bwgpu_include_dir],
     dependencies: cc.find_library('bwgpu',
       dirs: ['/path/to/mpv-wgpu/target/release']),
   )
   # then, in the sources list next to vo_wlshm.c:
   sources += files('video/out/vo_wgpu.c')
   # and add `vo_wgpu` to the optional feature wiring, e.g.:
   #   features += {'wgpu': vo_wgpu.found()}   (adapt to local style)
   ```

5. **Run it**

   ```sh
   mpv --vo=wgpu --hwdec=no video.mkv
   ```

## Platform notes

- **Linux** — Wayland first, X11 fallback; both reuse mpv's
  `wayland_common` / `x11_common` helpers. wgpu picks Vulkan.
- **Windows** — `w32_common` provides the HWND; wgpu picks DX12.
- **macOS** — the renderer is ready (wgpu Metal + `BWGPU_PLATFORM_COCOA`
  with an `NSView*`), but mpv's macOS window lives in Swift
  (`video/out/mac/`). Wiring it up means handing the layer-backed
  `NSView` pointer to `bwgpu_renderer_new` from the Swift window
  controller — a small, contained change tracked as an M1 follow-up.
- **Android media / mediacodec** — intentionally absent. The whole point
  of this rebase is that every platform goes through wgpu; see
  `PORTING.md` for the full removal manifest.

## What M1 does *not* do yet (by design)

- Zero-copy hardware decoding imports (DMABUF/DXGI/IOSurface) — M2.
- HDR/transfer-function output — M3.
- Per-subtitle-bitmap batching (OSD is composited into one RGBA scratch
  image; the C ABI already supports per-bitmap quads for a later pass).
