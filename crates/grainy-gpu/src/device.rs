use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::Duration;

use grainy_core::{Image, Plane, Settings};

use crate::pool::{BatchPool, Lease, Pool};
use crate::{params, Error, Resident, Result};

const SLAB_BYTES: u64 = 64 * 1024;
const REDUCE_SCRATCH_BYTES: u64 = 2049 * 16;
const REDUCE_PROFILE_BYTES: u64 = 8 * 8;
pub(crate) const FILM_BASE: usize = 35;
pub(crate) const LOCAL_BASE: usize = 40;
pub(crate) const GEOMETRY_STAGE: usize = 43;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Adapter policy for the port of `native_gpu._handle`: discrete first, or software for parity tests.
#[derive(Clone, Copy, Debug, Default)]
pub enum AdapterKind {
    /// Prefer a discrete GPU, then an integrated GPU, then other available adapters; never a software rasterizer
    /// (`DeviceType::Cpu`: WARP, llvmpipe, SwiftShader). The CPU engine gives the same pixels faster than a software
    /// adapter, and WARP on Windows Server loses its device in the heavier stages (the window, which shares the
    /// adapter, then fails too).
    #[default]
    Hardware,
    /// Request wgpu's `force_fallback_adapter` (WARP on Windows).
    Software,
}

/// Port of a `native_gpu.develop_resident` device slot: interleaved RGB float32, with no implicit readback.
#[derive(Clone, Debug)]
pub struct Frame {
    pub(crate) buffer: wgpu::Buffer,
    pub(crate) width: usize,
    pub(crate) height: usize,
    owner: u64,
    // Kept after `buffer` so the last raw frame handle drops before returning the lease.
    _lease: Option<Arc<Lease>>,
}

impl Frame {
    /// Shape of a `native_gpu._frame`, in (height, width, channels) order.
    pub fn shape(&self) -> (usize, usize, usize) {
        (self.height, self.width, 3)
    }
}

/// Port of `native_gpu._handle`, with one wgpu implementation for DX12, Vulkan and Metal.
pub struct Gpu {
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    info: wgpu::AdapterInfo,
    pub(crate) limits: wgpu::Limits,
    pub(crate) owner: u64,
    failed: Arc<Mutex<Option<String>>>,
    pipelines: Vec<wgpu::ComputePipeline>,
    layout: wgpu::BindGroupLayout,
    reduce_pipelines: [wgpu::ComputePipeline; 5],
    reduce_layout: wgpu::BindGroupLayout,
    pub(crate) present_pipelines: [wgpu::ComputePipeline; 2],
    pub(crate) present_layout: wgpu::BindGroupLayout,
    pub(crate) laplacian_pipelines: [wgpu::ComputePipeline; 7],
    pub(crate) laplacian_layout: wgpu::BindGroupLayout,
    pub(crate) empty_storage: wgpu::Buffer,
    pub(crate) empty_lut: wgpu::Buffer,
    pub(crate) empty_stats: wgpu::Buffer,
    pub(crate) wavelet_taps: wgpu::Buffer,
    pub(crate) box2_taps: wgpu::Buffer,
    outputs: Arc<Mutex<Pool>>,
    staging: Arc<Mutex<Pool>>,
    slabs: Arc<Mutex<Pool>>,
    reduce_scratch: Arc<Mutex<Pool>>,
    reduce_profiles: Arc<Mutex<Pool>>,
    pub(crate) film_stocks: Mutex<HashMap<String, Arc<wgpu::Buffer>>>,
}

impl Gpu {
    /// Port of `native_gpu._handle`: prefer a discrete adapter; log a clear diagnostic and return None on failure.
    pub fn new() -> Option<Self> {
        match Self::with_adapter(AdapterKind::Hardware) {
            Ok(Some(gpu)) => Some(gpu),
            Ok(None) => {
                eprintln!("grainy-gpu: no compute adapter; use CPU");
                None
            }
            Err(e) => {
                eprintln!("grainy-gpu: {e}; use CPU");
                None
            }
        }
    }

