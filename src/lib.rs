//! bwgpu — mpv, rebased on wgpu.
//!
//! One renderer, one dependency path: every pixel goes through wgpu
//! (DX12 on Windows, Vulkan on Linux, Metal on macOS — wgpu picks). There
//! are no OpenGL/Vulkan-C/D3D11 fallbacks, no per-platform render backends,
//! no Android media path. The C core (demux/decode/audio/subs) stays; only
//! the render side is replaced.
//!
//! Module map:
//! - [`params`]  — color math (single source of truth) + GPU uniform packing
//! - [`device`]  — adapter/device bootstrap
//! - [`surface`] — native window bridging + swapchain config
//! - [`video`]   — YUV upload + blit state
//! - [`osd`]     — subtitle/OSD bitmaps
//! - [`capi`]    — the stable C ABI mpv links against (see `include/bwgpu.h`)

pub mod capi;
pub mod device;
pub mod osd;
pub mod params;
pub mod surface;
pub mod video;

use std::collections::HashMap;

use device::{Error, Gpu, Result};
use surface::RawPair;
use wgpu::{BindGroup, RenderPipeline, TextureView};

pub use params::{YuvMatrix, YuvRange};
pub use video::{PixFmt, Plane, ScaleFilter, VideoFrame};

/// Crate version, surfaced through the C ABI for bug reports.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// One RGBA8 bitmap to blend over the video.
#[derive(Clone, Copy, Debug)]
pub struct Bitmap<'a> {
    /// Straight-alpha RGBA8, tightly packed, `width * 4 * height` bytes.
    pub data: &'a [u8],
    pub width: u32,
    pub height: u32,
    /// Destination position in target pixels.
    pub x: i32,
    pub y: i32,
}

/// Pipeline pair for one target format (surface formats differ per platform;
/// offscreen is always RGBA8Unorm).
#[derive(Clone)]
struct Pipes {
    video: RenderPipeline,
    osd: RenderPipeline,
}

/// The renderer. Create one per player window (or headless for screenshots
/// and tests). Not `Sync`: use it from the thread that owns the window.
pub struct Renderer {
    gpu: Gpu,
    /// `None` = headless (offscreen only).
    surface: Option<wgpu::Surface<'static>>,
    surface_config: Option<wgpu::SurfaceConfiguration>,
    pub width: u32,
    pub height: u32,
    video: video::VideoPass,
    osd: osd::OsdPass,
    offscreen: Option<(wgpu::Texture, TextureView)>,
    pipes: HashMap<wgpu::TextureFormat, Pipes>,
    colorspace: (YuvMatrix, YuvRange),
    filter: ScaleFilter,
    /// Video destination rect in target pixels (aspect/panscan from the
    /// host). `None` = full surface. Stored in pixels so a resize can
    /// re-derive the fraction form the shader consumes.
    video_rect: Option<(i32, i32, u32, u32)>,
    /// Input alpha convention for OSD bitmaps (false = straight, true =
    /// premultiplied as mpv master's SUBBITMAP_BGRA).
    osd_premul: bool,
    frame_count: u64,
}

impl Renderer {
    /// Headless renderer (offscreen rendering + readback only).
    pub fn new(width: u32, height: u32) -> Result<Self> {
        Self::create(None, width.max(1), height.max(1), true)
    }

    /// Renderer targeting a native window.
    pub fn for_window(win: &RawPair, width: u32, height: u32, vsync: bool) -> Result<Self> {
        let instance = Self::instance();
        let target = unsafe {
            wgpu::SurfaceTargetUnsafe::from_window(win)
                .map_err(|e| Error::Init(format!("window handle: {e}")))?
        };
        let surface = unsafe {
            instance
                .create_surface_unsafe(target)
                .map_err(|e| Error::Init(format!("create_surface: {e}")))?
        };
        Self::create(Some(surface), width.max(1), height.max(1), vsync)
    }

    fn instance() -> wgpu::Instance {
        wgpu::Instance::new(&wgpu::InstanceDescriptor::default())
    }

