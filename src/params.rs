//! YUV→RGB color conversion and the GPU uniform block.
//!
//! The matrix is derived by *evaluating the conversion at basis points*
//! rather than by hand-transcribing formulas a second time: the same
//! conversion function is probed at (1,0,0), (0,1,0), (0,0,1) and (0,0,0),
//! which yields the affine coefficients exactly. All range/offset handling
//! lives inside that one function (`conv_f`), so the GPU shader, the uniform
//! packer and the CPU reference in the roundtrip test consume literally the
//! same numbers — there is no second copy of the math to drift.

use bytemuck::{Pod, Zeroable};

/// YUV colorimetry families supported by the renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum YuvMatrix {
    /// ITU-R BT.601 (SD).
    Bt601 = 0,
    /// ITU-R BT.709 (HD).
    Bt709 = 1,
    /// ITU-R BT.2020 (UHD).
    Bt2020 = 2,
}

/// Luma/chroma signal range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum YuvRange {
    /// Studio swing: Y in [16,235], chroma in [16,240] (8-bit terms).
    Limited = 0,
    /// Full swing: Y and chroma in [0,255] (8-bit terms).
    Full = 1,
}

impl YuvMatrix {
    pub fn from_i32(v: i32) -> Option<Self> {
        Some(match v {
            0 => YuvMatrix::Bt601,
            1 => YuvMatrix::Bt709,
            2 => YuvMatrix::Bt2020,
            _ => return None,
        })
    }
}

impl YuvRange {
    pub fn from_i32(v: i32) -> Option<Self> {
        Some(match v {
            0 => YuvRange::Limited,
            1 => YuvRange::Full,
            _ => return None,
        })
    }
}

/// Affine conversion: `rgb = A * yuv_norm + b`.
///
/// Input domain: `yuv_norm` is exactly what the shader derives from raw
/// samples — luma and chroma both divided by `2^depth - 1`, so everything
/// spans [0,1] with chroma centered wherever the range convention puts it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coeffs {
    /// Row-major 3x3 matrix A (row = RGB channel, column = YUV channel).
    pub m: [[f32; 3]; 3],
    /// Bias b added after the matrix multiply.
    pub b: [f32; 3],
}

/// The complete YUV→RGB conversion on shader-normalized inputs.
///
/// This is the single source of truth for color math in the project.
fn conv_f(mtx: YuvMatrix, range: YuvRange) -> impl Fn(f32, f32, f32) -> [f32; 3] {
    let (ys, cs) = match range {
        YuvRange::Limited => (255.0 / 219.0, 255.0 / 224.0),
        YuvRange::Full => (1.0, 1.0),
    };
    let (y_off, c_off) = match range {
        // Limited: black at code 16, chroma centered on code 128.
        YuvRange::Limited => (16.0 / 255.0, 128.0 / 255.0),
        // Full: black at 0, chroma centered between codes (JPEG convention).
        YuvRange::Full => (0.0, 0.5),
    };
    move |yn, un, vn| {
        let y = (yn - y_off) * ys;
        let (u, v) = ((un - c_off) * cs, (vn - c_off) * cs);
        match mtx {
            YuvMatrix::Bt601 => [
                y + 1.402 * v,
                y - 0.344136 * u - 0.714136 * v,
                y + 1.772 * u,
            ],
            YuvMatrix::Bt709 => [y + 1.5748 * v, y - 0.1873 * u - 0.4681 * v, y + 1.8556 * u],
            YuvMatrix::Bt2020 => [
                y + 1.4746 * v,
                y - 0.16455 * u - 0.57135 * v,
                y + 1.8814 * u,
            ],
        }
    }
}

/// Derive the affine coefficients by probing `conv_f` at basis points.
pub fn coeffs(mtx: YuvMatrix, range: YuvRange) -> Coeffs {
    let f = conv_f(mtx, range);
    let zero = f(0.0, 0.0, 0.0);
    let col = |dy, du, dv| {
        let a = f(dy, du, dv);
        [a[0] - zero[0], a[1] - zero[1], a[2] - zero[2]]
    };
    let cy = col(1.0, 0.0, 0.0);
    let cu = col(0.0, 1.0, 0.0);
    let cv = col(0.0, 0.0, 1.0);
    Coeffs {
        m: [
            [cy[0], cu[0], cv[0]],
            [cy[1], cu[1], cv[1]],
            [cy[2], cu[2], cv[2]],
        ],
        b: zero,
    }
}

