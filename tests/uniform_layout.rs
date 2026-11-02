//! Layout drift guards: the Rust uniform structs and the WGSL shaders must
//! stay in sync. These tests need no GPU and always run.

use bwgpu::osd::OSD_INST_SIZE;
use bwgpu::params::{Params, PARAMS_SIZE};

/// Whitespace-insensitive contains, so formatting never breaks the guards.
fn norm_contains(haystack: &str, needle: &str) -> bool {
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    norm(haystack).contains(&norm(needle))
}

#[test]
fn params_layout_matches_wgsl() {
    // 8 x vec4<f32>.
    assert_eq!(PARAMS_SIZE, 128);
    assert_eq!(std::mem::offset_of!(Params, mat0), 0);
    assert_eq!(std::mem::offset_of!(Params, mat1), 16);
    assert_eq!(std::mem::offset_of!(Params, mat2), 32);
    assert_eq!(std::mem::offset_of!(Params, bias), 48);
    assert_eq!(std::mem::offset_of!(Params, src), 64);
    assert_eq!(std::mem::offset_of!(Params, misc), 80);
    assert_eq!(std::mem::offset_of!(Params, mode), 96);
    assert_eq!(std::mem::offset_of!(Params, rect), 112);

    // The WGSL struct must declare the same fields in the same order.
    let wgsl = include_str!("../shaders/video_blit.wgsl");
    let start = wgsl
        .find("struct Params {")
        .expect("video_blit.wgsl must declare `struct Params {`");
    let end = wgsl[start..]
        .find("};")
        .map(|i| start + i)
        .expect("struct Params must be closed");
    let body = &wgsl[start..end];
    let fields: Vec<&str> = body
        .lines()
        .skip(1) // "struct Params {" itself
        .filter_map(|l| l.trim().split(':').next())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    assert_eq!(
        fields,
        ["mat0", "mat1", "mat2", "bias", "src", "misc", "mode", "rect"],
        "video_blit.wgsl Params field list/order drifted"
    );
    for field in fields {
        assert!(
            norm_contains(body, &format!("{field}: vec4<f32>")),
            "video_blit.wgsl Params `{field}` must be vec4<f32>"
        );
    }
}

#[test]
fn osd_inst_layout_matches_wgsl() {
    assert_eq!(OSD_INST_SIZE, 48);
    let wgsl = include_str!("../shaders/osd_blend.wgsl");
    let start = wgsl
        .find("struct Inst {")
        .expect("osd_blend.wgsl must declare `struct Inst {`");
    let end = wgsl[start..]
        .find("};")
        .map(|i| start + i)
        .expect("struct Inst must be closed");
    let body = &wgsl[start..end];
    let fields: Vec<&str> = body
        .lines()
        .skip(1) // "struct Inst {" itself
        .filter_map(|l| l.trim().split(':').next())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    assert_eq!(
        fields,
        ["dst", "uv", "tgt"],
        "osd_blend.wgsl Inst field list/order drifted"
    );
}

#[test]
fn shader_bindings_are_stable() {
    // The Rust bind group layouts hard-code binding numbers; guard them.
    let video = include_str!("../shaders/video_blit.wgsl");
    for what in [
        "@binding(0) var<uniform> p",
        "@binding(1) var tex_y",
        "@binding(2) var tex_u",
        "@binding(3) var tex_v",
    ] {
        assert!(
            norm_contains(video, what),
            "video_blit.wgsl binding drifted: {what}"
        );
    }
    let osd = include_str!("../shaders/osd_blend.wgsl");
    for what in [
        "@binding(0) var<uniform> inst",
        "@binding(1) var tex",
        "@binding(2) var samp",
    ] {
        assert!(
            norm_contains(osd, what),
            "osd_blend.wgsl binding drifted: {what}"
        );
    }
}

#[test]
fn c_header_matches_rust_enums() {
    // Single source of truth for the C ABI: the header. Guard the numbers.
    let h = include_str!("../include/bwgpu.h");
    for (name, val) in [
        ("BWGPU_PLATFORM_WIN32", "0"),
        ("BWGPU_PLATFORM_XLIB", "1"),
        ("BWGPU_PLATFORM_WAYLAND", "2"),
        ("BWGPU_PLATFORM_COCOA", "3"),
        ("BWGPU_FMT_I420", "0"),
        ("BWGPU_FMT_NV12", "1"),
        ("BWGPU_FMT_YUV420P10", "2"),
        ("BWGPU_FMT_P010", "3"),
        ("BWGPU_MATRIX_BT601", "0"),
        ("BWGPU_MATRIX_BT709", "1"),
        ("BWGPU_MATRIX_BT2020", "2"),
        ("BWGPU_RANGE_LIMITED", "0"),
        ("BWGPU_RANGE_FULL", "1"),
        ("BWGPU_FILTER_NEAREST", "0"),
        ("BWGPU_FILTER_BILINEAR", "1"),
        ("BWGPU_FILTER_CATMULL_ROM", "2"),
    ] {
        assert!(
            norm_contains(h, &format!("{name} = {val}")),
            "include/bwgpu.h: expected `{name} = {val}`"
        );
    }
}

#[test]
fn c_header_declares_abi_surface() {
    // Every function vo_wgpu.c links against must exist in the header.
    let h = include_str!("../include/bwgpu.h");
    for what in [
        "bwgpu_version(void)",
        "bwgpu_last_error(void)",
        "bwgpu_renderer_new(const bwgpu_window *win",
        "bwgpu_renderer_free(bwgpu_renderer *r)",
        "bwgpu_renderer_resize(bwgpu_renderer *r",
        "bwgpu_set_video(bwgpu_renderer *r",
        "bwgpu_set_colorspace(bwgpu_renderer *r",
        "bwgpu_set_scale(bwgpu_renderer *r",
        "bwgpu_set_viewport(bwgpu_renderer *r",
        "bwgpu_set_osd_premul(bwgpu_renderer *r",
        "bwgpu_clear_osd(bwgpu_renderer *r)",
        "bwgpu_add_osd(bwgpu_renderer *r",
        "bwgpu_render_frame(bwgpu_renderer *r)",
        "bwgpu_render_offscreen(bwgpu_renderer *r",
    ] {
        assert!(
            norm_contains(h, what),
            "include/bwgpu.h: missing ABI entry `{what}`"
        );
    }
}
