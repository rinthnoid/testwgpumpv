/*
 * vo_wgpu.c — mpv video output driver on bwgpu ("mpv, rebased on wgpu").
 *
 * Everything is rendered by the bwgpu Rust/wgpu renderer through the
 * stable C ABI in <bwgpu.h>. This file is glue only: it maps mpv's frame
 * queue, colorspace metadata, OSD pipeline and window events onto that
 * ABI. There is no rendering code here — and deliberately no
 * OpenGL/Vulkan-C/D3D/xv/vdpau/Android fallback. wgpu picks the platform
 * backend (DX12 on Windows, Vulkan on Linux, Metal on macOS via the
 * Cocoa surface path in bwgpu).
 *
 * The window itself is mpv's own per-platform window/input layer
 * (w32_common / wayland_common / x11_common). Those handle events, input
 * and window state only — never rendering.
 *
 * Supported input: 4:2:0 planar/interleaved, 8/10-bit (IMGFMT_420P,
 * IMGFMT_NV12, IMGFMT_420P10, IMGFMT_P010). VO_CAP_ROTATE90 is not
 * claimed; the core rotates upstream. Colorimetry is mapped from
 * mp_image_params (libplacebo enums in current mpv master).
 *
 * Written against mpv master (Nov 2025, post-0.40 APIs: draw_frame /
 * flip_page / osd_render). See INSTALL.md for tree integration.
 *
 * License: GPL-2.0-or-later. See LICENSE.
 */

#include "config.h"

#include <stdint.h>
#include <string.h>

#include <bwgpu.h>

#include "common/common.h"
#include "sub/osd.h"
#include "video/mp_image.h"
#include "vo.h"

#if HAVE_WIN32_DESKTOP
#include "video/out/w32_common.h"
#elif HAVE_WAYLAND
#include "video/out/wayland_common.h"
#elif HAVE_X11
#include "video/out/x11_common.h"
#else
#error "vo_wgpu needs one of: win32 desktop, wayland, x11"
#endif

struct priv {
    bwgpu_renderer *r;
    // Native window descriptor handed to bwgpu_renderer_new.
    bwgpu_window win;
    // BWGPU_FMT_* selected at reconfig, -1 before.
    int imgfmt;
    // Rects from the last resize (video placement + OSD resolution).
    struct mp_rect src, dst;
    struct mp_osd_res osd_res;
};

// ---------------------------------------------------------------------------
// Renderer lifecycle
// ---------------------------------------------------------------------------

// Build the bwgpu renderer once a native window exists. Called from
// reconfig(); by then every platform window layer has its window.
static int ensure_renderer(struct vo *vo)
{
    struct priv *p = vo->priv;
    if (p->r)
        return 0;

#if HAVE_WIN32_DESKTOP
    p->win.platform = BWGPU_PLATFORM_WIN32;
    p->win.hwnd = (uintptr_t)vo_w32_hwnd(vo);
    if (!p->win.hwnd) {
        MP_ERR(vo, "[wgpu] no Win32 window\n");
        return -1;
    }
#elif HAVE_WAYLAND
    p->win.platform = BWGPU_PLATFORM_WAYLAND;
    p->win.display = vo->wl->display;
    p->win.wayland_surface = vo->wl->surface;
    if (!p->win.display || !p->win.wayland_surface) {
        MP_ERR(vo, "[wgpu] no Wayland surface\n");
        return -1;
    }
#elif HAVE_X11
    p->win.platform = BWGPU_PLATFORM_XLIB;
    p->win.xwin = vo->x11->window;
    p->win.display = vo->x11->display;
    if (!p->win.xwin || !p->win.display) {
        MP_ERR(vo, "[wgpu] no X11 window\n");
        return -1;
    }
#endif

    // vsync = 1 keeps wgpu's Fifo present mode; mpv's display-sync does
    // the fine-grained timing on top. The window must outlive the
    // renderer: uninit() frees the renderer before the window layer.
    p->r = bwgpu_renderer_new(&p->win, vo->dwidth > 0 ? vo->dwidth : 64,
                              vo->dheight > 0 ? vo->dheight : 64, 1);
    if (!p->r) {
        MP_ERR(vo, "[wgpu] renderer creation failed: %s\n",
               bwgpu_last_error() ? bwgpu_last_error() : "?");
        return -1;
    }
    // mpv master renders SUBBITMAP_BGRA with premultiplied alpha.
    bwgpu_set_osd_premul(p->r, 1);
    MP_VERBOSE(vo, "[wgpu] %s, adapter: n/a (see log)\n", bwgpu_version());
    return 0;
}