    /// Port of `native_gpu._handle`, with explicit adapter selection and initialization diagnostics.
    pub fn with_adapter(kind: AdapterKind) -> Result<Option<Self>> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::DX12 | wgpu::Backends::VULKAN | wgpu::Backends::METAL,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        Self::with_instance(kind, instance)
    }

    /// Attach `grainy-gpu` compute and presentation pipelines to an existing `wgpu::Device`
    /// and `wgpu::Queue` (such as `eframe`'s `egui_wgpu::RenderState`) for zero-copy viewport
    /// rendering without replacing the host's device error callbacks.
    pub fn from_shared(adapter: &wgpu::Adapter, device: wgpu::Device, queue: wgpu::Queue) -> Result<Self> {
        let info = adapter.get_info();
        let limits = device.limits();
        if limits.max_storage_buffers_per_shader_stage < 8 {
            return Err(Error::Limit(format!(
                "shared wgpu device exposes {} storage buffers per stage (need 8)",
                limits.max_storage_buffers_per_shader_stage
            )));
        }
        Self::init(info, limits, device, queue, false)
    }

    fn with_instance(kind: AdapterKind, instance: wgpu::Instance) -> Result<Option<Self>> {
        let adapter =
            match kind {
                AdapterKind::Hardware => {
                    let mut adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
                    adapters.sort_by_key(|a| {
                        (
                            match a.get_info().device_type {
                                wgpu::DeviceType::DiscreteGpu => 0,
                                wgpu::DeviceType::IntegratedGpu => 1,
                                wgpu::DeviceType::VirtualGpu => 2,
                                wgpu::DeviceType::Other => 3,
                                wgpu::DeviceType::Cpu => 4,
                            },
                            if a.get_info().backend == wgpu::Backend::Dx12 { 0 } else { 1 },
                        )
                    });
                    adapters.into_iter().find(|a| a.get_info().device_type != wgpu::DeviceType::Cpu)
                }
                AdapterKind::Software => pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    force_fallback_adapter: true,
                    ..Default::default()
                }))
                .ok(),
            };
        let Some(adapter) = adapter else {
            return Ok(None);
        };
        let info = adapter.get_info();
        let limits = adapter.limits();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("Grainy compute"),
            required_limits: limits.clone(),
            ..Default::default()
        }))
        .map_err(|e| Error::Device(e.to_string()))?;
        Ok(Some(Self::init(info, limits, device, queue, true)?))
    }

    fn init(
        info: wgpu::AdapterInfo,
        limits: wgpu::Limits,
        device: wgpu::Device,
        queue: wgpu::Queue,
        own_callbacks: bool,
    ) -> Result<Self> {
        let failed = Arc::new(Mutex::new(None));
        if own_callbacks {
            let state = failed.clone();
            device.set_device_lost_callback(move |reason, message| {
                *state.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(format!("device lost ({reason:?}): {message}"));
            });
            let state = failed.clone();
            device.on_uncaptured_error(Arc::new(move |e| {
                *state.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(format!("{e}"));
            }));
        }
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let entries: Vec<_> = (0..8)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: binding != 1 },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Grainy float32 stages"),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Grainy"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let mut pipelines = Vec::new();
        for (name, code, stages, const_name) in [
            ("tone", include_str!("tone.wgsl"), 1usize, ""),
            ("color", include_str!("color.wgsl"), 1, ""),
            ("finish", include_str!("finish.wgsl"), 1, ""),
            ("detail", include_str!("detail.wgsl"), 32, "detail_stage"),
            ("film", include_str!("film.wgsl"), 5, "film_stage"),
            ("local", include_str!("local.wgsl"), 3, "local_stage"),
            ("geometry", include_str!("geometry.wgsl"), 1, ""),
            ("ai_denoise", include_str!("ai_denoise.wgsl"), 1, ""),
        ] {
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(name),
                source: wgpu::ShaderSource::Wgsl(format!("{}\n{code}", include_str!("common.wgsl")).into()),
            });
            for stage in 0..stages {
                let constants = [(const_name, stage as f64)];
                pipelines.push(device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(&format!("{name}/{stage}")),
                    layout: Some(&pipeline_layout),
                    module: &shader,
                    entry_point: Some("main"),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants: if !const_name.is_empty() { &constants } else { &[] },
                        ..Default::default()
                    },
                    cache: None,
                }));
            }
        }
        let reduce_entries: Vec<_> = (0..4)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: binding == 0 || binding == 3 },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();
        let reduce_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Grainy wavelet reduction"),
            entries: &reduce_entries,
        });
        let reduce_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Grainy reduce"),
            bind_group_layouts: &[Some(&reduce_layout)],
            immediate_size: 0,
        });
        let reduce_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("reduce"),
            source: wgpu::ShaderSource::Wgsl(include_str!("reduce.wgsl").into()),
        });
        let reduce_names = ["mean_part", "mean_final", "hist_part", "hist_final", "halve_profile"];
        let reduce_pipelines = std::array::from_fn(|i| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(reduce_names[i]),
                layout: Some(&reduce_pipeline_layout),
                module: &reduce_shader,
                entry_point: Some(reduce_names[i]),
                compilation_options: Default::default(),
                cache: None,
            })
        });
        let present_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Grainy present"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let present_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Grainy present"),
            bind_group_layouts: &[Some(&present_layout)],
            immediate_size: 0,
        });
        let present_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("present"),
            source: wgpu::ShaderSource::Wgsl(include_str!("present.wgsl").into()),
        });
        let present_names = ["present_frame", "clip_overlay"];
        let present_pipelines = std::array::from_fn(|i| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(present_names[i]),
                layout: Some(&present_pipeline_layout),
                module: &present_shader,
                entry_point: Some(present_names[i]),
                compilation_options: Default::default(),
                cache: None,
            })
        });
        let lap_entries: Vec<_> = (0..5)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: binding != 3 },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();
        let laplacian_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Grainy local laplacian"),
            entries: &lap_entries,
        });
        let laplacian_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Grainy local laplacian"),
            bind_group_layouts: &[Some(&laplacian_layout)],
            immediate_size: 0,
        });
        let laplacian_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("laplacian"),
            source: wgpu::ShaderSource::Wgsl(include_str!("laplacian.wgsl").into()),
        });
        let lap_names =
            ["extract_luma", "remap_luma", "down_h", "down_v", "accum_laplacian", "collapse_add", "apply_luma"];
        let laplacian_pipelines = std::array::from_fn(|i| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(lap_names[i]),
                layout: Some(&laplacian_pipeline_layout),
                module: &laplacian_shader,
                entry_point: Some(lap_names[i]),
                compilation_options: Default::default(),
                cache: None,
            })
        });
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(Error::Device(e.to_string()));
        }
        let empty_storage = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("empty table and auxiliary"),
            size: 8,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let empty_lut = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("empty monitor LUT"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let empty_stats = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("empty present stats"),
            size: 385 * 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let init_static = |label: &str, data: &[[f32; 2]]| -> Result<wgpu::Buffer> {
            let contents = bytemuck::cast_slice(data);
            let unpadded = contents.len() as wgpu::BufferAddress;
            let mask = wgpu::COPY_BUFFER_ALIGNMENT - 1;
            let size = ((unpadded + mask) & !mask).max(wgpu::COPY_BUFFER_ALIGNMENT);
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: true,
            });
            {
                let mut view = buffer.get_mapped_range_mut(..).map_err(|e| Error::Device(format!("{label}: {e}")))?;
                view.slice(..unpadded as usize).copy_from_slice(contents);
            }
            buffer.unmap();
            Ok(buffer)
        };
        let wavelet_taps = init_static(
            "wavelet 5-tap kernel",
            &[[1.0 / 16.0, 0.0], [4.0 / 16.0, 0.0], [6.0 / 16.0, 0.0], [4.0 / 16.0, 0.0], [1.0 / 16.0, 0.0]],
        )?;
        let box2_taps = init_static("box(2) kernel", &[[0.2, 0.0]; 5])?;
        let gpu = Self {
            empty_storage,
            empty_lut,
            empty_stats,
            wavelet_taps,
            box2_taps,
            device,
            queue,
            info,
            limits,
            owner: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            failed,
            pipelines,
            layout,
            reduce_pipelines,
            reduce_layout,
            present_pipelines,
            present_layout,
            laplacian_pipelines,
            laplacian_layout,
            outputs: Pool::new(1024 * 1024 * 1024, 16),
            staging: Pool::new(512 * 1024 * 1024, 2),
            slabs: Pool::new(4 * 1024 * 1024, 8),
            reduce_scratch: Pool::new(4 * 1024 * 1024, 4),
            reduce_profiles: Pool::new(64 * 1024, 8),
            film_stocks: Mutex::new(HashMap::new()),
        };
        gpu.check()?;
        Ok(gpu)
    }

    /// Underlying `wgpu::Device` (shared with `eframe` when initialized via [`Self::from_shared`]).
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// Underlying `wgpu::Queue`.
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Port of `native_gpu.status`: the selected adapter and backend.
    pub fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.info
    }

    pub(crate) fn check(&self) -> Result<()> {
        match &*self.failed.lock().unwrap_or_else(std::sync::PoisonError::into_inner) {
            Some(e) => Err(Error::Device(e.clone())),
            None => Ok(()),
        }
    }

    fn bytes(&self, height: usize, width: usize) -> Result<u64> {
        let bytes = height
            .checked_mul(width)
            .and_then(|n| n.checked_mul(12))
            .ok_or_else(|| Error::Limit("image size overflow".into()))? as u64;
        if height == 0 || width == 0 {
            return Err(Error::Input("empty frame".into()));
        }
        if bytes > self.limits.max_buffer_size
            || bytes > self.limits.max_storage_buffer_binding_size
            || height.div_ceil(16) > self.limits.max_compute_workgroups_per_dimension as usize
            || width.div_ceil(16) > self.limits.max_compute_workgroups_per_dimension as usize
        {
            return Err(Error::Limit(format!("{width}x{height}, {bytes} bytes exceed adapter limits")));
        }
        Ok(bytes)
    }

    pub(crate) fn check_frame(&self, frame: &Frame) -> Result<()> {
        self.check()?;
        if frame.owner != self.owner {
            return Err(Error::ForeignFrame);
        }
        Ok(())
    }

    /// Port of the upload in `native_gpu.develop_resident`; RGB float32 storage has no half-float quantization.
    pub fn upload(&self, image: &Image) -> Result<Frame> {
        let _span = grainy_core::timing::Span::new("gpu.upload");
        self.check()?;
        let (height, width, channels) = image.dim();
        if channels != 3 || !image.iter().all(|v| v.is_finite()) {
            return Err(Error::Input("expected finite H x W x 3 pixels".into()));
        }
        self.bytes(height, width)?;
        let contiguous = image.as_standard_layout();
        let values = contiguous.as_slice().ok_or_else(|| Error::Input("cannot make contiguous image".into()))?;
        let buffer = self.buffer_init(
            "Grainy upload",
            bytemuck::cast_slice(values),
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        )?;
        self.check()?;
        Ok(Frame { buffer, width, height, owner: self.owner, _lease: None })
    }

    /// Port of the readback in `native_gpu.develop_resident`; waits with a bounded timeout and returns device errors.
    pub fn readback(&self, frame: &Frame) -> Result<Image> {
        self.check_frame(frame)?;
        let bytes = self.bytes(frame.height, frame.width)?;
        let staging = self.staging.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take(
            &self.device,
            &wgpu::BufferDescriptor {
                label: Some("Grainy readback"),
                size: bytes,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            },
        );
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&frame.buffer, 0, &staging, 0, bytes);
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = mpsc::sync_channel(1);
        staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let wait = grainy_core::timing::Span::new("gpu.readback.wait");
        self.wait()?;
        rx.recv_timeout(Duration::from_secs(30))
            .map_err(|e| Error::Device(e.to_string()))?
            .map_err(|e| Error::Device(e.to_string()))?;
        drop(wait);
        let copying = grainy_core::timing::Span::new("gpu.readback.copy");
        let mapped = staging.slice(..).get_mapped_range().map_err(|e| Error::Device(e.to_string()))?;
        // Mapping alignment is implementation-defined; decode little-endian floats without unsafe casts.
        let values = if cfg!(target_endian = "little") {
            bytemuck::try_cast_slice::<u8, f32>(&mapped).ok().map(<[f32]>::to_vec)
        } else {
            None
        }
        .unwrap_or_else(|| mapped.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect());
        drop(mapped);
        drop(copying);
        staging.unmap();
        self.check()?;
        // Failure paths discard the buffer: never cache a pending or failed mapping.
        self.staging.lock().unwrap_or_else(std::sync::PoisonError::into_inner).put(staging);
        Image::from_shape_vec(frame.shape(), values).map_err(|e| Error::Input(e.to_string()))
    }

    /// Port of the synchronization in `native_gpu._call`; useful for measuring resident work without readback.
    pub fn wait(&self) -> Result<()> {
        self.check()?;
        self.device
            .poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(Duration::from_secs(30)) })
            .map_err(|e| Error::Device(e.to_string()))?;
        self.check()
    }

    pub(crate) fn dispatch(
        &self,
        source: &Frame,
        stage: usize,
        params: &[f32],
        table: &[[f32; 2]],
        aux: &[f32],
    ) -> Result<Frame> {
        let aux = self.upload_values(aux)?;
        self.dispatch_buffers(source, stage, params, table, [Some(&aux), None, None, None])
    }

    pub(crate) fn upload_values(&self, values: &[f32]) -> Result<wgpu::Buffer> {
        self.check()?;
        if values.len() as u64 * 4 > self.limits.max_storage_buffer_binding_size {
            return Err(Error::Limit("auxiliary buffer too large".into()));
        }
        self.buffer_init(
            "Grainy auxiliary",
            bytemuck::cast_slice(if values.is_empty() { &[0.0] } else { values }),
            wgpu::BufferUsages::STORAGE,
        )
    }

    /// `wgpu::util::DeviceExt::create_buffer_init` without its panic: when the device is lost (also before the
    /// device-lost callback has run, as WARP of Windows Server 2022 shows), the new buffer cannot be mapped and
    /// this is an error, so the caller falls back to the CPU.
    pub(crate) fn buffer_init(&self, label: &str, contents: &[u8], usage: wgpu::BufferUsages) -> Result<wgpu::Buffer> {
        let unpadded = contents.len() as wgpu::BufferAddress;
        let mask = wgpu::COPY_BUFFER_ALIGNMENT - 1;
        let size = ((unpadded + mask) & !mask).max(wgpu::COPY_BUFFER_ALIGNMENT);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage,
            mapped_at_creation: true,
        });
        {
            let mut view = buffer.get_mapped_range_mut(..).map_err(|e| Error::Device(format!("{label}: {e}")))?;
            view.slice(..unpadded as usize).copy_from_slice(contents);
        }
        buffer.unmap();
        Ok(buffer)
    }
}

