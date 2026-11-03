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
    start: std::time::Instant,
}

/// Animated 4:2:0 test pattern: moving luma gradient + hue-cycling chroma.
fn test_pattern(t: f32, w: u32, h: u32) -> VideoFrame<'static> {
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);
    let mut y = vec![0u8; (w * h) as usize];
    let mut u = vec![128u8; (cw * ch) as usize];
    let mut v = vec![128u8; (cw * ch) as usize];
    for j in 0..h {
        for i in 0..w {
            let wave = ((i as f32 / w as f32 * 8.0 + t).sin() * 0.25 + 0.5) * 255.0;
            let grad = j as f32 / h as f32 * 0.4 * 255.0;
            y[(j * w + i) as usize] = (wave * 0.7 + grad * 0.3) as u8;
        }
    }
    for j in 0..ch {
        for i in 0..cw {
            u[(j * cw + i) as usize] = (((t * 0.7).sin() * 0.5 + 0.5) * 255.0) as u8;
            v[(j * cw + i) as usize] = (((t * 0.7 + 2.0).sin() * 0.5 + 0.5) * 255.0) as u8;
        }
    }
    let y: &'static [u8] = Box::leak(y.into_boxed_slice());
    let u: &'static [u8] = Box::leak(u.into_boxed_slice());
    let v: &'static [u8] = Box::leak(v.into_boxed_slice());
    VideoFrame {
        fmt: PixFmt::I420,
        width: w,
        height: h,
        planes: [
            Plane { data: y, stride: 0 },
            Plane { data: u, stride: 0 },
            Plane { data: v, stride: 0 },
        ],
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
                let (w, h) = (256, 144);
                r.set_scale(ScaleFilter::CatmullRom);
                let frame = test_pattern(t, w, h);
                if let Err(e) = r.set_video(&frame) {
                    eprintln!("upload: {e}");
                    return;
                }
                // OSD demo bar near the top.
                let rect = osd_rect(180, 36);
                let rect: &'static [u8] = Box::leak(rect.into_boxed_slice());
                r.clear_osd();
                let add = r.add_osd(&Bitmap {
                    data: rect,
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
        start: std::time::Instant::now(),
    };
    loop_handle.run_app(&mut app).expect("run");
}
