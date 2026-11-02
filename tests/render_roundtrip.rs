//! End-to-end roundtrip: a synthetic YUV frame goes through the GPU
//! pipeline and is compared against a CPU reference built from the *same*
//! color coefficients. Skips gracefully when no wgpu adapter is available
//! (headless CI containers); runs for real wherever there is a device.

use bwgpu::{PixFmt, Plane, Renderer, VideoFrame, YuvMatrix, YuvRange};

/// Build a 64x64 I420 test frame: horizontal luma ramp, vertical chroma
/// ramp. Deterministic, no RNG.
fn test_frame(w: u32, h: u32) -> (Vec<u8>, VideoFrame<'static>) {
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);
    let mut y = vec![0u8; (w * h) as usize];
    let mut u = vec![0u8; (cw * ch) as usize];
    let mut v = vec![0u8; (cw * ch) as usize];
    for j in 0..h {
        for i in 0..w {
            y[(j * w + i) as usize] = ((i * 255) / (w - 1).max(1)) as u8;
        }
    }
    for j in 0..ch {
        for i in 0..cw {
            u[(j * cw + i) as usize] = ((i * 255) / (cw - 1).max(1)) as u8;
            v[(j * cw + i) as usize] = ((j * 255) / (ch - 1).max(1)) as u8;
        }
    }
    // Leak the buffers so VideoFrame<'static> can reference them; the test
    // process is short-lived.
    let y: &'static [u8] = Box::leak(y.into_boxed_slice());
    let u: &'static [u8] = Box::leak(u.into_boxed_slice());
    let v: &'static [u8] = Box::leak(v.into_boxed_slice());
    let frame = VideoFrame {
        fmt: PixFmt::I420,
        width: w,
        height: h,
        planes: [
            Plane { data: y, stride: 0 },
            Plane { data: u, stride: 0 },
            Plane { data: v, stride: 0 },
        ],
    };
    let owned = (y.to_vec(), u.to_vec(), v.to_vec());
    (owned.0, frame) // keep the first alive for symmetry; leaked anyway
}

/// CPU reference: replicate the shader's math in f64 — normalized loads,
/// bilinear sampling at texel centers, matrix conversion.
fn reference_rgb(
    params: &bwgpu::params::Params,
    y: &[u8],
    u: &[u8],
    v: &[u8],
    w: u32,
    h: u32,
) -> Vec<[f64; 3]> {
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);
    let fetch = |plane: &[u8], pw: u32, ph: u32, x: f64, y: f64| -> f64 {
        // Manual bilinear with clamped edges (shader twin).
        let base_x = x - 0.5;
        let base_y = y - 0.5;
        let fx = base_x.fract();
        let fy = base_y.fract();
        let cl = |v: f64, n: f64| v.clamp(0.0, n - 1.0);
        let i0 = cl(base_x.floor(), pw as f64) as usize;
        let j0 = cl(base_y.floor(), ph as f64) as usize;
        let i1 = cl(base_x.floor() + 1.0, pw as f64) as usize;
        let j1 = cl(base_y.floor() + 1.0, ph as f64) as usize;
        let s = |i: usize, j: usize| plane[j * pw as usize + i] as f64 / 255.0;
        let a = s(i0, j0) * (1.0 - fx) + s(i1, j0) * fx;
        let b = s(i0, j1) * (1.0 - fx) + s(i1, j1) * fx;
        a * (1.0 - fy) + b * fy
    };
    let mut out = Vec::with_capacity((w * h) as usize);
    for j in 0..h {
        for i in 0..w {
            let yv = fetch(y, w, h, i as f64 + 0.5, j as f64 + 0.5);
            // Co-sited chroma: c = (y_texel + 0.5) * 0.5 - 0.5
            let ctc_x = (i as f64 + 0.5) * (cw as f64 / w as f64);
            let ctc_y = (j as f64 + 0.5) * (ch as f64 / h as f64);
            let uv = fetch(u, cw, ch, ctc_x, ctc_y);
            let vv = fetch(v, cw, ch, ctc_x, ctc_y);
            let rgb = params.convert_point(yv as f32, uv as f32, vv as f32);
            out.push([rgb[0] as f64, rgb[1] as f64, rgb[2] as f64]);
        }
    }
    out
}

