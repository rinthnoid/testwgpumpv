/* c_smoke.c — end-to-end C ABI smoke test for bwgpu.
 *
 * Exercises the exact surface mpv will link against:
 *   version -> headless renderer -> resize -> upload I420 -> colorspace ->
 *   filter -> OSD -> render -> readback verification -> error paths -> free.
 *
 * Usage: c_smoke [out.ppm]
 * Exit codes: 0 = pass, 1 = failure, 77 = skipped (no usable GPU here).
 *
 * Build/run via scripts/c_smoke.sh.
 *
 * License: GPL-2.0-or-later. See LICENSE.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "bwgpu.h"

static int failures = 0;
static int checks = 0;

static void check(int cond, const char *msg)
{
    ++checks;
    if (cond) {
        printf("ok   - %s\n", msg);
        return;
    }
    ++failures;
    const char *e = bwgpu_last_error();
    printf("FAIL - %s%s%s\n", msg, e ? " (" : "", e ? e : "");
}

/* ------------------------------------------------------------------ */
/* Test content: a 64x64 I420 gradient + a solid red OSD rectangle     */
/* ------------------------------------------------------------------ */

#define VID_W 64
#define VID_H 64
#define OSD_W 16
#define OSD_H 16
#define OUT_W 128
#define OUT_H 72

static void make_video(unsigned char *y, unsigned char *u, unsigned char *v)
{
    for (uint32_t j = 0; j < VID_H; j++)
        for (uint32_t i = 0; i < VID_W; i++)
            y[j * VID_W + i] = (unsigned char)((i * 3 + j * 5) & 0xff);
    for (uint32_t j = 0; j < VID_H / 2; j++)
        for (uint32_t i = 0; i < VID_W / 2; i++) {
            u[j * (VID_W / 2) + i] = (unsigned char)(40 + i * 2);
            v[j * (VID_W / 2) + i] = (unsigned char)(200 - j * 2);
        }
}

static void make_osd(unsigned char *rgba)
{
    /* Premultiplied red with an alpha ramp (mpv SUBBITMAP_BGRA convention:
       rgb *= a). The renderer un-premultiplies on upload. */
    for (uint32_t j = 0; j < OSD_H; j++)
        for (uint32_t i = 0; i < OSD_W; i++) {
            unsigned char *px = rgba + (j * OSD_W + i) * 4;
            unsigned a = (i * 255) / (OSD_W - 1);
            px[0] = (unsigned char)a; /* premultiplied R = R * a */
            px[1] = 0;                /* G                        */
            px[2] = 0;                /* B                        */
            px[3] = (unsigned char)a; /* alpha                    */
        }
}

/* ------------------------------------------------------------------ */
/* Pixel verification helpers                                          */
/* ------------------------------------------------------------------ */

static unsigned px(const unsigned char *buf, uint32_t x, uint32_t y, int c)
{
    return buf[(y * OUT_W + x) * 4 + c];
}

static int all_zero(const unsigned char *buf, size_t n)
{
    for (size_t i = 0; i < n; i++)
        if (buf[i])
            return 0;
    return 1;
}

static long luma_spread(const unsigned char *buf)
{
    unsigned lo = 255, hi = 0;
    for (uint32_t j = 0; j < OUT_H; j++)
        for (uint32_t i = 0; i < OUT_W; i++) {
            unsigned l = (px(buf, i, j, 0) + px(buf, i, j, 1) + px(buf, i, j, 2)) / 3;
            if (l < lo) lo = l;
            if (l > hi) hi = l;
        }
    return (long)hi - (long)lo;
}

static void write_ppm(const char *path, const unsigned char *buf)
{
    FILE *f = fopen(path, "wb");
    if (!f)
        return;
    fprintf(f, "P6\n%u %u\n255\n", OUT_W, OUT_H);
    for (uint32_t j = 0; j < OUT_H; j++)
        fwrite(buf + (size_t)j * OUT_W * 4, OUT_W * 3, 1, f);
    fclose(f);
    printf("     wrote %s\n", path);
}

/* ------------------------------------------------------------------ */

