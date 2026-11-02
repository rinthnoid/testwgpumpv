//! Adapter/device bootstrap and shader loading.
//!
//! There is exactly one way to get pixels out of this project: wgpu. This
//! module is the only place that touches backend selection, and it does not
//! pick one — wgpu does (DX12 on Windows, Vulkan on Linux, Metal on macOS,
//! with the `WGPU_BACKEND` env var available for overrides/debugging).

use std::fmt;

use wgpu::{Adapter, Device, Instance, Queue, RequestAdapterError, ShaderModule};

#[derive(Debug)]
pub enum Error {
    /// No usable wgpu adapter (no GPU, or no software fallback).
    NoAdapter(RequestAdapterError),
    /// Device or surface initialization failed.
    Init(String),
    /// Invalid frame/bitmap input.
    Upload(String),
    /// Present/readback failure.
    Render(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoAdapter(e) => write!(f, "no wgpu adapter: {e}"),
            Error::Init(s) => write!(f, "init: {s}"),
            Error::Upload(s) => write!(f, "upload: {s}"),
            Error::Render(s) => write!(f, "render: {s}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// The device side of the renderer.
pub struct Gpu {
    pub device: Device,
    pub queue: Queue,
    /// Retained for surface capability queries.
    pub adapter: Adapter,
    /// Human-readable adapter name for logs/`bwgpu_version` consumers.
    pub adapter_name: String,
}

impl Gpu {
    /// Request the highest-performance adapter wgpu offers.
    pub fn new(
        instance: &Instance,
        compatible_surface: Option<&wgpu::Surface<'static>>,
    ) -> Result<Self> {
        let adapter: Adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface,
                force_fallback_adapter: false,
            }))
            .map_err(Error::NoAdapter)?;

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("bwgpu"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            ..Default::default()
        }))
        .map_err(|e| Error::Init(format!("request_device: {e}")))?;

        let info = adapter.get_info();
        let adapter_name = format!("{} ({:?})", info.name, info.backend);
        log::info!("bwgpu: adapter {adapter_name}");
        Ok(Self {
            device,
            queue,
            adapter,
            adapter_name,
        })
    }

    /// Compile a WGSL module. Shaders are embedded at build time
    /// (`include_str!`), so there is no runtime file dependency.
    pub fn module(&self, src: &'static str, label: &str) -> ShaderModule {
        self.device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(src.into()),
            })
    }
}
