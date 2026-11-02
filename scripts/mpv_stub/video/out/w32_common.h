/* mpv stub: video/out/w32_common.h — Win32 window layer API. */
#ifndef MPV_STUB_W32_COMMON_H
#define MPV_STUB_W32_COMMON_H

#include <stdbool.h>
#include <stdint.h>

#include "vo.h"

typedef struct HWND__ *HWND;

struct vo_w32_state {
    int dummy; /* opaque in the real tree */
};

bool vo_w32_init(struct vo *vo);
void vo_w32_uninit(struct vo *vo);
int vo_w32_control(struct vo *vo, int *events, int request, void *arg);
void vo_w32_config(struct vo *vo);
HWND vo_w32_hwnd(struct vo *vo);

#endif