int main(int argc, char **argv)
{
    const char *ppm_path = argc > 1 ? argv[1] : "c_smoke_out.ppm";

    printf("bwgpu C smoke test\n  version: %s\n", bwgpu_version());

    /* --- renderer creation ---------------------------------------- */
    bwgpu_renderer *r = bwgpu_renderer_new(NULL, OUT_W, OUT_H, 0);
    if (!r) {
        const char *e = bwgpu_last_error();
        if (e && strstr(e, "no wgpu adapter")) {
            printf("SKIP - no usable GPU on this machine (error: %s)\n", e);
            return 77;
        }
        printf("FAIL - renderer_new returned NULL%s%s\n", e ? ": " : "", e ? e : "");
        return 1;
    }
    printf("     renderer created (headless)\n");

    /* --- mpv-style configuration ------------------------------------ */
    /* mpv master hands OSD bitmaps over premultiplied. */
    check(bwgpu_set_osd_premul(r, 1) == 0, "set OSD premultiplied convention");

    /* --- resize ---------------------------------------------------- */
    check(bwgpu_renderer_resize(r, OUT_W, OUT_H) == 0, "resize to 128x72");
    check(bwgpu_renderer_resize(r, OUT_W, OUT_H) == 0, "resize to same size is fine");

    /* --- video upload ---------------------------------------------- */
    static unsigned char y[VID_W * VID_H], u[VID_W * VID_H / 4], v[VID_W * VID_H / 4];
    make_video(y, u, v);

    bwgpu_frame fr;
    memset(&fr, 0, sizeof(fr));
    fr.fmt    = BWGPU_FMT_I420;
    fr.width  = VID_W;
    fr.height = VID_H;
    fr.y.data = y; fr.y.size = sizeof(y);
    fr.u.data = u; fr.u.size = sizeof(u);
    fr.v.data = v; fr.v.size = sizeof(v);
    check(bwgpu_set_video(r, &fr) == 0, "upload 64x64 I420 frame");

    /* --- colorspace + filter --------------------------------------- */
    check(bwgpu_set_colorspace(r, BWGPU_MATRIX_BT709, BWGPU_RANGE_LIMITED) == 0,
          "set BT.709 limited");
    check(bwgpu_set_scale(r, BWGPU_FILTER_CATMULL_ROM) == 0, "set Catmull-Rom");
    check(bwgpu_set_viewport(r, 0, 0, OUT_W, OUT_H) == 0, "set full-surface viewport");

    /* --- OSD -------------------------------------------------------- */
    static unsigned char osd[OSD_W * OSD_H * 4];
    make_osd(osd);
    bwgpu_bitmap bmp;
    memset(&bmp, 0, sizeof(bmp));
    bmp.data = osd; bmp.size = sizeof(osd);
    bmp.width = OSD_W; bmp.height = OSD_H;
    bmp.x = 8; bmp.y = 8;
    check(bwgpu_add_osd(r, &bmp) == 0, "add 16x16 OSD bitmap");
    check(bwgpu_clear_osd(r) == 0, "clear OSD");
    check(bwgpu_add_osd(r, &bmp) == 0, "re-add OSD bitmap");

    /* --- render + readback ------------------------------------------ */
    check(bwgpu_render_frame(r) == 0, "render_frame");

    static unsigned char out[OUT_W * OUT_H * 4];
    memset(out, 0xAA, sizeof(out));
    check(bwgpu_render_offscreen(r, out, sizeof(out), 0) == 0, "render_offscreen");

    check(!all_zero(out, sizeof(out)), "output is not all black");
    long spread = luma_spread(out);
    check(spread > 40, "gradient visible (luma spread > 40)");
    printf("     luma spread: %ld\n", spread);

    /* Red OSD with alpha ramp sits at (8,8)..(24,24). Its right edge
     * (alpha ~255) must show R dominating B; far from it, R must not
     * dominate B on the blue-ish gradient top rows (v=200 => B high). */
    int osd_ok = px(out, 20, 20, 0) > px(out, 20, 20, 2) + 40;
    int far_ok = px(out, OUT_W - 2, 2, 2) > px(out, OUT_W - 2, 2, 0);
    check(osd_ok, "OSD red wins in OSD region (R > B+40 at 20,20)");
    check(far_ok, "video blue wins away from OSD (B > R at 126,2)");

    /* --- error paths ------------------------------------------------ */
    check(bwgpu_set_video(r, NULL) < 0 && bwgpu_last_error() != NULL,
          "set_video(NULL) rejected with last_error");
    check(bwgpu_render_frame(NULL) < 0 && bwgpu_last_error() != NULL,
          "render_frame(NULL) rejected with last_error");

    bwgpu_frame bad = fr;
    bad.fmt = 999;
    check(bwgpu_set_video(r, &bad) < 0 && bwgpu_last_error() != NULL,
          "unknown pixel format rejected with last_error");

    check(bwgpu_renderer_resize(r, 0, 0) < 0, "zero-size resize rejected");

    /* --- teardown ---------------------------------------------------- */
    bwgpu_renderer_free(NULL);          /* documented no-op */
    bwgpu_renderer_free(r);
    printf("     renderer freed\n");

    write_ppm(ppm_path, out);

    printf("%s: %d checks, %d failures\n",
           failures ? "FAILED" : "PASSED", checks, failures);
    return failures ? 1 : 0;
}
