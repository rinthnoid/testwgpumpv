# mpv-wgpu — "mpv, rebased on wgpu"

[![ci](https://github.com/mpv-wgpu/mpv-wgpu/actions/workflows/ci.yml/badge.svg)](https://github.com/mpv-wgpu/mpv-wgpu/actions/workflows/ci.yml)
[![release](https://github.com/mpv-wgpu/mpv-wgpu/actions/workflows/release.yml/badge.svg)](https://github.com/mpv-wgpu/mpv-wgpu/actions/workflows/release.yml)

One renderer, one dependency path, every platform. mpv-wgpu keeps mpv's
C core (demux, decode, audio, subtitles, input) and replaces **all** of
its rendering backends with a single Rust renderer built on
[wgpu](https://github.com/gfx-rs/wgpu): DX12 on Windows, Vulkan on Linux,
Metal on macOS — chosen by wgpu, not by configure flags.

There is no OpenGL, no Vulkan-in-C, no D3D11/D3D9, no xv, no vdpau, no
Android media path, no libplacebo. See [REMOVALS.md](REMOVALS.md) for
the full deletion list and [PORTING.md](PORTING.md) for the milestone
map.

## Repository layout

```
Cargo.toml               single crate: bwgpu (rlib + cdylib + staticlib)
src/
  params.rs              color math — the single source of truth
  device.rs              adapter/device bootstrap (wgpu picks the backend)
  surface.rs             native window bridging (Win32/Xlib/Wayland/Cocoa)
  video.rs               YUV upload: I420, NV12, 420P10, P010
  osd.rs                 subtitle/OSD bitmaps (straight or premultiplied)
  lib.rs                 Renderer: pipelines, viewport, draw, readback
  capi.rs                the stable C ABI mpv links against
shaders/
  video_blit.wgsl        YUV->RGB, scaling, dithering (one pass)
  osd_blend.wgsl         OSD quads
include/bwgpu.h          the only public header
mpv-glue/
  vo_wgpu.c              the mpv-tree video output driver
  INSTALL.md             drop-in instructions for an mpv checkout
scripts/
  c_smoke.c/.sh          end-to-end C ABI smoke test
  check_vo_wgpu.sh       syntax-checks the glue on all 3 window layers
  strip_mpv.sh/.py       removes the legacy stack from an mpv tree
  mpv_stub/              stub mpv headers for the syntax check
tests/
  uniform_layout.rs      Rust/WGSL/C-header layout drift guards (no GPU)
  render_roundtrip.rs    GPU output vs f64 CPU reference (skips w/o GPU)
```

## Quick start

```sh
cargo build --release
sh scripts/c_smoke.sh          # builds, links C against the ABI, verifies pixels
cargo test                     # layout guards always run; GPU tests auto-skip
sh scripts/check_vo_wgpu.sh    # syntax-check mpv-glue/vo_wgpu.c (x11/wl/win32)
```

Render the demo window (needs a GPU):

```sh
cargo run --release --example player
```

Drop into mpv: [mpv-glue/INSTALL.md](mpv-glue/INSTALL.md).
Strip the legacy stack out of an mpv checkout:
`scripts/strip_mpv.sh /path/to/mpv` (dry-run supported).

## Prebuilt binaries (GitHub Actions)

CI builds and tests every push on Windows, Linux and macOS; the
`release` workflow cross-compiles the C library for all targets below
on a tag push (`git tag v0.1.1 && git push --tags`) and attaches the
archives — plus `SHA256SUMS` — to a GitHub Release:

| Archive | Contents |
| --- | --- |
| `bwgpu-<v>-x86_64-unknown-linux-gnu.tar.gz` | `libbwgpu.so`, `libbwgpu.a`, demo `player` (glibc 2.35+) |
| `bwgpu-<v>-aarch64-unknown-linux-gnu.tar.gz` | same, cross-compiled for ARM64 Linux |
| `bwgpu-<v>-x86_64-unknown-linux-musl.tar.gz` | fully static `libbwgpu.a` + static `player` (musl is a static-only target; no `.so` by design) |
| `bwgpu-<v>-x86_64-pc-windows-msvc.zip` | `bwgpu.dll`, import lib `bwgpu.dll.lib`, static `bwgpu.lib`, `player.exe` |
| `bwgpu-<v>-aarch64-pc-windows-msvc.zip` | same, for ARM64 Windows |
| `bwgpu-<v>-x86_64-apple-darwin.tar.gz` | `libbwgpu.dylib`, `libbwgpu.a`, `player` (Intel) |
| `bwgpu-<v>-aarch64-apple-darwin.tar.gz` | same (Apple Silicon) |
| `bwgpu-<v>-macos-universal2.tar.gz` | both macOS architectures `lipo`-merged |
| `bwgpu-<v>-aarch64-linux-android.tar.gz` | `libbwgpu.so` + `libbwgpu.a` + `ANDROID.txt` (arm64-v8a, API 24+, Vulkan-capable device; no demo binary) |
| `bwgpu-<v>-x86_64-linux-android.tar.gz` | same (x86_64 emulator). armeabi-v7a is deliberately not built — modern NDKs ship no arm libunwind for rustc, and wgpu needs Vulkan (an arm64-era feature) |

Every archive carries `include/bwgpu.h`, `LICENSE` and `README.md`.
To publish a release: push the repo to GitHub, then tag — no other
setup needed (the workflows use only first-party actions and the
built-in `GITHUB_TOKEN`).

## What it renders

- 4:2:0 8/10-bit: `I420` (yuv420p), `NV12`, `420P10` (yuv420p10le), `P010`
  — the formats decoders actually produce. Samples land in uint textures
  verbatim; bit depth and P010 shifts normalize in-shader.
- BT.601 / BT.709 / BT.2020, limited and full range.
- nearest / bilinear / Catmull-Rom scaling, 8×8 ordered (Bayer) dithering
  with frame-parity phase.
- OSD/subtitles as RGBA quads, straight- or premultiplied input
  (mpv master hands over premultiplied BGRA).
- Host-provided video destination rect (aspect ratio / panscan) with
  black letterbox; video-less mode for `--force-window --idle`.

## Guarantees

- **One color math source.** The shader, the uniform packer and the CPU
  reference in the roundtrip test all consume the same numbers — the
  affine coefficients are derived by evaluating one function at basis
  points. There is no second formula to drift.
- **Layout drift cannot compile silently.** `tests/uniform_layout.rs`
  asserts Rust struct sizes/offsets, parses the WGSL struct fields,
  checks shader binding numbers and the C header enum values.
- **A Rust panic never crosses into C.** Every ABI entry point is
  panic-guarded; errors surface as negative ints plus
  `bwgpu_last_error()`.
- **Tests are honest about the environment.** GPU tests probe for an
  adapter and skip with a message when there is none; layout guards run
  everywhere. CI runs the GPU path via lavapipe.

## Status

- M1 (mpv glue): complete — `vo_wgpu.c` + install path + syntax checks.
- M2 (zero-copy hwdec import: DMABUF/DXGI/IOSurface as wgpu textures):
  designed for, not yet implemented (`src/video.rs` already uploads into
  uint textures, which is what imports land in). See PORTING.md.
- M3 (legacy removal): complete as a manifest + tool
  (`REMOVALS.md`, `scripts/strip_mpv.sh`); HDR output is the remaining
  output-side milestone.

License: GPL-2.0-or-later (see [LICENSE](LICENSE)).