fn adapter_available() -> bool {
    // Probe: a 4x4 headless renderer. Failure means "no GPU here".
    Renderer::new(4, 4).is_ok()
}

#[test]
fn gpu_matches_cpu_reference() {
    let _ = env_logger::try_init();
    if !adapter_available() {
        eprintln!("skipping: no wgpu adapter in this environment (CI has one)");
        return;
    }

    let (w, h) = (64u32, 64u32);
    let (_keep, frame) = test_frame(w, h);

    let mut r = Renderer::new(w, h).expect("headless renderer");
    r.set_colorspace(YuvMatrix::Bt709, YuvRange::Limited);
    r.set_video(&frame).expect("upload");
    r.render_frame().expect("render");

    let mut out = vec![0u8; (w * h * 4) as usize];
    r.render_offscreen(&mut out, 0).expect("readback");

    // Reference with the same coefficients the GPU used.
    let mut params = bwgpu::params::Params::new(w, h);
    params.set_colorspace(YuvMatrix::Bt709, YuvRange::Limited);
    let (_keep2, frame2) = test_frame(w, h);
    let y = frame2.planes[0].data;
    let u = frame2.planes[1].data;
    let v = frame2.planes[2].data;
    let expected = reference_rgb(&params, y, u, v, w, h);

    let mut max_diff = 0u32;
    for (px, e) in out.chunks_exact(4).zip(&expected) {
        for c in 0..3 {
            let got = px[c] as f64;
            let diff = (got - e[c] * 255.0).abs();
            max_diff = max_diff.max(diff as u32);
        }
        assert_eq!(px[3], 255, "alpha must be opaque");
    }
    // f32 GPU vs f64 CPU + u8 rounding: 2 LSB is generous but honest.
    assert!(
        max_diff <= 2,
        "GPU output diverges from CPU reference by {max_diff} LSB"
    );
}

#[test]
fn osd_blends_over_video() {
    let _ = env_logger::try_init();
    if !adapter_available() {
        eprintln!("skipping: no wgpu adapter in this environment (CI has one)");
        return;
    }

    let (w, h) = (16u32, 16u32);
    let mut r = Renderer::new(w, h).expect("headless renderer");

    // Gray video (luma 128 => mid gray with 709 full? limited: ~0.5 signal).
    let y = vec![128u8; (w * h) as usize];
    let u = vec![128u8; ((w / 2) * (h / 2)) as usize];
    let v = vec![128u8; ((w / 2) * (h / 2)) as usize];
    let y: &'static [u8] = Box::leak(y.into_boxed_slice());
    let u: &'static [u8] = Box::leak(u.into_boxed_slice());
    let v: &'static [u8] = Box::leak(v.into_boxed_slice());
    let frame = VideoFrame {
        fmt: PixFmt::I420,
        width: w,
        height: h,
        planes: [
            Plane { data: y, stride: 0 },
            Plane { data: u, stride: 0 },
            Plane { data: v, stride: 0 },
        ],
    };
    r.set_video(&frame).expect("upload");

    // Fully opaque red OSD bitmap covering the left half.
    let bw = 8u32;
    let bmp = [255u8, 0, 0, 255].repeat((bw * h) as usize);
    let bmp_static: &'static [u8] = Box::leak(bmp.into_boxed_slice());
    r.add_osd(&bwgpu::Bitmap {
        data: bmp_static,
        width: bw,
        height: h,
        x: 0,
        y: 0,
    })
    .expect("add osd");
    r.render_frame().expect("render");

    let mut out = vec![0u8; (w * h * 4) as usize];
    r.render_offscreen(&mut out, 0).expect("readback");

    // Left half red, right half not red.
    let px = |x: u32, yy: u32| -> [u8; 4] {
        let o = ((yy * w + x) * 4) as usize;
        [out[o], out[o + 1], out[o + 2], out[o + 3]]
    };
    let left = px(2, 8);
    assert!(
        left[0] > 200 && left[1] < 60 && left[2] < 60,
        "left {left:?}"
    );
    let right = px(12, 8);
    assert!(
        right[0] < left[0] - 100,
        "right {right:?} should be much darker in R than left {left:?}"
    );
}
