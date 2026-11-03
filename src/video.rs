//! Video pass: pixel formats, texture upload, per-frame state.
//!
//! Frames arrive as FFmpeg-style 4:2:0 planes (8- or 10-bit, planar or
//! interleaved chroma) and land in uint textures verbatim — no CPU-side
//! conversion, no copy beyond the GPU upload. Bit depth and P010-style
//! shifts are normalized in-shader, which keeps zero-copy import (M2)
//! trivial: the same textures a decoder would produce are the ones we bind.

use wgpu::{BindGroup, BindGroupLayout, Buffer, Texture, TextureView};

use crate::params::Params;

/// Pixel formats accepted for upload. 4:2:0 only — that is what decoders
/// hand over; exotic subsampling belongs in a conversion filter upstream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum PixFmt {
    /// Planar 8-bit 4:2:0 (FFmpeg `yuv420p`).
    I420 = 0,
    /// 2-plane 8-bit: luma + interleaved chroma (FFmpeg `nv12`).
    Nv12 = 1,
    /// Planar 10-bit 4:2:0, 16-bit words, LSB-aligned (FFmpeg `yuv420p10le`).
    Yuv420P10 = 2,
    /// 2-plane 10-bit, 16-bit words, MSB-aligned (FFmpeg `p010le`).
    P010 = 3,
}

impl PixFmt {
    pub fn from_i32(v: i32) -> Option<Self> {
        Some(match v {
            0 => PixFmt::I420,
            1 => PixFmt::Nv12,
            2 => PixFmt::Yuv420P10,
            3 => PixFmt::P010,
            _ => return None,
        })
    }

    /// (luma texture format, chroma texture format)
    pub fn tex_formats(self) -> (wgpu::TextureFormat, wgpu::TextureFormat) {
        use wgpu::TextureFormat as F;
        match self {
            PixFmt::I420 => (F::R8Uint, F::R8Uint),
            PixFmt::Nv12 => (F::R8Uint, F::Rg8Uint),
            PixFmt::Yuv420P10 => (F::R16Uint, F::R16Uint),
            PixFmt::P010 => (F::R16Uint, F::Rg16Uint),
        }
    }

    /// (depth_y, depth_c, right-shift of raw samples)
    pub fn depth(self) -> (f32, f32, f32) {
        match self {
            PixFmt::I420 | PixFmt::Nv12 => (8.0, 8.0, 0.0),
            PixFmt::Yuv420P10 => (10.0, 10.0, 0.0),
            PixFmt::P010 => (10.0, 10.0, 6.0),
        }
    }

    /// 1.0 = chroma interleaved in one RG texture.
    pub fn interleaved(self) -> f32 {
        match self {
            PixFmt::Nv12 | PixFmt::P010 => 1.0,
            _ => 0.0,
        }
    }
}

/// Scaling filter applied by the blit shader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum ScaleFilter {
    Nearest = 0,
    Bilinear = 1,
    CatmullRom = 2,
}

impl ScaleFilter {
    pub fn from_i32(v: i32) -> Option<Self> {
        Some(match v {
            0 => ScaleFilter::Nearest,
            1 => ScaleFilter::Bilinear,
            2 => ScaleFilter::CatmullRom,
            _ => return None,
        })
    }
}

/// One image plane: bytes + row stride.
#[derive(Clone, Copy, Debug)]
pub struct Plane<'a> {
    pub data: &'a [u8],
    /// Stride in bytes; 0 means `width * bpp` (tightly packed).
    pub stride: usize,
}

impl<'a> Plane<'a> {
    pub fn empty() -> Self {
        Self {
            data: &[],
            stride: 0,
        }
    }
}

/// A video frame ready for upload.
#[derive(Clone, Copy, Debug)]
pub struct VideoFrame<'a> {
    pub fmt: PixFmt,
    pub width: u32,
    pub height: u32,
    /// [y, u, v] — for interleaved formats, `u` carries the RG plane and
    /// `v` must be empty.
    pub planes: [Plane<'a>; 3],
}

impl<'a> VideoFrame<'a> {
    /// Chroma dimensions for 4:2:0.
    pub fn chroma_size(&self) -> (u32, u32) {
        (self.width.div_ceil(2), self.height.div_ceil(2))
    }