#[derive(Clone)]
pub(crate) struct BufferSlice {
    buffer: wgpu::Buffer,
    offset: u64,
    size: Option<NonZeroU64>,
}

impl BufferSlice {
    pub(crate) fn entire(buffer: &wgpu::Buffer) -> Self {
        Self { buffer: buffer.clone(), offset: 0, size: None }
    }

    fn resource(&self) -> wgpu::BindingResource<'_> {
        wgpu::BindingResource::Buffer(wgpu::BufferBinding {
            buffer: &self.buffer,
            offset: self.offset,
            size: self.size,
        })
    }
}

/// Records multiple compute stages into a single `wgpu::CommandEncoder` and a pooled parameter slab,
/// submitting once with one `queue.submit()` while keeping intermediate frames isolated per batch.
pub(crate) struct Batch<'a> {
    pub(crate) gpu: &'a Gpu,
    encoder: wgpu::CommandEncoder,
    outputs: Arc<BatchPool>,
    slab: Option<wgpu::Buffer>,
    slab_cap: usize,
    slab_bytes: Vec<u8>,
    retired_slabs: Vec<(wgpu::Buffer, Vec<u8>)>,
    scratch: Option<wgpu::Buffer>,
    profiles: Vec<wgpu::Buffer>,
    pending: usize,
}

