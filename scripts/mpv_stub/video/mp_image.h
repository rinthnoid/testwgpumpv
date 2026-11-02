/* mpv stub: video/mp_image.h — mp_image, mp_image_params, pl color enums. */
#ifndef MPV_STUB_MP_IMAGE_H
#define MPV_STUB_MP_IMAGE_H

#include <stddef.h>
#include <stdint.h>

struct mp_rect {
    int x0, y0, x1, y1;
};
static inline int mp_rect_w(struct mp_rect rc) { return rc.x1 - rc.x0; }
static inline int mp_rect_h(struct mp_rect rc) { return rc.y1 - rc.y0; }

/* libplacebo enums as embedded in mpv master (via csputils.h) */
enum pl_color_system {
    PL_COLOR_SYSTEM_UNKNOWN = 0,
    PL_COLOR_SYSTEM_BT_601,
    PL_COLOR_SYSTEM_BT_709,
    PL_COLOR_SYSTEM_SMPTE_240M,
    PL_COLOR_SYSTEM_BT_2020_NC,
    PL_COLOR_SYSTEM_BT_2020_C,
    PL_COLOR_SYSTEM_YCGCO,
};

enum pl_color_levels {
    PL_COLOR_LEVELS_UNKNOWN = -1,
    PL_COLOR_LEVELS_LIMITED,
    PL_COLOR_LEVELS_FULL,
};

struct pl_color_repr {
    enum pl_color_system sys;
    enum pl_color_levels levels;
    int bits; /* unused in the stub */
};

enum mp_imgfmt {
    IMGFMT_NONE = 0,
    IMGFMT_420P,
    IMGFMT_NV12,
    IMGFMT_420P10,
    IMGFMT_P010,
    IMGFMT_BGRA,
};

struct mp_image {
    enum mp_imgfmt imgfmt;
    int w, h;
    double pts;
    uint8_t *planes[4];
    int stride[4];
};

struct mp_image_params {
    enum mp_imgfmt imgfmt;
    int w, h;
    int p_w, p_h;
    struct pl_color_repr repr;
    int rotate;
};

struct mp_image *mp_image_alloc(enum mp_imgfmt imgfmt, int w, int h);

#endif
