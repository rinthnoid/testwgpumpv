//! Demo player window: proves the full path — winit window → wgpu surface
//! → animated YUV test pattern → OSD quad. No video decoder involved; this
//! is the renderer's "hello world" and a handy manual test harness.
//!
//! ```sh
//! cargo run --example player
//! ```

use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::window::{Window, WindowId};

use bwgpu::{Bitmap, PixFmt, Plane, Renderer, ScaleFilter, VideoFrame, YuvMatrix, YuvRange};

struct App {
    renderer: Option<Renderer>,
    window: Option<Arc<Window>>,
    pattern: PatternBuf,
    start: std::time::Instant,
}

/// Animated 4:2:0 test pattern: moving luma gradient + hue-cycling chroma.
/// Fills the caller-owned plane buffers in place; `set_video` copies the
/// planes into GPU memory before returning, so the borrow ends right after.
fn fill_pattern(buf: &mut PatternBuf, t: f32) {
    let (w, h) = (buf.width, buf.height);
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);
    for j in 0..h {
        for i in 0..w {
            let wave = ((i as f32 / w as f32 * 8.0 + t).sin() * 0.25 + 0.5) * 255.0;
            let grad = j as f32 / h as f32 * 0.4 * 255.0;
            buf.y[(j * w + i) as usize] = (wave * 0.7 + grad * 0.3) as u8;
        }
    }
    for j in 0..ch {
        for i in 0..cw {
            buf.u[(j * cw + i) as usize] = (((t * 0.7).sin() * 0.5 + 0.5) * 255.0) as u8;
            buf.v[(j * cw + i) as usize] = (((t * 0.7 + 2.0).sin() * 0.5 + 0.5) * 255.0) as u8;
        }
    }
}

/// Reusable plane storage for the test pattern (no per-frame allocation).
struct PatternBuf {
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    width: u32,
    height: u32,
}

impl PatternBuf {
    fn new(width: u32, height: u32) -> Self {
        let (cw, ch) = (width.div_ceil(2), height.div_ceil(2));
        Self {
            y: vec![0u8; (width * height) as usize],
            u: vec![128u8; (cw * ch) as usize],
            v: vec![128u8; (cw * ch) as usize],
            width,
            height,
        }
    }

    fn frame(&self) -> VideoFrame<'_> {
        VideoFrame {
            fmt: PixFmt::I420,
            width: self.width,
            height: self.height,
            planes: [
                Plane {
                    data: &self.y,
                    stride: 0,
                },
                Plane {
                    data: &self.u,
                    stride: 0,
                },
                Plane {
                    data: &self.v,
                    stride: 0,
                },
            ],
        }
    }
}

/// A colored OSD rectangle (stand-in for subtitle bitmaps).
fn osd_rect(w: u32, h: u32) -> Vec<u8> {
    let mut px = vec![0u8; (w * h * 4) as usize];
    for j in 0..h {
        for i in 0..w {
            // Soft edge, mostly opaque teal.
            let edge = (i.min(w - 1 - i).min(j.min(h - 1 - j)) < 2) as u32;
            let o = ((j * w + i) * 4) as usize;
            px[o] = 0;
            px[o + 1] = 180;
            px[o + 2] = 180;
            px[o + 3] = if edge == 1 { 128 } else { 220 };
        }
    }
    px
}

impl ApplicationHandler for App {
    fn resumed(&mut self, loop_handle: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("bwgpu — mpv, rebased on wgpu")
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 540.0));
        let window = Arc::new(loop_handle.create_window(attrs).expect("create window"));
        let size = window.inner_size();
        let pair = bwgpu::surface::RawPair {
            display: window.display_handle().expect("display").as_raw(),
            window: window.window_handle().expect("window").as_raw(),
        };
        let mut renderer = Renderer::for_window(&pair, size.width.max(1), size.height.max(1), true)
            .expect("renderer");
        renderer.set_colorspace(YuvMatrix::Bt709, YuvRange::Limited);
        eprintln!("bwgpu ready: {}", renderer.adapter_name());
        self.renderer = Some(renderer);
        self.window = Some(window);
    }

    fn window_event(&mut self, loop_handle: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => loop_handle.exit(),
            WindowEvent::Resized(size) => {
                if let Some(r) = self.renderer.as_mut() {
                    let _ = r.resize(size.width.max(1), size.height.max(1));
                }
            }
            WindowEvent::RedrawRequested => {
                let Some(r) = self.renderer.as_mut() else {
                    return;
                };
                let t = self.start.elapsed().as_secs_f32();
                r.set_scale(ScaleFilter::CatmullRom);
                fill_pattern(&mut self.pattern, t);
                let frame = self.pattern.frame();
                if let Err(e) = r.set_video(&frame) {
                    eprintln!("upload: {e}");
                    return;
                }
                // OSD demo bar near the top (copies into GPU memory;
                // no need to keep the buffer alive afterwards).
                let rect = osd_rect(180, 36);
                r.clear_osd();
                let add = r.add_osd(&Bitmap {
                    data: &rect,
                    width: 180,
                    height: 36,
                    x: 24,
                    y: 24,
                });
                let _ = add;
                if let Err(e) = r.render_frame() {
                    eprintln!("render: {e}");
                }
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }
}

fn main() {
    let _ = env_logger::try_init();
    let loop_handle = EventLoop::new().expect("event loop");
    loop_handle.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        renderer: None,
        window: None,
        pattern: PatternBuf::new(256, 144),
        start: std::time::Instant::now(),
    };
    loop_handle.run_app(&mut app).expect("run");
}