impl<'a> Batch<'a> {
    pub(crate) fn new(gpu: &'a Gpu) -> Self {
        Self {
            gpu,
            encoder: gpu.device.create_command_encoder(&Default::default()),
            outputs: BatchPool::new(&gpu.outputs),
            slab: None,
            slab_cap: 0,
            slab_bytes: Vec::new(),
            retired_slabs: Vec::new(),
            scratch: None,
            profiles: Vec::new(),
            pending: 0,
        }
    }

    fn write_slab(&mut self, bytes: &[u8]) -> Result<BufferSlice> {
        let len = bytes.len();
        if len as u64 > self.gpu.limits.max_storage_buffer_binding_size {
            return Err(Error::Limit("stage table is too large".into()));
        }
        let align = (self.gpu.limits.min_storage_buffer_offset_alignment as usize).max(4);
        let padded_len = ((len + 3) & !3).max(4);
        let aligned_off = (self.slab_bytes.len() + align - 1) & !(align - 1);
        if self.slab.is_none() || aligned_off + padded_len > self.slab_cap {
            if let Some(old) = self.slab.take() {
                self.retired_slabs.push((old, std::mem::take(&mut self.slab_bytes)));
            }
            let cap = SLAB_BYTES.max(((padded_len as u64 + 255) & !255).max(SLAB_BYTES));
            let desc = wgpu::BufferDescriptor {
                label: Some("Grainy parameter slab"),
                size: cap,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            };
            let buf = if cap == SLAB_BYTES {
                self.gpu.slabs.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take(&self.gpu.device, &desc)
            } else {
                self.gpu.device.create_buffer(&desc)
            };
            self.slab_cap = cap as usize;
            self.slab = Some(buf);
            self.slab_bytes.clear();
        }
        let offset = if self.slab_bytes.is_empty() {
            0
        } else {
            self.slab_bytes.resize(aligned_off, 0);
            aligned_off
        };
        self.slab_bytes.extend_from_slice(bytes);
        self.slab_bytes.resize(offset + padded_len, 0);
        Ok(BufferSlice {
            buffer: self.slab.as_ref().unwrap().clone(),
            offset: offset as u64,
            size: NonZeroU64::new(padded_len as u64),
        })
    }

