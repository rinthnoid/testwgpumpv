/* bwgpu.h — the only public header of mpv-wgpu ("mpv, rebased on wgpu").
 *
 * One stable C ABI, one rendering path: wgpu. Everything below is
 * intentionally small — an opaque renderer, a frame in, pixels out.
 * Errors are reported per-thread via bwgpu_last_error().
 *
 * Threading: a renderer must be created, used and destroyed on one thread
 * (the thread that owns the window). Different renderers may live on
 * different threads.
 *
 * License: GPL-2.0-or-later. See LICENSE.
 */

#ifndef BWGPU_H
#define BWGPU_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ------------------------------------------------------------------ */
/* Library info                                                        */
/* ------------------------------------------------------------------ */

/* "bwgpu x.y.z" — static string, valid for the process lifetime. */
const char *bwgpu_version(void);

/* Last error on the calling thread, or NULL. The pointer stays valid
 * until the next bwgpu call on the same thread. All fallible entry
 * points return a negative int on failure and set this. */
const char *bwgpu_last_error(void);

/* ------------------------------------------------------------------ */
/* Renderer                                                            */
/* ------------------------------------------------------------------ */

typedef struct bwgpu_renderer bwgpu_renderer;

/* Platform tag for bwgpu_window.platform. */
enum {
    BWGPU_PLATFORM_WIN32   = 0,
    BWGPU_PLATFORM_XLIB    = 1,
    BWGPU_PLATFORM_WAYLAND = 2,
    BWGPU_PLATFORM_COCOA   = 3,
};

/* Native window descriptor. Only the fields relevant to `platform` are
 * read; unused ones may be zero. The referenced window/display must
 * outlive the renderer built from it. */
typedef struct bwgpu_window {
    int32_t platform;
    /* Win32 */
    uintptr_t hwnd;          /* HWND as integer                    */
    /* Xlib */
    uint32_t xwin;           /* X Window id                        */
    /* Xlib / Wayland */
    void *display;           /* Display* / wl_display*             */
    /* Wayland */
    void *wayland_surface;   /* wl_surface*                        */
    /* Cocoa */
    void *ns_view;           /* NSView* (layer-backed recommended) */
} bwgpu_window;

/* Create a renderer. win == NULL => headless (offscreen + readback only,
 * e.g. for screenshots or tests). Returns NULL on failure; fetch the
 * reason with bwgpu_last_error(). */
bwgpu_renderer *bwgpu_renderer_new(const bwgpu_window *win,
                                   uint32_t width, uint32_t height,
                                   int vsync);

/* Destroy. NULL is a no-op. */
void bwgpu_renderer_free(bwgpu_renderer *r);

/* Resize the render target. Safe to call with the current size. */
int bwgpu_renderer_resize(bwgpu_renderer *r, uint32_t width, uint32_t height);

/* ------------------------------------------------------------------ */
/* Video upload                                                        */
/* ------------------------------------------------------------------ */

/* Pixel formats. All 4:2:0; this is what decoders produce. */
enum {
    BWGPU_FMT_I420      = 0, /* planar 8-bit (yuv420p)                 */
    BWGPU_FMT_NV12      = 1, /* 2-plane 8-bit, interleaved chroma      */
    BWGPU_FMT_YUV420P10 = 2, /* planar 10-bit, 16-bit words, LSB       */
    BWGPU_FMT_P010      = 3, /* 2-plane 10-bit, 16-bit words, MSB      */
};

/* One image plane. `size` is the byte length of `data`; `stride` is the
 * row pitch in bytes, 0 = tightly packed. For NV12/P010 the interleaved
 * chroma plane goes into `u` and `v` stays zeroed. */
typedef struct bwgpu_plane {
    const void *data;
    size_t size;
    size_t stride;
} bwgpu_plane;

typedef struct bwgpu_frame {
    int32_t fmt;          /* BWGPU_FMT_*                    */
    uint32_t width;       /* luma width, even               */
    uint32_t height;      /* luma height, even              */
    bwgpu_plane y;
    bwgpu_plane u;
    bwgpu_plane v;        /* planar formats only            */
} bwgpu_frame;

/* Colorspace selection. */
enum {
    BWGPU_MATRIX_BT601  = 0,
    BWGPU_MATRIX_BT709  = 1,
    BWGPU_MATRIX_BT2020 = 2,
    BWGPU_RANGE_LIMITED = 0,
    BWGPU_RANGE_FULL    = 1,
};

/* Scaling filter. */
enum {
    BWGPU_FILTER_NEAREST  = 0,
    BWGPU_FILTER_BILINEAR = 1,
    BWGPU_FILTER_CATMULL_ROM = 2,
};

/* Upload one video frame (copies into GPU memory before returning). */
int bwgpu_set_video(bwgpu_renderer *r, const bwgpu_frame *frame);

/* Select YUV matrix + range. Default: BT.709 limited. */
int bwgpu_set_colorspace(bwgpu_renderer *r, int matrix, int range);

/* Select the video scaling filter. Default: bilinear. */
int bwgpu_set_scale(bwgpu_renderer *r, int filter);

/* Set the video destination rect in target pixels — the host's aspect
 * ratio / panscan decision (mpv: vo_get_src_dst_rects). Pixels outside
 * render black. A zero width or height renders video-less (black + OSD),
 * which is how --force-window --idle presents. Call again after resize;
 * the default is a full-surface blit. */
int bwgpu_set_viewport(bwgpu_renderer *r, int x, int y,
                       uint32_t width, uint32_t height);

/* OSD input alpha convention for bwgpu_add_osd bitmaps:
 *   0 = straight alpha (default, libass convention)
 *   1 = premultiplied alpha (mpv master's SUBBITMAP_BGRA)
 * Set once at startup. Internally normalized so the blend stays exact. */
int bwgpu_set_osd_premul(bwgpu_renderer *r, int premul);

/* ------------------------------------------------------------------ */
/* OSD                                                                 */
/* ------------------------------------------------------------------ */

/* Straight-alpha RGBA8 bitmap blended over the video (libass
 * convention). */
typedef struct bwgpu_bitmap {
    const void *data;     /* RGBA8, tightly packed          */
    size_t size;          /* >= width * 4 * height          */
    uint32_t width;
    uint32_t height;
    int32_t x;            /* destination, target pixels     */
    int32_t y;
} bwgpu_bitmap;

/* Drop all OSD bitmaps added so far. */
int bwgpu_clear_osd(bwgpu_renderer *r);

/* Add one OSD bitmap for subsequent frames. */
int bwgpu_add_osd(bwgpu_renderer *r, const bwgpu_bitmap *bmp);

/* ------------------------------------------------------------------ */
/* Output                                                              */
/* ------------------------------------------------------------------ */

/* Render + present one frame to the window (or to the internal offscreen
 * target when headless). */
int bwgpu_render_frame(bwgpu_renderer *r);

/* Render and read back RGBA8 pixels. out needs `stride * height` bytes
 * (stride 0 = width * 4); pass the exact capacity as out_size. Works
 * with and without a window. This is the screenshot path. */
int bwgpu_render_offscreen(bwgpu_renderer *r, void *out,
                           size_t out_size, size_t stride);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* BWGPU_H */
