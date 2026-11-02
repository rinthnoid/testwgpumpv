# Integrating vo_wgpu into an mpv tree

`vo_wgpu.c` is a drop-in video output driver for mpv. It renders
everything through the **bwgpu** Rust/wgpu renderer over the stable C ABI
(`include/bwgpu.h`) — there is no OpenGL, no Vulkan-in-C, no D3D11, no
Android media path anywhere in the chain. wgpu selects the platform
backend: DX12 on Windows, Vulkan on Linux, Metal on macOS.

Written against **mpv master (Nov 2025, post-0.40)** — the
`draw_frame`/`flip_page` API and `osd_render()` sub-bitmap pipeline.
mpv's vo API moves occasionally; if a signature drifted, the fixes are
one-liners in this file (the ABI side never changes).

## 1. Build bwgpu

```sh
cd mpv-wgpu
cargo build --release
```

Artifacts (in `target/release/`):

| file | purpose |
| --- | --- |
| `libbwgpu.so` / `.dylib` / `bwgpu.dll` | shared renderer — simplest route |
| `libbwgpu.a` / `bwgpu.lib` | static renderer for in-binary linking |
| `include/bwgpu.h` → repo `include/` | the only header |

## 2. Install the renderer

Easiest is a shared library in the default search path:

```sh
# Linux
sudo install -m644 include/bwgpu.h /usr/local/include/
sudo install -m755 target/release/libbwgpu.so /usr/local/lib/
sudo ldconfig
```

Windows: put `bwgpu.dll` next to `mpv.exe` (or on `PATH`); macOS:
`libbwgpu.dylib` next to `mpv` or in `/usr/local/lib`.

## 3. Copy the glue into the mpv tree

```sh
cp mpv-glue/vo_wgpu.c  <mpv>/video/out/
sudo install -m644 include/bwgpu.h /usr/local/include/   # if not done above
```

## 4. Register the driver in `<mpv>/video/out/vo.c`

Add the extern declaration next to the others:

```c
extern const struct vo_driver video_out_wgpu;
```

and add it to `video_out_drivers[]`. Put it **first** to make `wgpu` the
default output, or anywhere later and select it with `--vo=wgpu`:

```c
static const struct vo_driver *const video_out_drivers[] =
{
    &video_out_wgpu,          // mpv, rebased on wgpu
    &video_out_gpu_next,
    &video_out_gpu,
    ...
```

## 5. Build system (mpv uses a single top-level `meson.build`)

```meson
# with the renderer installed to a default path:
bwgpu = cc.find_library('bwgpu', required: true)
dependencies += bwgpu

# or point meson at the build dir:
bwgpu = cc.find_library('bwgpu',
    dirs: [join_paths(getenv('BWGPU_DIR'), 'target', 'release')])
dependencies += bwgpu

# and the source, next to the other vo files:
sources += files('video/out/vo_wgpu.c')
```

`HAVE_X11` / `HAVE_WAYLAND` / `HAVE_WIN32_DESKTOP` are mpv's existing
feature switches — vo_wgpu reuses whichever window layer the build
already has. No new build options are needed.

## 6. Run

```sh
mpv --vo=wgpu video.mkv
mpv --vo=help        # shows: wgpu (bwgpu) — one renderer, every platform
```

Backend selection inside wgpu follows the platform (DX12 / Vulkan /
Metal). `WGPU_BACKEND=vulkan|dx12|metal|gl` overrides it for debugging —
this is a wgpu feature, not a second render path.

## What vo_wgpu does *not* do (yet)

- **Zero-copy hardware decoding import** (DMABUF / DXGI / IOSurface
  straight into wgpu textures) — milestone M2. Frames are currently
  uploaded from decoder output memory (still no CPU-side color
  conversion; bit depth and P010 shifts normalize in-shader).
- **HDR output** (HDR10/PQ metadata, wide-gamut) — milestone M3 output
  half; SDR BT.601/709/2020 limited/full is complete.
- A `--scale=` mapping (the shader always uses Catmull-Rom for
  upscaling; nearest/bilinear are available over the ABI).

Those limits live in `PORTING.md` with the plan for closing them.
