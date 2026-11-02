/* mpv stub: vo.h — vo_driver/vo/vo_frame/controls (mpv master layout). */
#ifndef MPV_STUB_VO_H
#define MPV_STUB_VO_H

#include <stdbool.h>
#include <stdint.h>

#include "common/common.h"
#include "sub/osd.h"
#include "video/mp_image.h"

struct input_ctx;
struct mp_log;
struct mpv_global;
struct vo;
struct vo_vsync_info;
struct vo_x11_state;
struct vo_w32_state;
struct vo_wayland_state;
struct vo_extra;

enum {
    VO_EVENT_EXPOSE = 1 << 0,
    VO_EVENT_RESIZE = 1 << 1,
    VO_EVENT_ICC_PROFILE_CHANGED = 1 << 2,
    VO_EVENT_WIN_STATE = 1 << 3,
    VO_EVENT_LIVE_RESIZING = 1 << 5,
};

enum mp_voctrl {
    VOCTRL_RESET = 1,
    VOCTRL_CHECK_EVENTS,
    VOCTRL_PAUSE,
    VOCTRL_RESUME,
    VOCTRL_REDRAW,
    VOCTRL_SET_PANSCAN,
    VOCTRL_UPDATE_WINDOW_TITLE, /* char* */
    VOCTRL_SCREENSHOT,          /* struct voctrl_screenshot* */
};

struct voctrl_screenshot {
    bool scaled, subs, osd, high_bit_depth, native_csp;
    struct mp_image *res;
};

#define VO_TRUE true
#define VO_FALSE false
#define VO_ERROR -1
#define VO_NOTAVAIL -2
#define VO_NOTIMPL -3

struct vo_frame {
    int64_t pts;
    bool redraw, repeat;
    bool still;
    struct mp_image *current;
};

struct vo_driver {
    bool encode;
    int caps;
    const char *name;
    const char *description;
    int (*preinit)(struct vo *vo);
    int (*query_format)(struct vo *vo, int format);
    int (*reconfig)(struct vo *vo, struct mp_image_params *params);
    int (*control)(struct vo *vo, uint32_t request, void *data);
    bool (*draw_frame)(struct vo *vo, struct vo_frame *frame);
    void (*flip_page)(struct vo *vo);
    void (*get_vsync)(struct vo *vo, struct vo_vsync_info *info);
    void (*wakeup)(struct vo *vo);
    void (*wait_events)(struct vo *vo, int64_t until_time_ns);
    void (*uninit)(struct vo *vo);
    int priv_size;
    const void *priv_defaults;
};

struct vo {
    const struct vo_driver *driver;
    struct mp_log *log;
    void *priv;
    struct mpv_global *global;
    union {
        struct vo_x11_state *x11;
        struct vo_w32_state *w32;
        struct vo_wayland_state *wl;
    };
    struct input_ctx *input_ctx;
    struct osd_state *osd;
    bool want_redraw;
    int dwidth;
    int dheight;
    struct mp_image_params *params;
};

struct vo_vsync_info {
    int64_t last_queue_display_time;
    int64_t vsync_duration;
    int64_t skipped_vsyncs;
};

void vo_get_src_dst_rects(struct vo *vo, struct mp_rect *out_src,
                          struct mp_rect *out_dst, struct mp_osd_res *out_osd);
void vo_event(struct vo *vo, int event);

#endif