    fn create(
        surface: Option<wgpu::Surface<'static>>,
        width: u32,
        height: u32,
        vsync: bool,
    ) -> Result<Self> {
        let instance = Self::instance();
        let gpu = Gpu::new(&instance, surface.as_ref())?;

        let surface_config = surface.as_ref().map(|s| {
            let caps = s.get_capabilities(&gpu.adapter);
            let format = surface::pick_format(&caps);
            surface::make_config(format, width, height, vsync)
        });
        if let (Some(s), Some(cfg)) = (surface.as_ref(), &surface_config) {
            s.configure(&gpu.device, cfg);
        }

        let video = video::VideoPass::new(&gpu.device);
        let osd = osd::OsdPass::new(&gpu.device);

        let mut renderer = Self {
            gpu,
            surface,
            surface_config,
            width,
            height,
            video,
            osd,
            offscreen: None,
            pipes: HashMap::new(),
            colorspace: (YuvMatrix::Bt709, YuvRange::Limited),
            filter: ScaleFilter::Bilinear,
            video_rect: None,
            osd_premul: false,
            frame_count: 0,
        };
        // Pre-build pipelines for the known target formats so first-frame
        // hitches don't include shader compilation.
        if let Some(cfg) = &renderer.surface_config {
            renderer.ensure_pipes(cfg.format);
        }
        renderer.ensure_pipes(wgpu::TextureFormat::Rgba8Unorm);
        // Push the initial uniform block (safe idle defaults: bilinear,
        // 8-bit, full-surface rect) so every draw is well-defined.
        renderer.apply_rect();
        Ok(renderer)
    }

