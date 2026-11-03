//! The stable C ABI mpv links against (`include/bwgpu.h` is the contract).
//!
//! Rules:
//! - Opaque `bwgpu_renderer` handles; nothing else crosses the boundary.
//! - Every entry point is panic-guarded (a Rust panic must never unwind
//!   into C) and never takes a lock the caller would notice.
//! - Errors return a negative `int`; the message is fetched with
//!   `bwgpu_last_error()` from the calling thread (thread-local storage).

use std::cell::RefCell;
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::surface::Platform;
use crate::{Bitmap, PixFmt, Renderer, ScaleFilter, VideoFrame, YuvMatrix, YuvRange};

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

fn set_error(msg: impl std::fmt::Display) {
    let s = msg.to_string();
    log::error!("bwgpu: {s}");
    // Truncate rather than fail on interior NULs.
    let bytes = s.as_bytes();
    let bytes = &bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())];
    LAST_ERROR.with(|e| {
        *e.borrow_mut() = CString::new(bytes).ok();
    });
}

/// Run `f` behind a panic guard. On panic: record it as the last error and
/// return `default`.
fn guard<T>(default: T, f: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(_) => {
            set_error("internal panic (bug in bwgpu, please report)");
            default
        }
    }
}

const OK: c_int = 0;
const ERR: c_int = -1;

// ---------------------------------------------------------------------------
// Window descriptor (mirrors `bwgpu_window` in the header)
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct CWindow {
    pub platform: c_int,
    pub hwnd: usize,
    pub xwin: u32,
    pub display: *mut c_void,
    pub wayland_surface: *mut c_void,
    pub ns_view: *mut c_void,
}

fn renderer_from_ptr(ptr: *mut Renderer) -> Option<&'static mut Renderer> {
    // Mirrors the C contract: only pointers handed out by renderer_new are
    // valid; NULL is not an error here, callers check it beforehand.
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { &mut *ptr })
}

fn null_renderer_error() -> c_int {
    set_error("renderer handle is NULL");
    ERR
}

// ---------------------------------------------------------------------------
// Exported entry points
// ---------------------------------------------------------------------------