    pub(crate) fn stage_table(&mut self, table: &[[f32; 2]]) -> Result<BufferSlice> {
        if table.is_empty() {
            return Ok(BufferSlice::entire(&self.gpu.empty_storage));
        }
        let bytes = bytemuck::cast_slice(table);
        if bytes.len() as u64 <= SLAB_BYTES {
            self.write_slab(bytes)
        } else {
            if bytes.len() as u64 > self.gpu.limits.max_storage_buffer_binding_size {
                return Err(Error::Limit("stage table is too large".into()));
            }
            let buf = self.gpu.buffer_init("curves and local maps", bytes, wgpu::BufferUsages::STORAGE)?;
            Ok(BufferSlice::entire(&buf))
        }
    }

    pub(crate) fn alloc_profile(&mut self) -> wgpu::Buffer {
        let buf = self.gpu.reduce_profiles.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take(
            &self.gpu.device,
            &wgpu::BufferDescriptor {
                label: Some("Grainy wavelet profile"),
                size: REDUCE_PROFILE_BYTES,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            },
        );
        self.profiles.push(buf.clone());
        buf
    }

    fn ensure_scratch(&mut self) -> wgpu::Buffer {
        self.scratch
            .get_or_insert_with(|| {
                self.gpu.reduce_scratch.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take(
                    &self.gpu.device,
                    &wgpu::BufferDescriptor {
                        label: Some("Grainy reduction scratch"),
                        size: REDUCE_SCRATCH_BYTES,
                        usage: wgpu::BufferUsages::STORAGE,
                        mapped_at_creation: false,
                    },
                )
            })
            .clone()
    }

