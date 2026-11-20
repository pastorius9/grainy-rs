use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::Duration;

use grainy_core::{Image, Plane, Settings};

use crate::pool::{Lease, Pool};
use crate::{params, Error, Resident, Result};

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
    queue: wgpu::Queue,
    info: wgpu::AdapterInfo,
    limits: wgpu::Limits,
    owner: u64,
    failed: Arc<Mutex<Option<String>>>,
    pipelines: Vec<wgpu::ComputePipeline>,
    layout: wgpu::BindGroupLayout,
    outputs: Arc<Mutex<Pool>>,
    staging: Arc<Mutex<Pool>>,
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
        let failed = Arc::new(Mutex::new(None));
        let state = failed.clone();
        device.set_device_lost_callback(move |reason, message| {
            *state.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(format!("device lost ({reason:?}): {message}"));
        });
        let state = failed.clone();
        device.on_uncaptured_error(Arc::new(move |e| {
            *state.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(format!("{e}"));
        }));
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
        for (name, code) in [
            ("tone", include_str!("tone.wgsl")),
            ("color", include_str!("color.wgsl")),
            ("finish", include_str!("finish.wgsl")),
            ("detail", include_str!("detail.wgsl")),
        ] {
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(name),
                source: wgpu::ShaderSource::Wgsl(format!("{}\n{code}", include_str!("common.wgsl")).into()),
            });
            let stages = if name == "detail" { 32 } else { 1 };
            for stage in 0..stages {
                let constants = [("detail_stage", stage as f64)];
                pipelines.push(device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(&format!("{name}/{stage}")),
                    layout: Some(&pipeline_layout),
                    module: &shader,
                    entry_point: Some("main"),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants: if name == "detail" { &constants } else { &[] },
                        ..Default::default()
                    },
                    cache: None,
                }));
            }
        }
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(Error::Device(e.to_string()));
        }
        let gpu = Self {
            device,
            queue,
            info,
            limits,
            owner: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            failed,
            pipelines,
            layout,
            outputs: Pool::new(1024 * 1024 * 1024, 16),
            staging: Pool::new(512 * 1024 * 1024, 2),
        };
        gpu.check()?;
        Ok(Some(gpu))
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
        self.wait()?;
        rx.recv_timeout(Duration::from_secs(30))
            .map_err(|e| Error::Device(e.to_string()))?
            .map_err(|e| Error::Device(e.to_string()))?;
        let mapped = staging.slice(..).get_mapped_range().map_err(|e| Error::Device(e.to_string()))?;
        // Mapping alignment is implementation-defined; decode little-endian floats without unsafe casts.
        let values = mapped.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
        drop(mapped);
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

    pub(crate) fn dispatch_buffers(
        &self,
        source: &Frame,
        stage: usize,
        params: &[f32],
        table: &[[f32; 2]],
        extra: [Option<&wgpu::Buffer>; 4],
    ) -> Result<Frame> {
        self.check_frame(source)?;
        let bytes = self.bytes(source.height, source.width)?;
        for len in [params.len() * 4, table.len() * 8] {
            if len as u64 > self.limits.max_storage_buffer_binding_size {
                return Err(Error::Limit("stage table is too large".into()));
            }
        }
        let out = self.outputs.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take(
            &self.device,
            &wgpu::BufferDescriptor {
                label: Some("Grainy resident frame"),
                size: bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            },
        );
        let buffer = |name, contents: &[u8]| self.buffer_init(name, contents, wgpu::BufferUsages::STORAGE);
        let p = buffer("parameters", bytemuck::cast_slice(params))?;
        let t = buffer(
            "curves and local maps",
            bytemuck::cast_slice(if table.is_empty() { &[[0.0, 0.0]] } else { table }),
        )?;
        let a = buffer("unused auxiliary", bytemuck::cast_slice(&[0f32]))?;
        let entries: Vec<_> = [
            &source.buffer,
            &out,
            &p,
            &t,
            extra[0].unwrap_or(&a),
            extra[1].unwrap_or(&a),
            extra[2].unwrap_or(&a),
            extra[3].unwrap_or(&a),
        ]
        .into_iter()
        .enumerate()
        .map(|(binding, b)| wgpu::BindGroupEntry { binding: binding as u32, resource: b.as_entire_binding() })
        .collect();
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Grainy stage"),
            layout: &self.layout,
            entries: &entries,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder
                .begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("Grainy"), timestamp_writes: None });
            pass.set_pipeline(&self.pipelines[stage]);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(source.width.div_ceil(16) as u32, source.height.div_ceil(16) as u32, 1);
        }
        self.queue.submit([encoder.finish()]);
        self.check()?;
        let lease = Lease::new(&out, &self.outputs);
        Ok(Frame { buffer: out, width: source.width, height: source.height, owner: self.owner, _lease: Some(lease) })
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
        let Some((p, t)) = params::tone(s, linear, None)? else {
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