/// Library version string (static, valid for the process lifetime).
#[no_mangle]
pub extern "C" fn bwgpu_version() -> *const c_char {
    concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

/// Last error on this thread, or NULL. The pointer stays valid until the
/// next bwgpu call on the same thread.
#[no_mangle]
pub extern "C" fn bwgpu_last_error() -> *const c_char {
    LAST_ERROR.with(|e| {
        e.borrow()
            .as_ref()
            .map(|s| s.as_ptr())
            .unwrap_or(std::ptr::null())
    })
}

/// Create a renderer.
///
/// `win == NULL` builds a headless renderer (offscreen + readback only).
/// `win` must outlive the returned renderer. Returns NULL on failure;
/// call `bwgpu_last_error()` for the reason.
///
/// # Safety
/// `win` (if non-NULL) must point to a valid `bwgpu_window` whose native
/// handles are alive for the renderer's lifetime.
#[no_mangle]
pub unsafe extern "C" fn bwgpu_renderer_new(
    win: *const CWindow,
    width: u32,
    height: u32,
    vsync: c_int,
) -> *mut Renderer {
    guard(std::ptr::null_mut(), || {
        let result = if win.is_null() {
            Renderer::new(width.max(1), height.max(1))
        } else {
            let w = &*win;
            let platform = match w.platform {
                0 => Platform::Win32,
                1 => Platform::Xlib,
                2 => Platform::Wayland,
                3 => Platform::Cocoa,
                other => {
                    set_error(format!("unknown platform tag {other}"));
                    return std::ptr::null_mut();
                }
            };
            crate::surface::build_pair(
                platform,
                w.hwnd as isize,
                w.xwin,
                w.display,
                w.wayland_surface,
                w.ns_view,
            )
            .and_then(|pair| Renderer::for_window(&pair, width.max(1), height.max(1), vsync != 0))
        };
        match result {
            Ok(r) => Box::into_raw(Box::new(r)),
            Err(e) => {
                set_error(e);
                std::ptr::null_mut()
            }
        }
    })
}

/// Destroy a renderer. NULL is a no-op.
///
/// # Safety
/// `r` must be a pointer from `bwgpu_renderer_new`, not yet freed, and not
/// used again afterwards.
#[no_mangle]
pub unsafe extern "C" fn bwgpu_renderer_free(r: *mut Renderer) {
    guard((), || {
        if !r.is_null() {
            drop(Box::from_raw(r));
        }
    })
}

/// Resize the render target.
///
/// # Safety
/// `r` must be a live renderer pointer.
#[no_mangle]
pub unsafe extern "C" fn bwgpu_renderer_resize(r: *mut Renderer, w: u32, h: u32) -> c_int {
    guard(ERR, || match renderer_from_ptr(r) {
        None => null_renderer_error(),
        // A zero-size target is a host bug, not something to silently
        // clamp: reject it so mpv-side geometry mistakes are visible.
        Some(_) if w == 0 || h == 0 => {
            set_error("resize: zero width or height");
            ERR
        }
        Some(rend) => match rend.resize(w, h) {
            Ok(()) => OK,
            Err(e) => {
                set_error(e);
                ERR
            }
        },
    })
}

/// Frame descriptor (mirrors `bwgpu_frame`).
#[repr(C)]
pub struct CPlane {
    pub data: *const u8,
    pub size: usize,
    pub stride: usize,
}

#[repr(C)]
pub struct CFrame {
    pub fmt: c_int,
    pub width: u32,
    pub height: u32,
    pub y: CPlane,
    pub u: CPlane,
    pub v: CPlane,
}

/// Upload one video frame. See `bwgpu_frame` in the header for plane rules.
///
/// # Safety
/// `r` must be live; plane pointers must stay valid for the duration of the
/// call (the upload copies into GPU memory before returning).
#[no_mangle]
pub unsafe extern "C" fn bwgpu_set_video(r: *mut Renderer, f: *const CFrame) -> c_int {
    guard(ERR, || {
        if f.is_null() {
            set_error("frame pointer is NULL");
            return ERR;
        }
        let rend = match renderer_from_ptr(r) {
            Some(x) => x,
            None => return null_renderer_error(),
        };
        let cf = &*f;
        let fmt = match PixFmt::from_i32(cf.fmt) {
            Some(f) => f,
            None => {
                set_error(format!("unknown pixel format {}", cf.fmt));
                return ERR;
            }
        };
        let plane = |p: &CPlane| crate::video::Plane {
            data: if p.data.is_null() || p.size == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(p.data, p.size)
            },
            stride: p.stride,
        };
        let frame = VideoFrame {
            fmt,
            width: cf.width,
            height: cf.height,
            planes: [plane(&cf.y), plane(&cf.u), plane(&cf.v)],
        };
        match rend.set_video(&frame) {
            Ok(()) => OK,
            Err(e) => {
                set_error(e);
                ERR
            }
        }
    })
}

/// Select colorspace: `matrix` 0=BT.601 1=BT.709 2=BT.2020,
/// `range` 0=limited 1=full.
///
/// # Safety
/// `r` must be a live renderer pointer.
#[no_mangle]
pub unsafe extern "C" fn bwgpu_set_colorspace(
    r: *mut Renderer,
    matrix: c_int,
    range: c_int,
) -> c_int {
    guard(ERR, || {
        let rend = match renderer_from_ptr(r) {
            Some(x) => x,
            None => return null_renderer_error(),
        };
        match (YuvMatrix::from_i32(matrix), YuvRange::from_i32(range)) {
            (Some(m), Some(g)) => {
                rend.set_colorspace(m, g);
                OK
            }
            _ => {
                set_error("invalid colorspace arguments");
                ERR
            }
        }
    })
}

/// Select the scaling filter: 0=nearest 1=bilinear 2=Catmull-Rom.
///
/// # Safety
/// `r` must be a live renderer pointer.
#[no_mangle]
pub unsafe extern "C" fn bwgpu_set_scale(r: *mut Renderer, filter: c_int) -> c_int {
    guard(ERR, || {
        let rend = match renderer_from_ptr(r) {
            Some(x) => x,
            None => return null_renderer_error(),
        };
        match ScaleFilter::from_i32(filter) {
            Some(f) => {
                rend.set_scale(f);
                OK
            }
            None => {
                set_error("invalid filter");
                ERR
            }
        }
    })
}

