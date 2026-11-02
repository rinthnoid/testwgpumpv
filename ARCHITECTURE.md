# ARCHITECTURE

How mpv-wgpu is put together, and why it is put together this way.

## 1. The one rule

Every pixel goes through wgpu. There is exactly one render path — the
`bwgpu` crate — and exactly one way for a host (mpv, a test, the example)
to drive it: the stable C ABI in `include/bwgpu.h`. Platform differences
start and end at two seams:

1. **wgpu itself** picks the backend: DX12 (Windows), Vulkan (Linux),
   Metal (macOS). No code here selects a backend or owns a second one.
2. **`src/surface.rs`** converts a native window descriptor (hwnd /
   xwin+display / wl_surface / ns_view) into a `wgpu::Surface` via
   raw-window-handle. Four descriptors, no render logic.

Everything else — color conversion, scaling, dithering, OSD blending,
texturing — is shared WGSL executed identically on every platform.

## 2. Color math: one source, zero transcription

The classic failure mode of YUV renderers is formula drift: the GLSL
shader, the CPU-side code and the test reference each carry their own
copy of BT.601/709 coefficients, and they diverge silently.

mpv-wgpu makes that structurally impossible:

- `src/params.rs::conv_f(mtx, range)` is **the** conversion function.
- The affine coefficients shipped to the GPU are not transcribed — they
  are *derived* by evaluating `conv_f` at basis points (1,0,0), (0,1,0),
  (0,0,1), (0,0,0). Range handling (offsets, studio-swing gains) lives
  entirely inside `conv_f`.
- The WGSL fragment shader computes `A @ yuv_norm + b` from those
  coefficients — it contains no colorimetry constants at all.
- `tests/render_roundtrip.rs` renders real frames on the GPU and
  compares against `Params::convert_point` — the same packed bytes —
  evaluated in f64.

`yuv_norm` is defined once: raw sample / (2^depth − 1), with an optional
right-shift for P010-style storage applied before the division. The
shader's `norm()` and the CPU reference agree by construction.

## 3. The uniform contract

`Params` is 8 × `vec4<f32>` = 128 bytes:

| offset | field | contents |
| --- | --- | --- |
| 0 | `mat0..mat2` | rows of A in `rgb = A·yuv + b` |
| 48 | `bias` | b |
| 64 | `src` | luma w/h, chroma w/h |
| 80 | `misc` | depth_y, depth_c, shift_bits, filter |
| 96 | `mode` | interleaved, dither amplitude, frame parity, 0 |
| 112 | `rect` | video destination rect, fractions of target |

The contract is enforced from three directions by
`tests/uniform_layout.rs`: Rust sizes/offsets, the parsed WGSL struct
(field names and order), and the shader's binding numbers. The C header
enums are guarded the same way. If any side drifts, the test suite goes
red — there is no way to "temporarily" change one side.

`rect` is how mpv-wgpu stays ignorant of window management: the host
decides aspect ratio / panscan (`vo_get_src_dst_rects` on the mpv side)
and hands over pixels; the renderer draws video inside that rect and
black outside. A zero-size rect is the video-less mode (`--force-window
--idle`), so one shader serves both.

## 4. Textures and passes

- **Video planes are uint textures** (`R8Uint`, `Rg8Uint`, `R16Uint`,
  `Rg16Uint`) uploaded verbatim — no CPU conversion, no normalized
  formats. This is the deliberate M2 seam: zero-copy imports
  (DMABUF/DXGI/IOSurface) produce the same objects, so the upload path
  becomes an import path without touching the shader.
- **One blit pass** does sampling (nearest/bilinear/Catmull-Rom, manual
  `textureLoad` so 10-bit is exact), chroma upscaling (co-sited
  4:2:0), the matrix, dithering (8×8 Bayer, frame-parity phase), and the
  viewport clip.
- **OSD is a second pass**: one quad per bitmap, straight-alpha RGBA8
  uploaded and premultiplied in-shader, blended One / OneMinusSrcAlpha.
  Premultiplied input (mpv master's SUBBITMAP_BGRA) is normalized on
  upload (`src/osd.rs`), so the shader keeps exactly one premultiply
  step regardless of the host convention.
- A **dummy bind group** (1×1 black uint textures) is built at startup,
  so the renderer is drawable before the first frame arrives.

## 5. The C ABI

`include/bwgpu.h` is the only public header; `src/capi.rs` implements
it. Rules:

- Opaque handles, no Rust types cross the boundary, no callbacks.
- Every entry point is wrapped in `catch_unwind` — a Rust panic becomes
  an error return, never an unwind into C.
- Errors are per-thread (`bwgpu_last_error()`), set on every failure.
- Uploading copies into GPU memory before returning, so callers can free
  their buffers immediately; the renderer borrows nothing across calls.
- Headless (`bwgpu_renderer_new(NULL, …)`) is the same renderer without
  a surface — the screenshot/readback path (`bwgpu_render_offscreen`)
  works with and without a window.

`scripts/c_smoke.c` is the executable version of this contract: it
exercises the full call sequence mpv uses and verifies output pixels on
a machine with a GPU (exit 77 with a clear message where there is none).

## 6. mpv integration

`mpv-glue/vo_wgpu.c` is ~500 lines of glue, no rendering:

- reuses mpv's own window layers (w32 / wayland / x11) for events and
  input — those are not render backends and were never the problem;
- maps `mp_image_params` (libplacebo enums in current master) to
  `BWGPU_MATRIX_*`/`BWGPU_RANGE_*` and the four 4:2:0 formats;
- renders OSD through `osd_render()` → SUBBITMAP_BGRA parts, with the
  BGRA→RGBA byte swap done here (a per-pixel loop that stays in C on
  purpose: the renderer's texture format stays the generic RGBA8);
- takes screenshots through `bwgpu_render_offscreen`;
- presents inside `draw_frame` (wgpu Fifo), `flip_page` is a no-op —
  documented, and the honest description of what happens.

The glue is syntax-checked on all three window layers by
`scripts/check_vo_wgpu.sh` against `scripts/mpv_stub/` — stub headers
that mirror mpv master signatures — so signature drift is caught in CI,
not on someone's mpv build.

## 7. Test strategy

| layer | test | needs GPU |
| --- | --- | --- |
| color math | `basis_eval_matches_known_constants` (BT.601/709 gains, black-point) | no |
| layout | `uniform_layout.rs` (Rust ↔ WGSL ↔ C header) | no |
| conversion | `premul_to_straight_is_lossless_at_extremes` | no |
| render | `render_roundtrip.rs`: GPU output vs f64 CPU reference (2 LSB tolerance), OSD blend assertions | yes, skips w/o adapter |
| C ABI | `c_smoke.c`: full call sequence + pixel checks | yes, skips (exit 77) |
| mpv glue | `check_vo_wgpu.sh`: syntax on x11/wayland/win32 | no |

The GPU-gated tests print why they skip; CI installs lavapipe so the GPU
path is exercised on every push.

## 8. Why a single crate (no workspace)

The previous iteration used a workspace; it is now one package on
purpose. There is one artifact to build, one dependency graph to audit
(wgpu + raw-window-handle + bytemuck + log + pollster — nothing else),
and no version-pinning surface. Leanness here is not aesthetic: the
claim "everything relies on wgpu" is only credible if the dependency
tree makes a second path *impossible to hide*.
