//! Window handle bridging and surface management.
//!
//! mpv hands the renderer a native window; this module converts the C-level
//! platform handles (win32/xlib/wayland/cocoa) into raw-window-handle types
//! and drives the wgpu surface. This is *presentation* glue only — every
//! platform shares the same pipeline underneath, which is the whole point
//! of rebasing mpv on wgpu.

use raw_window_handle::{
    AppKitDisplayHandle, AppKitWindowHandle, DisplayHandle, HasDisplayHandle, HasWindowHandle,
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
    Win32WindowHandle, WindowHandle, XlibDisplayHandle, XlibWindowHandle,
};
use std::num::NonZeroIsize;
use std::ptr::NonNull;
use wgpu::SurfaceConfiguration;

use crate::device::{Error, Result};

/// Platform tag used by the C ABI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Platform {
    Win32 = 0,
    Xlib = 1,
    Wayland = 2,
    Cocoa = 3,
}

/// A raw (display, window) pair owned together, implementing the
/// raw-window-handle traits so `SurfaceTargetUnsafe::from_window` accepts it.
pub struct RawPair {
    pub display: RawDisplayHandle,
    pub window: RawWindowHandle,
}

impl HasDisplayHandle for RawPair {
    fn display_handle(
        &self,
    ) -> std::result::Result<DisplayHandle<'_>, raw_window_handle::HandleError> {
        Ok(unsafe { DisplayHandle::borrow_raw(self.display) })
    }
}

impl HasWindowHandle for RawPair {
    fn window_handle(
        &self,
    ) -> std::result::Result<WindowHandle<'_>, raw_window_handle::HandleError> {
        Ok(unsafe { WindowHandle::borrow_raw(self.window) })
    }
}

/// Build a `RawPair` from the flat C-style descriptor.
///
/// The caller guarantees the referenced window/display outlive the renderer
/// (documented in the C ABI contract).
pub fn build_pair(
    platform: Platform,
    hwnd: isize,
    xwin: u32,
    display: *mut std::ffi::c_void,
    wayland_surface: *mut std::ffi::c_void,
    ns_view: *mut std::ffi::c_void,
) -> Result<RawPair> {
    let nn = |p, what: &str| -> Result<NonNull<std::ffi::c_void>> {
        NonNull::new(p).ok_or_else(|| Error::Init(format!("{what} pointer is NULL")))
    };
    let (display_handle, window_handle) = match platform {
        Platform::Win32 => {
            let hwnd = NonZeroIsize::new(hwnd).ok_or_else(|| Error::Init("HWND is NULL".into()))?;
            let mut wh = Win32WindowHandle::new(hwnd);
            wh.hinstance = NonZeroIsize::new(0); // wgpu does not need it
            (
                RawDisplayHandle::Windows(raw_window_handle::WindowsDisplayHandle::new()),
                RawWindowHandle::Win32(wh),
            )
        }
        Platform::Xlib => (
            RawDisplayHandle::Xlib(XlibDisplayHandle::new(NonNull::new(display), 0)),
            RawWindowHandle::Xlib(XlibWindowHandle::new(xwin as std::os::raw::c_ulong)),
        ),
        Platform::Wayland => (
            RawDisplayHandle::Wayland(WaylandDisplayHandle::new(nn(display, "wl_display")?)),
            RawWindowHandle::Wayland(WaylandWindowHandle::new(nn(wayland_surface, "wl_surface")?)),
        ),
        Platform::Cocoa => (
            RawDisplayHandle::AppKit(AppKitDisplayHandle::new()),
            RawWindowHandle::AppKit(AppKitWindowHandle::new(nn(ns_view, "NSView")?)),
        ),
    };
    Ok(RawPair {
        display: display_handle,
        window: window_handle,
    })
}

/// Pick a sane 8-bit swapchain format: prefer plain (non-srgb) RGBA/BGRA so
/// the renderer controls the full color pipeline and does not get a hidden
/// sRGB encode. Video is authored in signal RGB; transfer functions (HDR)
/// arrive with the M3 milestone.
pub fn pick_format(caps: &wgpu::SurfaceCapabilities) -> wgpu::TextureFormat {
    use wgpu::TextureFormat as F;
    let wanted = [F::Rgba8Unorm, F::Bgra8Unorm];
    for w in wanted {
        if caps.formats.contains(&w) {
            return w;
        }
    }
    caps.formats[0]
}

pub fn make_config(
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    vsync: bool,
) -> SurfaceConfiguration {
    wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: width.max(1),
        height: height.max(1),
        present_mode: if vsync {
            wgpu::PresentMode::Fifo
        } else {
            wgpu::PresentMode::Immediate
        },
        alpha_mode: wgpu::CompositeAlphaMode::Auto,
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    }
}