    pub(crate) fn reduce_noise_profile(&mut self, energy: &Frame, profile: &wgpu::Buffer) -> Result<()> {
        self.gpu.check_frame(energy)?;
        let count = energy
            .width
            .checked_mul(energy.height)
            .and_then(|n| u32::try_from(n).ok())
            .filter(|&n| n > 0)
            .ok_or_else(|| Error::Limit("image size overflow".into()))?;
        let groups = count.div_ceil(256).clamp(1, 256);
        let inv = 1.0f64 / f64::from(count);
        let inv_hi = inv as f32;
        let inv_lo = (inv - f64::from(inv_hi)) as f32;
        let meta = [count, groups, inv_hi.to_bits(), inv_lo.to_bits()];
        let meta_slice = self.write_slab(bytemuck::cast_slice(&meta))?;
        let scratch = self.ensure_scratch();
        let bind = self.gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Grainy wavelet reduction"),
            layout: &self.gpu.reduce_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: energy.buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: scratch.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: profile.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: meta_slice.resource() },
            ],
        });
        for (pipeline, dispatch_x) in [(0, groups), (1, 1), (2, groups), (3, 1)] {
            let mut pass = self.encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Grainy reduce"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.gpu.reduce_pipelines[pipeline]);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(dispatch_x, 1, 1);
        }
        self.pending += 4;
        Ok(())
    }

    pub(crate) fn halve_noise_profile(&mut self, profile: &wgpu::Buffer) -> Result<()> {
        self.gpu.check()?;
        let scratch = self.ensure_scratch();
        let bind = self.gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Grainy wavelet halve"),
            layout: &self.gpu.reduce_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.gpu.empty_storage.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: scratch.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: profile.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: self.gpu.empty_storage.as_entire_binding() },
            ],
        });
        let mut pass = self
            .encoder
            .begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("Grainy halve"), timestamp_writes: None });
        pass.set_pipeline(&self.gpu.reduce_pipelines[4]);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(1, 1, 1);
        self.pending += 1;
        Ok(())
    }

    pub(crate) fn alloc_plane(&self, elems: usize) -> Result<wgpu::Buffer> {
        let bytes = (elems.max(1) as u64) * 4;
        if bytes > self.gpu.limits.max_storage_buffer_binding_size {
            return Err(Error::Limit("laplacian plane exceeds storage limit".into()));
        }
        Ok(self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grainy laplacian plane"),
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        }))
    }

    pub(crate) fn alloc_frame(&mut self, width: usize, height: usize) -> Result<Frame> {
        let bytes = self.gpu.bytes(height, width)?;
        let out = self.outputs.take(
            &self.gpu.device,
            &wgpu::BufferDescriptor {
                label: Some("Grainy resident frame"),
                size: bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            },
        );
        let lease = Lease::from_batch(&out, &self.outputs);
        Ok(Frame { buffer: out, width, height, owner: self.gpu.owner, _lease: Some(lease) })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn laplacian_dispatch(
        &mut self,
        pipeline: usize,
        src0: &wgpu::Buffer,
        src1: Option<&wgpu::Buffer>,
        src2: Option<&wgpu::Buffer>,
        dst: &wgpu::Buffer,
        lp: &[f32],
        width: usize,
        height: usize,
    ) -> Result<()> {
        let p_slice = self.write_slab(bytemuck::cast_slice(lp))?;
        let empty = &self.gpu.empty_storage;
        let bind = self.gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Grainy laplacian"),
            layout: &self.gpu.laplacian_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: src0.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: src1.unwrap_or(empty).as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: src2.unwrap_or(empty).as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: dst.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: p_slice.resource() },
            ],
        });
        let mut pass = self.encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Grainy laplacian"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.gpu.laplacian_pipelines[pipeline]);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(width.div_ceil(16) as u32, height.div_ceil(16) as u32, 1);
        self.pending += 1;
        Ok(())
    }

    pub(crate) fn dispatch_with_table(
        &mut self,
        source: &Frame,
        stage: usize,
        params: &[f32],
        table: &BufferSlice,
        extra: [Option<&wgpu::Buffer>; 4],
    ) -> Result<Frame> {
        self.gpu.check_frame(source)?;
        let bytes = self.gpu.bytes(source.height, source.width)?;
        let p_slice = self.write_slab(bytemuck::cast_slice(if params.is_empty() { &[0.0] } else { params }))?;
        let out = self.outputs.take(
            &self.gpu.device,
            &wgpu::BufferDescriptor {
                label: Some("Grainy resident frame"),
                size: bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            },
        );
        let a = &self.gpu.empty_storage;
        let bind = self.gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Grainy stage"),
            layout: &self.gpu.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: source.buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: out.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: p_slice.resource() },
                wgpu::BindGroupEntry { binding: 3, resource: table.resource() },
                wgpu::BindGroupEntry { binding: 4, resource: extra[0].unwrap_or(a).as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: extra[1].unwrap_or(a).as_entire_binding() },
                wgpu::BindGroupEntry { binding: 6, resource: extra[2].unwrap_or(a).as_entire_binding() },
                wgpu::BindGroupEntry { binding: 7, resource: extra[3].unwrap_or(a).as_entire_binding() },
            ],
        });
        {
            let mut pass = self
                .encoder
                .begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("Grainy"), timestamp_writes: None });
            pass.set_pipeline(&self.gpu.pipelines[stage]);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(source.width.div_ceil(16) as u32, source.height.div_ceil(16) as u32, 1);
        }
        self.pending += 1;
        let lease = Lease::from_batch(&out, &self.outputs);
        Ok(Frame {
            buffer: out,
            width: source.width,
            height: source.height,
            owner: self.gpu.owner,
            _lease: Some(lease),
        })
    }

    pub(crate) fn dispatch_buffers(
        &mut self,
        source: &Frame,
        stage: usize,
        params: &[f32],
        table: &[[f32; 2]],
        extra: [Option<&wgpu::Buffer>; 4],
    ) -> Result<Frame> {
        let table_slice = self.stage_table(table)?;
        self.dispatch_with_table(source, stage, params, &table_slice, extra)
    }

    pub(crate) fn dispatch_sized(
        &mut self,
        source: &Frame,
        stage: usize,
        out_width: usize,
        out_height: usize,
        params: &[f32],
    ) -> Result<Frame> {
        self.gpu.check_frame(source)?;
        let bytes = self.gpu.bytes(out_height, out_width)?;
        let p_slice = self.write_slab(bytemuck::cast_slice(if params.is_empty() { &[0.0] } else { params }))?;
        let out = self.outputs.take(
            &self.gpu.device,
            &wgpu::BufferDescriptor {
                label: Some("Grainy resized frame"),
                size: bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            },
        );
        let a = &self.gpu.empty_storage;
        let bind = self.gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Grainy sized stage"),
            layout: &self.gpu.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: source.buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: out.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: p_slice.resource() },
                wgpu::BindGroupEntry { binding: 3, resource: a.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: a.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: a.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 6, resource: a.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 7, resource: a.as_entire_binding() },
            ],
        });
        {
            let mut pass = self
                .encoder
                .begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("Grainy"), timestamp_writes: None });
            pass.set_pipeline(&self.gpu.pipelines[stage]);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(out_width.div_ceil(16) as u32, out_height.div_ceil(16) as u32, 1);
        }
        self.pending += 1;
        let lease = Lease::from_batch(&out, &self.outputs);
        Ok(Frame { buffer: out, width: out_width, height: out_height, owner: self.gpu.owner, _lease: Some(lease) })
    }

    pub(crate) fn dispatch(
        &mut self,
        source: &Frame,
        stage: usize,
        params: &[f32],
        table: &[[f32; 2]],
        aux: &[f32],
    ) -> Result<Frame> {
        if aux.is_empty() {
            self.dispatch_buffers(source, stage, params, table, [None; 4])
        } else {
            let aux = self.gpu.upload_values(aux)?;
            self.dispatch_buffers(source, stage, params, table, [Some(&aux), None, None, None])
        }
    }

    fn return_resources(&mut self) {
        self.outputs.commit();
        if let Some(slab) = self.slab.take() {
            if slab.size() == SLAB_BYTES {
                self.gpu.slabs.lock().unwrap_or_else(std::sync::PoisonError::into_inner).put(slab);
            }
        }
        for (slab, _) in self.retired_slabs.drain(..) {
            if slab.size() == SLAB_BYTES {
                self.gpu.slabs.lock().unwrap_or_else(std::sync::PoisonError::into_inner).put(slab);
            }
        }
        if let Some(scratch) = self.scratch.take() {
            self.gpu.reduce_scratch.lock().unwrap_or_else(std::sync::PoisonError::into_inner).put(scratch);
        }
        if !self.profiles.is_empty() {
            let mut pool = self.gpu.reduce_profiles.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            for buf in self.profiles.drain(..) {
                pool.put(buf);
            }
        }
        self.slab_bytes.clear();
        self.slab_cap = 0;
    }

    /// Submit all recorded passes to the GPU queue in a single submission and reset the batch for further use.
    pub(crate) fn flush(&mut self) -> Result<()> {
        if self.pending == 0 {
            self.return_resources();
            return self.gpu.check();
        }
        self.gpu.check()?;
        for (slab, bytes) in &self.retired_slabs {
            if !bytes.is_empty() {
                self.gpu.queue.write_buffer(slab, 0, bytes);
            }
        }
        if let Some(slab) = &self.slab {
            if !self.slab_bytes.is_empty() {
                self.gpu.queue.write_buffer(slab, 0, &self.slab_bytes);
            }
        }
        let next_encoder = self.gpu.device.create_command_encoder(&Default::default());
        let encoder = std::mem::replace(&mut self.encoder, next_encoder);
        self.gpu.queue.submit([encoder.finish()]);
        self.pending = 0;
        self.return_resources();
        self.outputs = BatchPool::new(&self.gpu.outputs);
        let _ = self.gpu.device.poll(wgpu::PollType::Poll);
        self.gpu.check()
    }

    /// Submit all recorded passes and consume the batch.
    pub(crate) fn submit(mut self) -> Result<()> {
        self.flush()
    }
}

