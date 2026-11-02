# PORTING — milestones, gaps, and where the seams are

This file is the honest status page: what works today, what is designed
for but not built, and exactly where new work attaches.

## M1 — mpv glue (complete)

- `mpv-glue/vo_wgpu.c`: preinit/reconfig/control/draw_frame/flip_page/
  uninit over the bwgpu ABI, using mpv's w32/wayland/x11 window layers.
- Colorspace mapping from mpv master's `mp_image_params` (libplacebo
  enums); BGRA OSD (premultiplied, mpv master convention) with byte
  swap; screenshots via readback; aspect/panscan via `bwgpu_set_viewport`.
- `mpv-glue/INSTALL.md`: registration in `vo.c` + meson wiring.
- `scripts/strip_mpv.sh` + `REMOVALS.md`: the legacy-stack removal.
- Known wobbles (mpv moves; each is a one-line fix in the glue):
  - `vo_x11_create_vo_window(vo, NULL, …)` assumes the NULL-visual path
    picks the default visual; if a specific mpv revision requires an
    explicit visual, pass one.
  - X11 window ids are truncated to `uint32_t` (XIDs are 32-bit on the
    wire — safe in practice).
  - Presenting inside `draw_frame` (flip_page no-op) is one sub-step
    earlier than vo_wlshm's commit-in-flip_page; harmless with Fifo.

## M2 — zero-copy hardware-decode import (designed, not built)

**Goal**: decoder-produced DMABUF (Linux), DXGI shared handles
(Windows), IOSurface (macOS) become wgpu textures directly — no staging
copy on the render side.

**Why the current design is ready for it**: video planes already live in
**uint textures** (`R8Uint`/`Rg8Uint`/`Rg16Uint`), and all bit-depth /
shift normalization happens in-shader (`norm()`). Imported decoder
memory is exactly this shape — NV12 is two planes, P010 the 16-bit
variant. The shader does not change at all.

**What needs building**:
1. **wgpu-side**: import via `hal` (wgpu-hal device wrapper) —
   `Device::create_texture_from_hal` with `TextureDescriptor` marked
   appropriately, plus the platform import structs
   (`hal::vulkan::Device::texture_from_raw`, DX12 `D3D12_RESOURCE_DESC`
   path, Metal `IOSurface` path). wgpu 25 exposes all three; each needs
   a small `#[cfg(target_os)]` block in a new `src/import.rs`.
2. **mpv-side**: vo_wgpu takes frames when `frame->current->imgfmt` is a
   hw prefix (AV_PIX_FMT_VAAPI / D3D11 / VIDEOTOOLBOX), pulls the AVDRMFrameDescriptor
   / DXGI handle / IOSurface out of the mp_image's AVBufferRef, maps
   it to `bwgpu_import_*` (new ABI functions), and falls back to the
   current upload path when import is unavailable (format, driver,
   modifier).
3. **ABI**: `bwgpu_import_dmabuf / bwgpu_import_dxgi / bwgpu_import_iosurface`
   alongside `bwgpu_set_video` — same renderer state machine, so
   colorspace/filter/viewport code is untouched.
4. **Tests**: the roundtrip harness gains an import-vs-upload
   equivalence check (same input planes through both paths must produce
   identical readbacks) on machines that expose the import paths.

Fallout / prerequisite: mpv's `--hwdec=…` must be configured to hand
over drm_prime / D3D11 textures to the VO — that is decode-side config,
not render code.

## M3 — legacy removal (manifest complete) + HDR output (remaining)

**Removal half: complete.** `REMOVALS.md` is the authority;
`scripts/strip_mpv.sh` executes it (files, vo.c rewriter with
`#if`-guard collapsing, wgpu registration) and prints the remaining
manual meson steps. After stripping, `wgpu` is the first (default) VO
and nothing else renders.

**Output half: HDR**. Current output is SDR (Rgba8/Bgra8 non-sRGB
targets, clamp-to-1.0, no transfer functions). The path to HDR:

1. Target formats: prefer `Rgba16Float` when the surface offers it
   (`src/surface.rs::pick_format` extension + a `caps` probe).
2. Add the inverse-EOTF/OETF stage to `video_blit.wgsl` — a small
   `transfer` field in `Params.misc.w` (the filter enum moves or packs
   into a nibble) with PQ / HLG / linear / sRGB transfer functions as
   pure math, same basis-eval discipline as the matrix.
3. Metadata plumbing: `bwgpu_set_hdr(r, max_luma, min_luma, primaries)`
   + mpv-side `vo->target_params` / `VOCTRL_GET_ICC_PROFILE` handling —
   the same place colorspace is set today.
4. Wide-gamut: extend `YuvMatrix` with primaries-to-XYZ output matrices
   (BT.2020 container over BT.709 content and vice versa).

Chroma-upscaling quality note: Catmull-Rom is the current ceiling; a
spline36/gaussian set slots into the same `fetch()` dispatcher.

## Other known gaps (deliberate, listed)

- **KMS/DRM direct output**: wgpu has no KMS scanout backend; after the
  strip, `--vo=wgpu` on a TTY needs a compositor or the future wgpu DRM
  support. vo_drm was removed with eyes open (REMOVALS.md §4).
- **`--scale=` mapping**: the shader always uses Catmull-Rom for
  upscaling today; the ABI exposes nearest/bilinear/Catmull-Rom.
- **ICC profiles / 3D LUTs**: not implemented; vo_wgpu answers
  `VOCTRL_GET_ICC_PROFILE` through the window layer but does not apply
  it. The natural place is a pre-blit 3D texture stage.
- **Performance data** (`VOCTRL_PERFORMANCE_DATA`): timestamps exist in
  wgpu (pass timestamp writes) but are not wired.

## Verification commands

```sh
cargo test
cargo clippy --all-targets
cargo fmt --check
sh scripts/c_smoke.sh          # GPU machine: full pixel verification
sh scripts/check_vo_wgpu.sh    # glue syntax on all 3 window layers
sh scripts/strip_mpv.sh /path/to/mpv --dry-run
```