    /// Build (and cache) the pipeline pair for a target format. Pipelines
    /// are `Arc`-backed, so the clone handed to the draw path is cheap.
    fn ensure_pipes(&mut self, format: wgpu::TextureFormat) -> Pipes {
        if !self.pipes.contains_key(&format) {
            let video_shader = self.gpu.module(
                include_str!("../shaders/video_blit.wgsl"),
                "bwgpu-video-blit",
            );
            let osd_shader = self
                .gpu
                .module(include_str!("../shaders/osd_blend.wgsl"), "bwgpu-osd");

            let vpl = self
                .gpu
                .device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("bwgpu-video-pl"),
                    bind_group_layouts: &[&self.video.bgl],
                    push_constant_ranges: &[],
                });
            let opl = self
                .gpu
                .device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("bwgpu-osd-pl"),
                    bind_group_layouts: &[&self.osd.bgl],
                    push_constant_ranges: &[],
                });

            let video = self
                .gpu
                .device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("bwgpu-video-pipe"),
                    layout: Some(&vpl),
                    vertex: wgpu::VertexState {
                        module: &video_shader,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &video_shader,
                        entry_point: Some("fs_main"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: Some(wgpu::BlendState::REPLACE),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    multisample: Default::default(),
                    multiview: None,
                    cache: None,
                });

            let osd = self
                .gpu
                .device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("bwgpu-osd-pipe"),
                    layout: Some(&opl),
                    vertex: wgpu::VertexState {
                        module: &osd_shader,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &osd_shader,
                        entry_point: Some("fs_main"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: Some(wgpu::BlendState {
                                color: wgpu::BlendComponent {
                                    src_factor: wgpu::BlendFactor::One,
                                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                                    operation: wgpu::BlendOperation::Add,
                                },
                                alpha: wgpu::BlendComponent {
                                    src_factor: wgpu::BlendFactor::One,
                                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                                    operation: wgpu::BlendOperation::Add,
                                },
                            }),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    multisample: Default::default(),
                    multiview: None,
                    cache: None,
                });

            self.pipes.insert(format, Pipes { video, osd });
        }
        self.pipes.get(&format).unwrap().clone()
    }

    pub fn adapter_name(&self) -> &str {
        &self.gpu.adapter_name
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Resize the target. Safe to call with the same size (no-op).
    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        let (w, h) = (width.max(1), height.max(1));
        if self.width == w && self.height == h {
            return Ok(());
        }
        self.width = w;
        self.height = h;
        self.offscreen = None;
        if let (Some(s), Some(cfg)) = (self.surface.as_ref(), self.surface_config.as_mut()) {
            cfg.width = w;
            cfg.height = h;
            s.configure(&self.gpu.device, cfg);
        }
        self.apply_rect();
        Ok(())
    }

    /// Set the video destination rect in target pixels. This is the host's
    /// aspect-ratio / panscan decision; pixels outside render black.
    /// A zero width or height renders video-less (black + OSD), which is
    /// how `--force-window --idle` presents.
    pub fn set_viewport(&mut self, x: i32, y: i32, w: u32, h: u32) {
        self.video_rect = Some((x, y, w, h));
        self.apply_rect();
    }

    /// Set the OSD input alpha convention: `true` = bitmaps are
    /// premultiplied (mpv master SUBBITMAP_BGRA), `false` = straight
    /// alpha (default, libass convention).
    pub fn set_osd_premul(&mut self, premul: bool) {
        self.osd_premul = premul;
    }

    /// Recompute the fraction-form video rect from the stored pixel rect
    /// and push the whole uniform block.
    fn apply_rect(&mut self) {
        let (w, h) = (self.width as f32, self.height as f32);
        self.video.params.rect = match self.video_rect {
            None => [0.0, 0.0, 1.0, 1.0],
            Some((x, y, rw, rh)) => [x as f32 / w, y as f32 / h, rw as f32 / w, rh as f32 / h],
        };
        self.gpu.queue.write_buffer(
            &self.video.params_buf,
            0,
            bytemuck::bytes_of(&self.video.params),
        );
    }

    /// Select the YUV matrix and range. Applies immediately to subsequent
    /// frames (uniforms are rewritten here and on every upload).
    pub fn set_colorspace(&mut self, matrix: YuvMatrix, range: YuvRange) {
        self.colorspace = (matrix, range);
        self.video.params.set_colorspace(matrix, range);
        self.gpu.queue.write_buffer(
            &self.video.params_buf,
            0,
            bytemuck::bytes_of(&self.video.params),
        );
    }

    /// Select the video scaling filter.
    pub fn set_scale(&mut self, filter: ScaleFilter) {
        self.filter = filter;
    }

    /// Upload a video frame. Frames are pooled; identical formats/sizes
    /// reuse textures.
    pub fn set_video(&mut self, frame: &VideoFrame) -> Result<()> {
        let parity = (self.frame_count & 1) as u32;
        self.video
            .params
            .set_colorspace(self.colorspace.0, self.colorspace.1);
        self.video
            .upload(&self.gpu, frame, self.filter, 1.0, parity)
    }

    /// Remove all OSD bitmaps.
    pub fn clear_osd(&mut self) {
        self.osd.clear();
    }

    /// Add an OSD bitmap for the next frame(s).
    pub fn add_osd(&mut self, bmp: &Bitmap) -> Result<()> {
        self.osd
            .add(
                &self.gpu.device,
                &self.gpu.queue,
                bmp,
                (self.width, self.height),
                self.osd_premul,
            )
            .map_err(Error::Upload)
    }

    /// Render one frame to the surface (or to the internal offscreen target
    /// when headless) and present it.
    pub fn render_frame(&mut self) -> Result<()> {
        if self.surface.is_some() {
            let frame = self.acquire_surface_frame()?;
            let view = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let format = frame.texture.format();
            self.draw(&view, format)?;
            self.frame_count += 1;
            frame.present();
        } else {
            let view = self.offscreen_view()?;
            self.draw(&view, wgpu::TextureFormat::Rgba8Unorm)?;
            self.frame_count += 1;
        }
        Ok(())
    }

    /// Acquire the next surface texture, handling resize-behind-our-back
    /// (reconfigure and retry once).
    fn acquire_surface_frame(&mut self) -> Result<wgpu::SurfaceTexture> {
        let mut frame = self
            .surface
            .as_ref()
            .unwrap()
            .get_current_texture()
            .map_err(|e| Error::Render(format!("acquire: {e}")))?;
        if frame.texture.width() != self.width || frame.texture.height() != self.height {
            self.width = frame.texture.width().max(1);
            self.height = frame.texture.height().max(1);
            self.offscreen = None;
            if let Some(cfg) = self.surface_config.as_mut() {
                cfg.width = self.width;
                cfg.height = self.height;
            }
            if let (Some(s), Some(cfg)) = (self.surface.as_ref(), self.surface_config.as_ref()) {
                s.configure(&self.gpu.device, cfg);
            }
            self.apply_rect();
            frame = self
                .surface
                .as_ref()
                .unwrap()
                .get_current_texture()
                .map_err(|e| Error::Render(format!("acquire after resize: {e}")))?;
        }
        Ok(frame)
    }

    /// Render and read back RGBA8 pixels (screenshots, tests, C smoke).
    /// `out` needs `stride * height` bytes; stride 0 = width * 4.
    pub fn render_offscreen(&mut self, out: &mut [u8], stride: usize) -> Result<()> {
        {
            let view = self.offscreen_view()?;
            self.draw(&view, wgpu::TextureFormat::Rgba8Unorm)?;
        }
        let w = self.width as usize;
        let h = self.height as usize;
        let stride = if stride == 0 { w * 4 } else { stride };
        if out.len() < stride * (h.max(1) - 1) + w * 4 {
            return Err(Error::Upload(format!(
                "readback buffer too small: {} bytes, need {}",
                out.len(),
                stride * (h - 1) + w * 4
            )));
        }

        // Copy to a 256-aligned staging buffer.
        let bytes_per_row = (w * 4).div_ceil(256) * 256;
        let buf = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bwgpu-readback"),
            size: (bytes_per_row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let (tex, _) = self.offscreen.as_ref().unwrap();
        let mut enc = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("bwgpu-readback-enc"),
            });
        enc.copy_texture_to_buffer(
            tex.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row as u32),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        let submission = self.gpu.queue.submit(Some(enc.finish()));
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        if let Err(e) = self.gpu.device.poll(wgpu::PollType::Wait) {
            return Err(Error::Render(format!("poll: {e}")));
        }
        let _ = submission;
        let data = slice.get_mapped_range();

        for row in 0..h {
            let src = &data[row * bytes_per_row..row * bytes_per_row + w * 4];
            out[row * stride..row * stride + w * 4].copy_from_slice(src);
        }
        drop(data);
        buf.unmap();
        Ok(())
    }

    fn offscreen_view(&mut self) -> Result<TextureView> {
        if self.offscreen.is_none() {
            let tex = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("bwgpu-offscreen"),
                size: wgpu::Extent3d {
                    width: self.width,
                    height: self.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            self.offscreen = Some((tex, view));
        }
        Ok(self.offscreen.as_ref().unwrap().1.clone())
    }

    /// The one draw: video blit, then OSD quads. Same path for surface and
    /// offscreen targets.
    fn draw(&mut self, view: &TextureView, format: wgpu::TextureFormat) -> Result<()> {
        let pipes = self.ensure_pipes(format);
        // Bind groups are rebuilt per frame; OSD item counts are tiny.
        let osd_binds: Vec<BindGroup> = self
            .osd
            .items()
            .map(|(view, uniform)| self.osd_bind(view, uniform))
            .collect();
        let mut enc = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("bwgpu-frame"),
            });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bwgpu-frame-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // Video (bind group is always present: dummy textures before
            // the first upload — the rect decides whether video shows).
            pass.set_pipeline(&pipes.video);
            if let Some(bind) = self.video.bind.as_ref() {
                pass.set_bind_group(0, bind, &[]);
                pass.draw(0..3, 0..1);
            }

            // OSD
            if !osd_binds.is_empty() {
                pass.set_pipeline(&pipes.osd);
                for bg in &osd_binds {
                    pass.set_bind_group(0, bg, &[]);
                    pass.draw(0..6, 0..1);
                }
            }
        }
        self.gpu.queue.submit(Some(enc.finish()));
        Ok(())
    }

    fn osd_bind(&self, view: &TextureView, uniform: &wgpu::Buffer) -> BindGroup {
        self.gpu
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("bwgpu-osd-bind"),
                layout: &self.osd.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.osd.sampler),
                    },
                ],
            })
    }
}