/// Set the video destination rect in target pixels (aspect ratio /
/// panscan as decided by the host). Pixels outside render black. A zero
/// width or height renders video-less (black + OSD), e.g. `--force-window
/// --idle`. Call again after resize; a full-surface blit is the default.
///
/// # Safety
/// `r` must be a live renderer pointer.
#[no_mangle]
pub unsafe extern "C" fn bwgpu_set_viewport(
    r: *mut Renderer,
    x: c_int,
    y: c_int,
    w: u32,
    h: u32,
) -> c_int {
    guard(ERR, || match renderer_from_ptr(r) {
        Some(rend) => {
            rend.set_viewport(x, y, w, h);
            OK
        }
        None => null_renderer_error(),
    })
}

/// Select the OSD input alpha convention: 0 = straight alpha (default,
/// libass), 1 = premultiplied alpha (mpv master's SUBBITMAP_BGRA). Set
/// once at startup; applies to all subsequent bwgpu_add_osd calls.
///
/// # Safety
/// `r` must be a live renderer pointer.
#[no_mangle]
pub unsafe extern "C" fn bwgpu_set_osd_premul(r: *mut Renderer, premul: c_int) -> c_int {
    guard(ERR, || match renderer_from_ptr(r) {
        Some(rend) => {
            rend.set_osd_premul(premul != 0);
            OK
        }
        None => null_renderer_error(),
    })
}

/// Drop all OSD bitmaps.
///
/// # Safety
/// `r` must be a live renderer pointer.
#[no_mangle]
pub unsafe extern "C" fn bwgpu_clear_osd(r: *mut Renderer) -> c_int {
    guard(ERR, || match renderer_from_ptr(r) {
        Some(x) => {
            x.clear_osd();
            OK
        }
        None => null_renderer_error(),
    })
}

/// Bitmap descriptor (mirrors `bwgpu_bitmap`).
#[repr(C)]
pub struct CBitmap {
    pub data: *const u8,
    pub size: usize,
    pub width: u32,
    pub height: u32,
    pub x: c_int,
    pub y: c_int,
}

/// Add a straight-alpha RGBA8 OSD bitmap.
///
/// # Safety
/// `r` must be live; `b->data` must stay valid for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn bwgpu_add_osd(r: *mut Renderer, b: *const CBitmap) -> c_int {
    guard(ERR, || {
        if b.is_null() {
            set_error("bitmap pointer is NULL");
            return ERR;
        }
        let rend = match renderer_from_ptr(r) {
            Some(x) => x,
            None => return null_renderer_error(),
        };
        let cb = &*b;
        let data = if cb.data.is_null() || cb.size == 0 {
            &[][..]
        } else {
            std::slice::from_raw_parts(cb.data, cb.size)
        };
        let bmp = Bitmap {
            data,
            width: cb.width,
            height: cb.height,
            x: cb.x,
            y: cb.y,
        };
        match rend.add_osd(&bmp) {
            Ok(()) => OK,
            Err(e) => {
                set_error(e);
                ERR
            }
        }
    })
}

/// Render + present one frame.
///
/// # Safety
/// `r` must be a live renderer pointer.
#[no_mangle]
pub unsafe extern "C" fn bwgpu_render_frame(r: *mut Renderer) -> c_int {
    guard(ERR, || match renderer_from_ptr(r) {
        Some(x) => match x.render_frame() {
            Ok(()) => OK,
            Err(e) => {
                set_error(e);
                ERR
            }
        },
        None => null_renderer_error(),
    })
}

/// Render and read back RGBA8 pixels into `out` (`stride` bytes per row,
/// 0 = width*4). Works headless and with a window (screenshot path).
///
/// # Safety
/// `r` must be live; `out` must have room for `stride * height` bytes.
#[no_mangle]
pub unsafe extern "C" fn bwgpu_render_offscreen(
    r: *mut Renderer,
    out: *mut u8,
    out_size: usize,
    stride: usize,
) -> c_int {
    guard(ERR, || {
        let rend = match renderer_from_ptr(r) {
            Some(x) => x,
            None => return null_renderer_error(),
        };
        if out.is_null() {
            set_error("output buffer is NULL");
            return ERR;
        }
        let dst = std::slice::from_raw_parts_mut(out, out_size);
        match rend.render_offscreen(dst, stride) {
            Ok(()) => OK,
            Err(e) => {
                set_error(e);
                ERR
            }
        }
    })
}