/// Everything the video blit shader needs, packed for `uniform` storage.
///
/// Layout contract (enforced by `tests/uniform_layout.rs`): every field is a
/// `vec4<f32>` (16-byte stride), total size 128 bytes. The WGSL struct in
/// `shaders/video_blit.wgsl` must match field-for-field.
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
#[repr(C)]
pub struct Params {
    /// Rows 0/1/2 of A where `rgb = A * yuv_norm + bias`.
    pub mat0: [f32; 4],
    pub mat1: [f32; 4],
    pub mat2: [f32; 4],
    /// rgb bias (xyz, w unused).
    pub bias: [f32; 4],
    /// Luma width/height, chroma width/height (texels).
    pub src: [f32; 4],
    /// depth_y (bits), depth_c (bits), shift_bits (right-shift of raw
    /// sample, for P010-style formats), filter: 0 = nearest, 1 = bilinear,
    /// 2 = Catmull-Rom.
    pub misc: [f32; 4],
    /// chroma_interleaved (1 = U/V live in one RG texture), dither amplitude
    /// (LSB), frame parity (Bayer phase), 0.
    pub mode: [f32; 4],
    /// Video destination rect as fractions of the target surface:
    /// [x, y, w, h] in [0,1]. Pixels outside render black. This is how the
    /// host (mpv) expresses aspect ratio / panscan without the renderer
    /// knowing anything about window geometry. Default: full surface.
    pub rect: [f32; 4],
}

pub const PARAMS_SIZE: usize = std::mem::size_of::<Params>(); // must be 128

impl Params {
    /// Neutral defaults: 1:1 8-bit bilinear blit.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            mat0: [1.0, 0.0, 0.0, 0.0],
            mat1: [0.0, 1.0, 0.0, 0.0],
            mat2: [0.0, 0.0, 1.0, 0.0],
            bias: [0.0; 4],
            src: [
                width as f32,
                height as f32,
                (width / 2) as f32,
                (height / 2) as f32,
            ],
            misc: [8.0, 8.0, 0.0, 1.0],
            mode: [0.0, 0.0, 0.0, 0.0],
            rect: [0.0, 0.0, 1.0, 1.0],
        }
    }

    /// Install a colorspace: the shader-side math is a flat
    /// `A @ yuv_norm + b`, so this is a straight copy of the basis-evaluated
    /// coefficients.
    pub fn set_colorspace(&mut self, mtx: YuvMatrix, range: YuvRange) {
        let c = coeffs(mtx, range);
        self.mat0 = [c.m[0][0], c.m[0][1], c.m[0][2], 0.0];
        self.mat1 = [c.m[1][0], c.m[1][1], c.m[1][2], 0.0];
        self.mat2 = [c.m[2][0], c.m[2][1], c.m[2][2], 0.0];
        self.bias = [c.b[0], c.b[1], c.b[2], 0.0];
    }

    /// Convert one shader-normalized YUV texel with this parameter block
    /// (CPU reference twin of the shader fragment path).
    pub fn convert_point(&self, y: f32, u: f32, v: f32) -> [f32; 3] {
        [
            self.mat0[0] * y + self.mat0[1] * u + self.mat0[2] * v + self.bias[0],
            self.mat1[0] * y + self.mat1[1] * u + self.mat1[2] * v + self.bias[1],
            self.mat2[0] * y + self.mat2[1] * u + self.mat2[2] * v + self.bias[2],
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_layout_is_packed() {
        assert_eq!(PARAMS_SIZE, 128);
        assert_eq!(std::mem::offset_of!(Params, bias), 48);
        assert_eq!(std::mem::offset_of!(Params, misc), 80);
        assert_eq!(std::mem::offset_of!(Params, mode), 96);
        assert_eq!(std::mem::offset_of!(Params, rect), 112);
    }

    #[test]
    fn basis_eval_matches_known_constants() {
        // BT.601 limited: luma gain 255/219, R chroma gain 1.402 * 255/224.
        let mut p = Params::new(16, 16);
        p.set_colorspace(YuvMatrix::Bt601, YuvRange::Limited);
        assert!((p.mat0[0] - 1.164384).abs() < 1e-5);
        assert!((p.mat0[2] - 1.596027).abs() < 1e-5);

        // Full-range BT.709 luma gain is exactly 1.
        let mut p = Params::new(16, 16);
        p.set_colorspace(YuvMatrix::Bt709, YuvRange::Full);
        assert!((p.mat0[0] - 1.0).abs() < 1e-6);

        // Studio black converts to ~(0,0,0). Limited: codes (16,128,128) sit
        // exactly on the centers. Full: black luma is code 0 and chroma code
        // 128 sits half a code above the 127.5 center (the classic 1-LSB
        // siting nuance), hence the half-LSB-scale tolerance.
        {
            let mut p = Params::new(16, 16);
            p.set_colorspace(YuvMatrix::Bt709, YuvRange::Limited);
            let rgb = p.convert_point(16.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0);
            for ch in rgb {
                assert!(ch.abs() < 1e-4, "black leak {rgb:?} (limited)");
            }
        }
        {
            let mut p = Params::new(16, 16);
            p.set_colorspace(YuvMatrix::Bt709, YuvRange::Full);
            let rgb = p.convert_point(0.0, 128.0 / 255.0, 128.0 / 255.0);
            for ch in rgb {
                assert!(ch.abs() < 1.5 / 255.0, "black leak {rgb:?} (full)");
            }
        }
    }
}
