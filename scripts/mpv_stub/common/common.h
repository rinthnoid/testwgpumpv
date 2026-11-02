/* mpv stub: common/common.h — talloc + log macros as vo files see them. */
#ifndef MPV_STUB_COMMON_H
#define MPV_STUB_COMMON_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>

/* talloc is vendored in mpv (mpv_talloc.h via common.h) */
void *talloc_size(const void *ctx, size_t size);
void talloc_free(void *ptr);

#define MP_VERBOSE(...) ((void)0)
#define MP_INFO(...) ((void)0)
#define MP_WARN(...) ((void)0)
#define MP_ERR(...) ((void)0)
#define MP_FATAL(...) ((void)0)

#endif
