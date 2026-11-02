/* mpv stub: video/out/x11_common.h — X11 window layer API. */
#ifndef MPV_STUB_X11_COMMON_H
#define MPV_STUB_X11_COMMON_H

#include <stdbool.h>

#include "vo.h"

typedef struct _XDisplay Display;
typedef unsigned long Window;
typedef struct {
    int dummy;
} XVisualInfo;

struct vo_x11_state {
    Display *display;
    Window window;
    /* ... many more fields in the real header ... */
};

bool vo_x11_init(struct vo *vo);
void vo_x11_uninit(struct vo *vo);
void vo_x11_check_events(struct vo *vo);
bool vo_x11_create_vo_window(struct vo *vo, XVisualInfo *vis,
                             const char *classname);
void vo_x11_config_vo_window(struct vo *vo);
int vo_x11_control(struct vo *vo, int *events, int request, void *arg);

#endif
