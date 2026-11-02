/* mpv stub: sub/osd.h — sub-bitmap API (signatures from mpv master). */
#ifndef MPV_STUB_OSD_H
#define MPV_STUB_OSD_H

#include <stdbool.h>
#include <stdint.h>

struct osd_state;
struct mp_image;

struct mp_osd_res {
    int w, h;
    int mt, mb, ml, mr;
    double display_par;
};

enum mp_osd_draw_flags {
    OSD_DRAW_SUB_FILTER = (1 << 0),
    OSD_DRAW_SUB_ONLY = (1 << 1),
    OSD_DRAW_OSD_ONLY = (1 << 2),
};

enum sub_bitmap_format {
    SUBBITMAP_EMPTY = 0,
    SUBBITMAP_LIBASS,
    SUBBITMAP_BGRA, /* IMGFMT_BGRA (MSB=A, LSB=B), scaled, premultiplied alpha */
    SUBBITMAP_SUBRANDR,
    SUBBITMAP_COUNT
};

struct sub_bitmap {
    void *bitmap;
    int stride;
    int w, h;
    int x, y;
    int dw, dh;
    int src_x, src_y;
    union {
        struct {
            uint32_t color;
        } libass;
        struct {
            const void *image;
        } subrandr;
        struct {
            int primaries;
            int transfer;
            float max_luma;
            bool video_color_space;
        } bgra;
    };
};

struct sub_bitmaps {
    int render_index;
    enum sub_bitmap_format format;
    struct sub_bitmap *parts;
    int num_parts;
    struct mp_image *packed;
    int packed_w, packed_h;
    int change_id;
    bool video_color_space;
};

struct sub_bitmap_list {
    int64_t change_id;
    int w, h;
    struct sub_bitmaps **items;
    int num_items;
};

struct sub_bitmap_list *osd_render(struct osd_state *osd, struct mp_osd_res res,
                                   double video_pts, int draw_flags,
                                   const bool formats[SUBBITMAP_COUNT]);

#endif
