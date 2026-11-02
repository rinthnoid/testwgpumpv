/* mpv stub: video/out/wayland_common.h — Wayland window layer API. */
#ifndef MPV_STUB_WAYLAND_COMMON_H
#define MPV_STUB_WAYLAND_COMMON_H

#include <stdbool.h>
#include <stdint.h>

#include "vo.h"

struct wl_display;
struct wl_surface;
struct wl_shm;

struct mp_present;
struct mp_vo_opts;

struct vo_wayland_state {
    struct wl_display *display;
    struct wl_surface *surface;
    struct mp_rect geometry;
    struct wl_shm *shm;
    struct mp_present *present;
    bool use_present;
    struct mp_vo_opts *opts;
};

bool vo_wayland_check_visible(struct vo *vo);
bool vo_wayland_init(struct vo *vo);
bool vo_wayland_reconfig(struct vo *vo);
int vo_wayland_control(struct vo *vo, int *events, int request, void *arg);
void vo_wayland_handle_scale(struct vo_wayland_state *wl);
void vo_wayland_set_opaque_region(struct vo_wayland_state *wl, bool alpha);
void vo_wayland_uninit(struct vo *vo);
void vo_wayland_wait_events(struct vo *vo, int64_t until_time_ns);
void vo_wayland_wakeup(struct vo *vo);

#endif