impl Drop for Batch<'_> {
    fn drop(&mut self) {
        self.return_resources();
    }
}

impl Gpu {
    pub(crate) fn batch(&self) -> Batch<'_> {
        Batch::new(self)
    }

    pub(crate) fn dispatch_buffers(
        &self,
        source: &Frame,
        stage: usize,
        params: &[f32],
        table: &[[f32; 2]],
        extra: [Option<&wgpu::Buffer>; 4],
    ) -> Result<Frame> {
        let mut batch = self.batch();
        let frame = batch.dispatch_buffers(source, stage, params, table, extra)?;
        batch.submit()?;
        Ok(frame)
    }

    /// Port of `native_gpu.tone`: upload, tone compute and readback, or None for a CPU-only setting.
    pub fn tone(&self, image: &Image, s: &Settings) -> Result<Option<Image>> {
        let frame = self.upload(image)?;
        self.tone_frame(&frame, s, image)?.map(|out| self.readback(&out)).transpose()
    }

    /// Port of the tone slot of `native_gpu.develop_resident`; `linear` must be the image uploaded to `frame`.
    /// It supplies process-3 analysis only and is not uploaded again.
    pub fn tone_frame(&self, frame: &Frame, s: &Settings, linear: &Image) -> Result<Option<Frame>> {
        self.check_frame(frame)?;
        if linear.dim() != frame.shape() {
            return Err(Error::Input("tone analysis frame shape differs".into()));
        }
        if params::needs_local(s) && !linear.iter().all(|v| v.is_finite()) {
            return Err(Error::Input("non-finite tone analysis pixels".into()));
        }
        if grainy_film::active(s) {
            let mut batch = self.batch();
            let Some(encoded) = self.film_encode_in_batch(&mut batch, frame, s, None)? else {
                return Ok(None);
            };
            let base = if params::needs_local(s) {
                let name = s.str("film_look").to_owned();
                let space = grainy_film::WorkingSpace::from_setting(s.str("working_space"));
                let render = move |a: ndarray::ArrayView3<'_, f32>| {
                    grainy_film::render(&a.to_owned(), &name, space).unwrap_or_else(|_| a.to_owned())
                };
                Some(grainy_tone::tone_base(linear, s, Some(&render)).map_err(|e| Error::Input(e.to_string()))?)
            } else {
                None
            };
            let Some((p, t)) = params::tone(s, linear, (frame.width, frame.height), base.as_ref(), None, true)? else {
                return Ok(None);
            };
            let out = batch.dispatch(&encoded, 0, &p, &t, &[])?;
            batch.submit()?;
            return Ok(Some(out));
        }
        let Some((p, t)) = params::tone(s, linear, (frame.width, frame.height), None, None, false)? else {
            return Ok(None);
        };
        self.dispatch(frame, 0, &p, &t, &[]).map(Some)
    }

    /// Port of `native_gpu.color`: encoded RGB colour tools, or None for HDR, photo filters or unsupported parameters.
    pub fn color(&self, image: &Image, s: &Settings) -> Result<Option<Image>> {
        let frame = self.upload(image)?;
        self.color_frame(&frame, s)?.map(|out| self.readback(&out)).transpose()
    }

    /// Port of the colour slot of `native_gpu.develop_resident`, without a host round trip.
    pub fn color_frame(&self, frame: &Frame, s: &Settings) -> Result<Option<Frame>> {
        self.check_frame(frame)?;
        let Some((p, t)) = params::color(s, frame.width, frame.height)? else {
            return Ok(None);
        };
        self.dispatch(frame, 1, &p, &t, &[]).map(Some)
    }

    /// Port of `native_gpu.develop_resident` grain addition and final clip, with the oracle's CPU-generated field.
    pub fn grain_clip(&self, frame: &Frame, grain: Option<&Plane>) -> Result<Frame> {
        self.check_frame(frame)?;
        if grain.is_some_and(|g| g.dim() != (frame.height, frame.width) || g.iter().any(|v| !v.is_finite())) {
            return Err(Error::Input("grain field must be finite and match the frame".into()));
        }
        let values: Vec<f32> = grain.map_or_else(Vec::new, |g| g.iter().copied().collect());
        self.dispatch(
            frame,
            2,
            &[frame.width as f32, frame.height as f32, f32::from(grain.is_some()), 0.0],
            &[],
            &values,
        )
    }

    /// Port of `native_gpu.develop`, including the resident chain's grain and final clip. None requests CPU processing.
    pub fn develop(&self, image: &Image, s: &Settings, pixel_scale: f64) -> Result<Option<Image>> {
        let mut resident = Resident::new(self, image)?;
        self.develop_resident(&mut resident, s, pixel_scale)?.map(|out| self.readback(&out)).transpose()
    }

    #[cfg(test)]
    pub(crate) fn readback_profile(&self, profile: &wgpu::Buffer) -> Result<[f32; 8]> {
        self.check()?;
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grainy profile readback"),
            size: REDUCE_PROFILE_BYTES,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(profile, 0, &staging, 0, REDUCE_PROFILE_BYTES);
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = mpsc::sync_channel(1);
        staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.wait()?;
        rx.recv_timeout(Duration::from_secs(30))
            .map_err(|e| Error::Device(e.to_string()))?
            .map_err(|e| Error::Device(e.to_string()))?;
        let mapped = staging.slice(..).get_mapped_range().map_err(|e| Error::Device(e.to_string()))?;
        let mut out = [0f32; 8];
        for (i, slot) in out.iter_mut().enumerate() {
            let off = i * 8;
            *slot = f32::from_le_bytes(mapped[off..off + 4].try_into().unwrap());
        }
        drop(mapped);
        staging.unmap();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_adapters_return_none_for_both_policies() {
        for kind in [AdapterKind::Hardware, AdapterKind::Software] {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::empty(),
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            assert!(Gpu::with_instance(kind, instance).unwrap().is_none());
        }
    }

    #[test]
    fn pooled_frames_preserve_clones_queue_order_and_readbacks() {
        let Some(g) = Gpu::with_adapter(AdapterKind::Hardware).expect("GPU initialization") else {
            eprintln!("SKIP pool test: no adapter");
            return;
        };
        let image = grainy_testkit::input("colours");
        let mut resident = Resident::new(&g, &image).unwrap();
        let mut s = grainy_core::defaults();
        // Exercise reuse between actual shader dispatches; neutral colour aliases the tone buffer.
        s.set("saturation", 24.0);
        let original = g.develop_resident(&mut resident, &s, 1.0).unwrap().unwrap();
        let original_pixels = g.readback(&original).unwrap();
        let clone = original.clone();
        drop(original);

        // Submit several slider updates without waiting. A retained frame must never be recycled.
        let mut last = None;
        for i in 0..12 {
            s.set("exposure", 0.05 * i as f64);
            last = g.develop_resident(&mut resident, &s, 1.0).unwrap();
        }
        let last = last.unwrap();
        let tone = grainy_tone::develop_tone(&image, &s, None).unwrap();
        let expected = grainy_colorgrade::develop_color(&tone, &s).unwrap();
        grainy_testkit::assert_close(&g.readback(&last).unwrap(), &expected, 1e-5, "pooled queued chain");
        assert_eq!(g.readback(&clone).unwrap(), original_pixels);

        // Holding `last` is one additional live frame compared with the loop above.
        for i in 0..3 {
            s.set("exposure", -1.0 - 0.05 * i as f64);
            g.develop_resident(&mut resident, &s, 1.0).unwrap();
        }
        let outputs = g.outputs.lock().unwrap();
        let allocated = outputs.allocations;
        assert!(outputs.reuses > 20);
        drop(outputs);
        let staging = g.staging.lock().unwrap();
        assert_eq!(staging.allocations, 1);
        assert_eq!(staging.reuses, 2);
        drop(staging);

        // Repeating the same-size workload reaches a stable allocation count.
        for i in 0..8 {
            s.set("exposure", -0.05 * i as f64);
            g.develop_resident(&mut resident, &s, 1.0).unwrap();
        }
        assert_eq!(g.outputs.lock().unwrap().allocations, allocated);
        std::thread::scope(|scope| {
            let first = scope.spawn(|| g.readback(&clone).unwrap());
            let second = scope.spawn(|| g.readback(&last).unwrap());
            assert_eq!(first.join().unwrap(), original_pixels);
            grainy_testkit::assert_close(&second.join().unwrap(), &expected, 1e-5, "concurrent pooled readback");
        });

        // Change the image dimensions while old frames and a second resident remain alive.
        let small = Image::from_elem((3, 5, 3), 0.25);
        let mut second = Resident::new(&g, &small).unwrap();
        let defaults = grainy_core::defaults();
        let small_frame = g.develop_resident(&mut second, &defaults, 1.0).unwrap().unwrap();
        resident.replace(&g, &small).unwrap();
        let replaced = g.develop_resident(&mut resident, &defaults, 1.0).unwrap().unwrap();
        assert_eq!(g.readback(&small_frame).unwrap(), g.readback(&replaced).unwrap());
        assert_eq!(g.readback(&clone).unwrap(), original_pixels);
        assert_eq!(g.readback(&clone).unwrap().dim(), image.dim());
    }

    #[test]
    fn a_device_lost_before_its_callback_runs_gives_errors_not_panics() {
        // WARP of Windows Server 2022 removes the device while a detail stage runs; the next stage then created
        // its buffers on the lost device before the device-lost callback had run, and wgpu's `create_buffer_init`
        // panicked. Every entry point must instead return an error, so the preview falls back to the CPU.
        let Some(g) = Gpu::with_adapter(AdapterKind::Hardware).expect("GPU initialization") else {
            eprintln!("SKIP device-lost test: no adapter");
            return;
        };
        let image = grainy_testkit::input("ramp");
        let frame = g.upload(&image).unwrap();
        let mut noise = grainy_core::defaults();
        noise.set("noise_color", serde_json::json!(50.0));
        g.device.destroy();
        assert!(g.buffer_init("parameters", &[0; 16], wgpu::BufferUsages::STORAGE).is_err());
        assert!(g.upload(&image).is_err());
        assert!(g.upload_values(&[1.0, 2.0]).is_err());
        assert!(g.dispatch(&frame, 0, &[0.0; 4], &[], &[]).is_err());
        assert!(g.detail(&image, &noise, 1.0).is_err());
        assert!(g.develop(&image, &grainy_core::defaults(), 1.0).is_err());
        assert!(g.readback(&frame).is_err());
    }

    #[test]
    fn destroyed_device_returns_errors_and_new_context_recovers() {
        let Some(g) = Gpu::with_adapter(AdapterKind::Hardware).expect("GPU initialization") else {
            eprintln!("SKIP device-lost test: no adapter");
            return;
        };
        let image = grainy_testkit::input("ramp");
        let frame = g.upload(&image).unwrap();
        g.device.destroy();
        let _ = g.device.poll(wgpu::PollType::Poll);
        assert!(g.readback(&frame).is_err());
        assert!(g.upload(&image).is_err());
        let replacement = Gpu::with_adapter(AdapterKind::Hardware).unwrap().unwrap();
        assert_eq!(replacement.readback(&replacement.upload(&image).unwrap()).unwrap(), image);
        assert!(replacement.readback(&frame).is_err());
    }
}
