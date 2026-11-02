//! OSD pass: straight-alpha RGBA8 bitmaps blended over the video.
//!
//! The libass convention is straight alpha, so bitmaps upload verbatim into
//! `RGBA8Unorm` textures and the shader premultiplies; the pipeline blends
//! with One / OneMinusSrcAlpha. One small uniform block per bitmap — OSD
//! item counts are tiny (subtitles, OSC), so per-item draw calls are fine
//! and keep the code honest.

use wgpu::{Buffer, Texture, TextureView};

/// One blended OSD bitmap.
struct OsdItem {
    _texture: Texture,
    view: TextureView,
    uniform: Buffer,
}

/// Uniform block for one OSD quad — matches `shaders/osd_blend.wgsl::Inst`.
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct OsdInst {
    /// x, y, w, h in target pixels.
    dst: [f32; 4],
    /// u0, v0, u1, v1.
    uv: [f32; 4],
    /// target width, height, 0, 0.
    tgt: [f32; 4],
}

pub const OSD_INST_SIZE: usize = std::mem::size_of::<OsdInst>(); // 48

pub struct OsdPass {
    pub bgl: wgpu::BindGroupLayout,
    pub sampler: wgpu::Sampler,
    items: Vec<OsdItem>,
}

impl OsdPass {
    pub fn new(device: &wgpu::Device) -> Self {
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bwgpu-osd-bgl"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("bwgpu-osd-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        Self {
            bgl,
            sampler,
            items: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Add an RGBA8 bitmap at pixel position (x, y).
    ///
    /// `premul` selects the input alpha convention: `false` = straight alpha
    /// (libass-style, uploaded verbatim), `true` = premultiplied alpha (mpv
    /// master's SUBBITMAP_BGRA), which is un-premultiplied here so the
    /// shader keeps exactly one premultiply step. OSD bitmaps are small, so
    /// the CPU pass is negligible.
    pub fn add(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bmp: &crate::Bitmap,
        target: (u32, u32),
        premul: bool,
    ) -> Result<(), String> {
        let (width, height) = (bmp.width, bmp.height);
        let (x, y) = (bmp.x, bmp.y);
        if width == 0 || height == 0 {
            return Ok(()); // nothing to draw
        }
        let stride = width as usize * 4;
        let need = stride * height as usize;
        if bmp.data.len() < need {
            return Err(format!(
                "osd bitmap: {} bytes < required {need} ({width}x{height})",
                bmp.data.len()
            ));
        }
        // Un-premultiply into an owned staging buffer when the source is
        // premultiplied. Round-to-nearest, clamped; the error is at most
        // half a code per channel and invisible on glyph edges. Fully
        // transparent pixels are forced to black — that is the premultiplied
        // invariant, and garbage RGB at a=0 would flash as a halo.
        let converted;
        let data: &[u8] = if premul {
            let mut buf = bmp.data[..need].to_vec();
            for px in buf.chunks_exact_mut(4) {
                let a = px[3] as u32;
                if a == 0 {
                    px[0] = 0;
                    px[1] = 0;
                    px[2] = 0;
                } else if a < 255 {
                    for c in &mut px[..3] {
                        *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
                    }
                }
            }
            converted = buf;
            &converted
        } else {
            &bmp.data[..need]
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bwgpu-osd-bitmap"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
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
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let inst = OsdInst {
            dst: [x as f32, y as f32, width as f32, height as f32],
            uv: [0.0, 0.0, 1.0, 1.0],
            tgt: [target.0 as f32, target.1 as f32, 0.0, 0.0],
        };
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bwgpu-osd-inst"),
            size: OSD_INST_SIZE as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform, 0, bytemuck::bytes_of(&inst));
        self.items.push(OsdItem {
            _texture: texture,
            view,
            uniform,
        });
        Ok(())
    }

    pub fn items(&self) -> impl Iterator<Item = (&TextureView, &Buffer)> {
        self.items.iter().map(|i| (&i.view, &i.uniform))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn premul_to_straight_is_lossless_at_extremes() {
        // a = 0 -> black; a = 255 -> identity; mid-alpha -> ~x/a.
        let src = [128u8, 64, 32, 0, 255, 255, 255, 255, 128, 128, 128, 128];
        let bmp = crate::Bitmap {
            data: &src,
            width: 3,
            height: 1,
            x: 0,
            y: 0,
        };
        let out = {
            // Exercise the same conversion add() performs, without a GPU.
            let mut buf = bmp.data[..12].to_vec();
            for px in buf.chunks_exact_mut(4) {
                let a = px[3] as u32;
                if a == 0 {
                    px[0] = 0;
                    px[1] = 0;
                    px[2] = 0;
                } else if a < 255 {
                    for c in &mut px[..3] {
                        *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
                    }
                }
            }
            buf
        };
        assert_eq!(&out[0..4], &[0, 0, 0, 0]);
        assert_eq!(&out[4..8], &[255, 255, 255, 255]);
        assert_eq!(&out[8..12], &[255, 255, 255, 128]); // 128*255/128
    }
}