    fn validate(&self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 {
            return Err("frame has zero extent".into());
        }
        let interleave = self.fmt.interleaved() > 0.0;
        if interleave && !self.planes[2].data.is_empty() {
            return Err("interleaved format must not carry a v plane".into());
        }
        if !interleave && self.planes[2].data.is_empty() {
            return Err("planar format requires a v plane".into());
        }
        let (cw, ch) = self.chroma_size();
        let need = [(self.width, self.height), (cw, ch), (cw, ch)];
        // Bytes per sample: luma is 1 byte for 8-bit formats and 2 for
        // 10-bit; chroma additionally widens for interleaved (RG) storage.
        // A single per-format number here rejected tight-packed NV12 luma
        // (stride = width, not width * 2) at upload time.
        let luma_bpp: [usize; 4] = [1, 1, 2, 2];
        let chroma_bpp: [usize; 4] = [1, 2, 2, 4];
        let idx = match self.fmt {
            PixFmt::I420 => 0,
            PixFmt::Nv12 => 1,
            PixFmt::Yuv420P10 => 2,
            PixFmt::P010 => 3,
        };
        for (i, plane) in self.planes.iter().enumerate() {
            if plane.data.is_empty() && !(i == 2 && interleave) {
                return Err(format!("plane {i} is empty"));
            }
            if plane.data.is_empty() {
                continue;
            }
            let (w, h) = need[i];
            let bpp = if i == 0 {
                luma_bpp[idx]
            } else {
                chroma_bpp[idx]
            };
            let min_stride = w as usize * bpp;
            let stride = if plane.stride == 0 {
                min_stride
            } else {
                plane.stride
            };
            let need_bytes = stride * (h as usize - 1) + min_stride;
            if plane.data.len() < need_bytes {
                return Err(format!(
                    "plane {i}: {} bytes < required {need_bytes} ({}x{}, stride {stride})",
                    plane.data.len(),
                    w,
                    h
                ));
            }
        }
        Ok(())
    }
}

/// Current video textures + uniforms for the blit pipeline.
pub struct VideoPass {
    pub bgl: BindGroupLayout,
    pub params_buf: Buffer,
    pub params: Params,
    /// Pooled textures keyed by (slot, fmt, w, h) to avoid per-frame
    /// reallocation. Slots: 0 = luma, 1 = chroma, 2 = V plane (planar
    /// formats only — interleaved formats reuse slot 1 for both bindings).
    pub(crate) pool: Vec<(usize, PixFmt, u32, u32, Texture)>,
    pub views: [Option<TextureView>; 2],
    pub bind: Option<BindGroup>,
}