static void uninit(struct vo *vo)
{
    struct priv *p = vo->priv;
    // Renderer first — it borrows the native window.
    if (p->r) {
        bwgpu_renderer_free(p->r);
        p->r = NULL;
    }
#if HAVE_WIN32_DESKTOP
    vo_w32_uninit(vo);
#elif HAVE_WAYLAND
    vo_wayland_uninit(vo);
#elif HAVE_X11
    vo_x11_uninit(vo);
#endif
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

// Apply the current window size to the renderer and recompute the
// video/OSD rects. Also the VOCTRL_SET_PANSCAN entry point.
static int resize(struct vo *vo)
{
    struct priv *p = vo->priv;
    if (!p->r || vo->dwidth <= 0 || vo->dheight <= 0)
        return 0;

#if HAVE_WAYLAND
    // Wayland reports the logical surface size via wl->geometry (mirrors
    // vo_wlshm); dwidth/dheight follow it.
    if (vo->wl) {
        int32_t gw = mp_rect_w(vo->wl->geometry);
        int32_t gh = mp_rect_h(vo->wl->geometry);
        if (gw > 0 && gh > 0) {
            vo->dwidth = gw;
            vo->dheight = gh;
        }
        vo_wayland_handle_scale(vo->wl);
    }
#endif

    if (bwgpu_renderer_resize(p->r, vo->dwidth, vo->dheight) < 0) {
        MP_ERR(vo, "[wgpu] resize failed: %s\n", bwgpu_last_error());
        return -1;
    }
    // mpv owns aspect ratio / panscan decisions; hand the resulting
    // destination rect to the renderer (letterboxing is its job).
    vo_get_src_dst_rects(vo, &p->src, &p->dst, &p->osd_res);
    vo->want_redraw = true;
    return 0;
}

// ---------------------------------------------------------------------------
// Video upload
// ---------------------------------------------------------------------------

static int mpimgfmt_to_bwgpu(int imgfmt)
{
    switch (imgfmt) {
    case IMGFMT_420P:   return BWGPU_FMT_I420;
    case IMGFMT_NV12:   return BWGPU_FMT_NV12;
    case IMGFMT_420P10: return BWGPU_FMT_YUV420P10;
    case IMGFMT_P010:   return BWGPU_FMT_P010;
    default:            return -1;
    }
}

// Bytes per sample for one plane of a supported format (chroma of
// NV12/P010 is 2 bytes/texel; 10-bit planar is 2 bytes/texel).
static int plane_bpp(int bwfmt, int idx)
{
    switch (bwfmt) {
    case BWGPU_FMT_I420:     return 1;
    case BWGPU_FMT_NV12:     return idx == 0 ? 1 : 2;
    case BWGPU_FMT_YUV420P10: return 2;
    case BWGPU_FMT_P010:     return 2;
    default:                 return 1;
    }
}

// Wrap one mp_image plane for the ABI. `size` is the exact number of
// bytes the renderer may touch: stride * (rows - 1) + row width, so no
// byte beyond the mp_image allocation is ever read.
static void set_plane(struct bwgpu_plane *pl, struct mp_image *mpi,
                      int idx, int bwfmt, int w, int h)
{
    pl->stride = mpi->stride[idx];
    pl->size = (size_t)mpi->stride[idx] * (h - 1) + (size_t)w * plane_bpp(bwfmt, idx);
    pl->data = mpi->planes[idx];
}

// ---------------------------------------------------------------------------
// OSD
// ---------------------------------------------------------------------------

// mpv BGRA (bytes B,G,R,A; MSB=A) -> bwgpu RGBA (bytes R,G,B,A). Alpha
// convention (premultiplied) is normalized inside bwgpu.
static bool add_osd_part(struct vo *vo, struct sub_bitmap *b)
{
    struct priv *p = vo->priv;
    if (b->w <= 0 || b->h <= 0 || !b->bitmap)
        return true;

    size_t rowlen = (size_t)b->w * 4;
    size_t size = rowlen * (size_t)b->h;
    uint8_t *tmp = talloc_size(NULL, size);
    if (!tmp)
        return false;
    for (int y = 0; y < b->h; y++) {
        const uint8_t *in = (const uint8_t *)b->bitmap + (size_t)y * b->stride;
        uint8_t *out = tmp + rowlen * (size_t)y;
        for (int x = 0; x < (int)rowlen; x += 4) {
            out[x + 0] = in[x + 2];
            out[x + 1] = in[x + 1];
            out[x + 2] = in[x + 0];
            out[x + 3] = in[x + 3];
        }
    }
    struct bwgpu_bitmap bmp = {
        .data = tmp,
        .size = size,
        .width = b->w,
        .height = b->h,
        .x = b->x,
        .y = b->y,
    };
    // BGRA parts are pre-scaled by mpv (w==dw, h==dh); the renderer draws
    // 1:1, which matches.
    bool ok = bwgpu_add_osd(p->r, &bmp) == 0;
    if (!ok)
        MP_ERR(vo, "[wgpu] OSD upload failed: %s\n", bwgpu_last_error());
    talloc_free(tmp);
    return ok;
}

static void draw_osd(struct vo *vo, double pts)
{
    struct priv *p = vo->priv;
    bwgpu_clear_osd(p->r);
    if (!vo->osd)
        return;

    const bool formats[SUBBITMAP_COUNT] = {
        [SUBBITMAP_BGRA] = true,
    };
    struct sub_bitmap_list *list =
        osd_render(vo->osd, p->osd_res, pts, 0, formats);
    if (!list)
        return;
    for (int n = 0; n < list->num_items; n++) {
        struct sub_bitmaps *item = list->items[n];
        if (item->format != SUBBITMAP_BGRA)
            continue;
        for (int i = 0; i < item->num_parts; i++) {
            if (!add_osd_part(vo, &item->parts[i]))
                break;
        }
    }
    talloc_free(list);
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

static bool draw_frame(struct vo *vo, struct vo_frame *frame)
{
    struct priv *p = vo->priv;
    if (!p->r)
        return VO_FALSE;

    struct mp_image *src = frame->current;

    if (src && p->imgfmt >= 0) {
        int cw = (src->w + 1) / 2;
        int ch = (src->h + 1) / 2;
        struct bwgpu_frame f;
        memset(&f, 0, sizeof(f));
        f.fmt = p->imgfmt;
        f.width = src->w;
        f.height = src->h;
        set_plane(&f.y, src, 0, p->imgfmt, src->w, src->h);
        set_plane(&f.u, src, 1, p->imgfmt, cw, ch);
        if (p->imgfmt == BWGPU_FMT_I420 || p->imgfmt == BWGPU_FMT_YUV420P10)
            set_plane(&f.v, src, 2, p->imgfmt, cw, ch);
        if (bwgpu_set_video(p->r, &f) < 0) {
            MP_ERR(vo, "[wgpu] frame upload failed: %s\n", bwgpu_last_error());
            return VO_FALSE;
        }
    }

    // No frame (--force-window --idle): zero-size rect => black + OSD.
    if (src) {
        bwgpu_set_viewport(p->r, p->dst.x0, p->dst.y0,
                           mp_rect_w(p->dst), mp_rect_h(p->dst));
    } else {
        bwgpu_set_viewport(p->r, 0, 0, 0, 0);
    }

    draw_osd(vo, src ? src->pts : 0);

    // Present happens here (wgpu surface present, Fifo); flip_page stays
    // a no-op — by the time draw_frame returns, the frame is queued.
    if (bwgpu_render_frame(p->r) < 0) {
        MP_ERR(vo, "[wgpu] render failed: %s\n", bwgpu_last_error());
        return VO_FALSE;
    }
    return VO_TRUE;
}

static void flip_page(struct vo *vo)
{
    // Presentation already queued by bwgpu_render_frame (see draw_frame).
}

// ---------------------------------------------------------------------------
// Screenshots
// ---------------------------------------------------------------------------

// Composite screenshot (video + OSD) through the renderer's readback
// path. Returned image is refcounted; the caller unrefs it.
static struct mp_image *screenshot(struct vo *vo)
{
    struct priv *p = vo->priv;
    if (!p->r || vo->dwidth <= 0 || vo->dheight <= 0)
        return NULL;

    struct mp_image *img = mp_image_alloc(IMGFMT_BGRA, vo->dwidth, vo->dheight);
    size_t n = (size_t)vo->dwidth * 4 * vo->dheight;
    uint8_t *buf = talloc_size(NULL, n);
    if (!img || !buf) {
        talloc_free(buf);
        talloc_free(img);
        return NULL;
    }
    if (bwgpu_render_offscreen(p->r, buf, n, (size_t)vo->dwidth * 4) < 0) {
        MP_ERR(vo, "[wgpu] screenshot failed: %s\n", bwgpu_last_error());
        talloc_free(buf);
        talloc_free(img);
        return NULL;
    }
    // Readback is RGBA8; mp_image is BGRA.
    for (int y = 0; y < vo->dheight; y++) {
        const uint8_t *in = buf + (size_t)y * vo->dwidth * 4;
        uint8_t *out = img->planes[0] + (size_t)y * img->stride[0];
        for (int x = 0; x < vo->dwidth; x++) {
            out[x * 4 + 0] = in[x * 4 + 2];
            out[x * 4 + 1] = in[x * 4 + 1];
            out[x * 4 + 2] = in[x * 4 + 0];
            out[x * 4 + 3] = in[x * 4 + 3];
        }
    }
    talloc_free(buf);
    return img;
}

// ---------------------------------------------------------------------------
// Control
// ---------------------------------------------------------------------------

static int control(struct vo *vo, uint32_t request, void *data)
{
    int events = 0;

    switch (request) {
    case VOCTRL_CHECK_EVENTS:
#if HAVE_WIN32_DESKTOP
        vo_w32_control(vo, &events, request, data);
#elif HAVE_WAYLAND
        vo_wayland_control(vo, &events, request, data);
#elif HAVE_X11
        vo_x11_check_events(vo);
#endif
        break;
    case VOCTRL_SET_PANSCAN:
        if (resize(vo) < 0)
            return VO_ERROR;
        return VO_TRUE;
    case VOCTRL_RESET:
        // The renderer is stateless between frames; nothing to reset.
        return VO_TRUE;
    case VOCTRL_SCREENSHOT: {
        struct voctrl_screenshot *args = data;
        args->res = screenshot(vo);
        return args->res ? VO_TRUE : VO_ERROR;
    }
    default:
        // Window title, cursor visibility, screensaver, unfs window size,
        // display fps, ... — all window-layer business.
#if HAVE_WIN32_DESKTOP
        return vo_w32_control(vo, &events, request, data);
#elif HAVE_WAYLAND
        return vo_wayland_control(vo, &events, request, data);
#elif HAVE_X11
        return vo_x11_control(vo, &events, request, data);
#endif
    }

    if (events & (VO_EVENT_RESIZE | VO_EVENT_LIVE_RESIZING))
        resize(vo);
    if (events & VO_EVENT_EXPOSE)
        vo->want_redraw = true;
    vo_event(vo, events);
    return VO_TRUE;
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

static int preinit(struct vo *vo)
{
    struct priv *p = vo->priv;

#if HAVE_WIN32_DESKTOP
    if (!vo_w32_init(vo))
        return -1;
#elif HAVE_WAYLAND
    if (!vo_wayland_init(vo))
        return -1;
#elif HAVE_X11
    if (!vo_x11_init(vo))
        return -1;
    // X11 wants the window up front; the renderer itself is built at
    // reconfig (ensure_renderer) once config_vo_window has run.
    if (!vo_x11_create_vo_window(vo, NULL, "mpv-wgpu")) {
        vo_x11_uninit(vo);
        return -1;
    }
#endif

    p->imgfmt = -1;
    MP_VERBOSE(vo, "[wgpu] %s — one renderer, every platform\n",
               bwgpu_version());
    return 0;
}

static int query_format(struct vo *vo, int format)
{
    return mpimgfmt_to_bwgpu(format) >= 0;
}

static int reconfig(struct vo *vo, struct mp_image_params *params)
{
    struct priv *p = vo->priv;

    // Window (re)configuration first.
#if HAVE_WIN32_DESKTOP
    vo_w32_config(vo);
#elif HAVE_WAYLAND
    if (!vo_wayland_reconfig(vo))
        return -1;
#elif HAVE_X11
    vo_x11_config_vo_window(vo);
#endif

    if (ensure_renderer(vo) < 0)
        return -1;

    p->imgfmt = mpimgfmt_to_bwgpu(params->imgfmt);
    if (p->imgfmt < 0) {
        MP_ERR(vo, "[wgpu] unsupported pixel format (need 4:2:0 8/10-bit)\n");
        return -1;
    }

    // Colorimetry: mpv master carries it as libplacebo enums in
    // params->repr. Unknown/exotic systems fall back by resolution,
    // mirroring mpv's own guessing heuristic.
    int matrix = BWGPU_MATRIX_BT709;
    switch (params->repr.sys) {
    case PL_COLOR_SYSTEM_BT_601:    matrix = BWGPU_MATRIX_BT601; break;
    case PL_COLOR_SYSTEM_BT_709:    matrix = BWGPU_MATRIX_BT709; break;
    case PL_COLOR_SYSTEM_BT_2020_NC:
    case PL_COLOR_SYSTEM_BT_2020_C: matrix = BWGPU_MATRIX_BT2020; break;
    default:
        matrix = params->w < 1280 ? BWGPU_MATRIX_BT601 : BWGPU_MATRIX_BT709;
        break;
    }
    int range = params->repr.levels == PL_COLOR_LEVELS_FULL
                    ? BWGPU_RANGE_FULL
                    : BWGPU_RANGE_LIMITED;
    bwgpu_set_colorspace(p->r, matrix, range);

    // Upscaling quality: Catmull-Rom for all cases (8/10-bit exact in the
    // shader). A --scale= mapping is future work (see PORTING.md).
    bwgpu_set_scale(p->r, BWGPU_FILTER_CATMULL_ROM);

    resize(vo);
    return 0;
}

const struct vo_driver video_out_wgpu = {
    .description = "wgpu (bwgpu) — one renderer, every platform",
    .name = "wgpu",
    .preinit = preinit,
    .query_format = query_format,
    .reconfig = reconfig,
    .control = control,
    .draw_frame = draw_frame,
    .flip_page = flip_page,
    .uninit = uninit,
    .priv_size = sizeof(struct priv),
#if HAVE_WAYLAND
    .wakeup = vo_wayland_wakeup,
    .wait_events = vo_wayland_wait_events,
#endif
};