impl VideoPass {
    /// Build a 1x1 black uint bind group so the video pass is drawable
    /// before any frame has arrived (`--force-window --idle`: black + OSD).
    fn dummy_bind(device: &wgpu::Device, bgl: &BindGroupLayout, params_buf: &Buffer) -> BindGroup {
        let make = |label: &str, format: wgpu::TextureFormat| {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            tex.create_view(&wgpu::TextureViewDescriptor::default())
        };
        let y = make("bwgpu-dummy-y", wgpu::TextureFormat::R8Uint);
        let c = make("bwgpu-dummy-c", wgpu::TextureFormat::R8Uint);
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bwgpu-video-dummy"),
            layout: bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&y),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&c),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&c),
                },
            ],
        })
    }

    pub fn new(device: &wgpu::Device) -> Self {
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bwgpu-video-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                uint_tex(1),
                uint_tex(2),
                uint_tex(3),
            ],
        });
        let params_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bwgpu-video-params"),
            size: crate::params::PARAMS_SIZE as u64,
            usage: wgpu::BufferUsages::UNIFORM
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let bind = Self::dummy_bind(device, &bgl, &params_buf);
        Self {
            bgl,
            params_buf,
            params: Params::new(1, 1),
            pool: Vec::new(),
            views: [None, None],
            bind: Some(bind),
        }
    }

    fn texture(
        &mut self,
        device: &wgpu::Device,
        slot: usize,
        fmt: PixFmt,
        w: u32,
        h: u32,
    ) -> Texture {
        if let Some(pos) = self
            .pool
            .iter()
            .position(|&(s, f, tw, th, _)| s == slot && f == fmt && tw == w && th == h)
        {
            return self.pool.swap_remove(pos).4;
        }
        // Slot 0 is the luma plane; slots 1/2 are chroma and must use the
        // CHROMA format (RG for interleaved NV12/P010). Creating chroma
        // textures with the luma format silently dropped the V channel —
        // every NV12/P010 frame rendered with V = 0.
        let (tf_l, tf_c) = fmt.tex_formats();
        let tf = if slot == 0 { tf_l } else { tf_c };
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("bwgpu-video-slot{slot}")),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: tf,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    /// Upload a frame, update uniforms, rebuild the bind group.
    pub fn upload(
        &mut self,
        gpu: &crate::device::Gpu,
        frame: &VideoFrame,
        filter: ScaleFilter,
        dither: f32,
        parity: u32,
    ) -> crate::device::Result<()> {
        frame.validate().map_err(crate::device::Error::Upload)?;

        let dev = &gpu.device;
        let q = &gpu.queue;
        let (tf_y, tf_c) = frame.fmt.tex_formats();
        let (cw, ch) = frame.chroma_size();

        let tex_y = self.texture(dev, 0, frame.fmt, frame.width, frame.height);
        let tex_u = self.texture(dev, 1, frame.fmt, cw, ch);

        // Luma
        upload_plane(
            q,
            &tex_y,
            tf_y,
            frame.width,
            frame.height,
            frame.planes[0].data,
            frame.planes[0].stride,
        );
        // Chroma (U, or the RG plane for interleaved formats)
        upload_plane(
            q,
            &tex_u,
            tf_c,
            cw,
            ch,
            frame.planes[1].data,
            frame.planes[1].stride,
        );

        // Planar formats carry a real V plane that must land in its own
        // texture; binding the U view to the V slot (the pre-0.1.2
        // behavior) silently converted every planar frame with V = U.
        let interleaved = frame.fmt.interleaved() > 0.0;
        let (tex_v, view_v) = if interleaved {
            // The shader's interleaved branch reads U/V from tex_u .r/.g;
            // the V binding is never fetched, so alias it to the chroma
            // view (bindings must still be valid).
            let v = tex_u.create_view(&wgpu::TextureViewDescriptor::default());
            (None, v)
        } else {
            let tex = self.texture(dev, 2, frame.fmt, cw, ch);
            upload_plane(
                q,
                &tex,
                tf_c,
                cw,
                ch,
                frame.planes[2].data,
                frame.planes[2].stride,
            );
            let v = tex.create_view(&wgpu::TextureViewDescriptor::default());
            (Some(tex), v)
        };

        let view_y = tex_y.create_view(&wgpu::TextureViewDescriptor::default());
        let view_u = tex_u.create_view(&wgpu::TextureViewDescriptor::default());
        // Drop last frame's views; the textures themselves are pooled.
        self.views = [Some(view_y), Some(view_u.clone())];

        // Uniforms
        self.params.src = [
            frame.width as f32,
            frame.height as f32,
            cw as f32,
            ch as f32,
        ];
        let (dy, dc, shift) = frame.fmt.depth();
        self.params.misc = [dy, dc, shift, filter as i32 as f32];
        self.params.mode = [frame.fmt.interleaved(), dither, parity as f32, 0.0];
        q.write_buffer(&self.params_buf, 0, bytemuck::bytes_of(&self.params));

        // Bind group: for interleaved formats the same chroma view goes to
        // both u and v slots (the shader reads .r/.g from the u slot only);
        // planar formats bind their own V texture.
        self.bind = Some(dev.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bwgpu-video-bind"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(self.views[0].as_ref().unwrap()),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&view_u),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&view_v),
                },
            ],
        }));

        // Pool: hold this frame's textures for reuse next frame.
        self.pool
            .push((0, frame.fmt, frame.width, frame.height, tex_y));
        self.pool.push((1, frame.fmt, cw, ch, tex_u));
        if let Some(tex_v) = tex_v {
            self.pool.push((2, frame.fmt, cw, ch, tex_v));
        }
        while self.pool.len() > 12 {
            self.pool.remove(0);
        }
        Ok(())
    }
}

fn uint_tex(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Uint,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn upload_plane(
    q: &wgpu::Queue,
    tex: &Texture,
    format: wgpu::TextureFormat,
    w: u32,
    h: u32,
    data: &[u8],
    stride: usize,
) {
    if data.is_empty() {
        return;
    }
    let bpp = format
        .block_copy_size(Some(wgpu::TextureAspect::All))
        .unwrap_or(1) as usize;
    let stride = if stride == 0 {
        w as usize * bpp
    } else {
        stride
    };
    q.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(stride as u32),
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
}
