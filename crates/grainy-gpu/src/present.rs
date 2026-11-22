//! Zero-copy GPU presentation: optional 33^3 tetrahedral monitor ICC 3D LUT, Rgba8Unorm storage
//! texture output, exact 128x3 histogram + channel clipping flags (1.5 KB readback), and on-demand
//! GPU clipping overlay generation.

use std::sync::{mpsc, Arc};
use std::time::Duration;

use crate::{Error, Frame, Gpu, Result};

/// Edge size of the tetrahedral 3D display LUT (33^3 = 35,937 entries).
pub const LUT_SIZE: usize = 33;
/// Number of `vec4<f32>` entries in one 33^3 3D LUT.
pub const LUT_ENTRIES: usize = LUT_SIZE * LUT_SIZE * LUT_SIZE;
const STATS_U32S: usize = 385;
const STATS_BYTES: u64 = (STATS_U32S * 4) as u64;

/// A 33^3 monitor (and optional working->sRGB) 3D LUT uploaded to GPU memory.
#[derive(Clone, Debug)]
pub struct MonitorLut {
    pub(crate) buffer: wgpu::Buffer,
    pub(crate) has_display: bool,
    pub(crate) has_srgb: bool,
    owner: u64,
}

/// Optional sub-rectangle inside a padded GPU frame (used to trim 1:1 tile halos on GPU).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentCrop {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Result of a zero-copy GPU presentation pass: the RGBA8 texture lives solely on the GPU,
/// while only the 1,540-byte histogram & channel clipping stats are read back to the CPU.
#[derive(Clone, Debug)]
pub struct Presented {
    pub texture: Arc<wgpu::Texture>,
    pub view: Arc<wgpu::TextureView>,
    pub clip_bits: Arc<wgpu::Buffer>,
    pub width: usize,
    pub height: usize,
    pub bins: [[u32; 128]; 3],
    pub channels: ([bool; 3], [bool; 3]),
}

/// Build the 33^3 grid sample coordinates `[r, g, b]` in `[0, 1]` (len = `35,937 * 3`)
/// for feeding into LittleCMS (`grainy_cms::native_convert`).
pub fn lut33_input_grid() -> Vec<f32> {
    let mut out = Vec::with_capacity(LUT_ENTRIES * 3);
    let step = 1.0 / (LUT_SIZE - 1) as f32;
    for r in 0..LUT_SIZE {
        let rf = if r + 1 == LUT_SIZE { 1.0 } else { r as f32 * step };
        for g in 0..LUT_SIZE {
            let gf = if g + 1 == LUT_SIZE { 1.0 } else { g as f32 * step };
            for b in 0..LUT_SIZE {
                let bf = if b + 1 == LUT_SIZE { 1.0 } else { b as f32 * step };
                out.push(rf);
                out.push(gf);
                out.push(bf);
            }
        }
    }
    out
}

/// Pack flat RGB `f32` LUT values (`LUT_ENTRIES * 3`) into `vec4<f32>` entries (`[r, g, b, 1.0]`).
pub fn pack_lut33(rgb: &[f32]) -> Result<Vec<[f32; 4]>> {
    if rgb.len() != LUT_ENTRIES * 3 {
        return Err(Error::Input(format!("expected {} LUT floats, got {}", LUT_ENTRIES * 3, rgb.len())));
    }
    Ok(rgb.as_chunks::<3>().0.iter().map(|c| [c[0], c[1], c[2], 1.0]).collect())
}

impl Gpu {
    /// Upload a 33^3 display 3D LUT (and optional second 33^3 working->sRGB LUT for wide working
    /// spaces such as ProPhoto) to GPU storage.
    pub fn upload_monitor_lut(&self, display_rgb: Option<&[f32]>, srgb_rgb: Option<&[f32]>) -> Result<MonitorLut> {
        self.check()?;
        let has_display = display_rgb.is_some();
        let has_srgb = srgb_rgb.is_some();
        let mut entries = if let Some(rgb) = display_rgb {
            pack_lut33(rgb)?
        } else if has_srgb {
            vec![[0.0; 4]; LUT_ENTRIES]
        } else {
            vec![[0.0; 4]; 1]
        };
        if let Some(rgb) = srgb_rgb {
            entries.extend(pack_lut33(rgb)?);
        }
        let buffer =
            self.buffer_init("Grainy monitor 3D LUT", bytemuck::cast_slice(&entries), wgpu::BufferUsages::STORAGE)?;
        Ok(MonitorLut { buffer, has_display, has_srgb, owner: self.owner })
    }

    /// Convert a GPU-resident RGB float32 frame directly into a GPU `Rgba8Unorm` texture (for
    /// zero-copy binding in `egui_wgpu`), applying an optional 33^3 tetrahedral monitor ICC LUT,
    /// computing exact 128x3 histogram bins, channel clipping flags, and packed 2-bit clip mask.
    ///
    /// Only 1,540 bytes (histogram + 6 channel clip bits) are read back to the CPU.
    pub fn present_frame(
        &self,
        frame: &Frame,
        lut: Option<&MonitorLut>,
        crop: Option<PresentCrop>,
        compute_histogram: bool,
    ) -> Result<Presented> {
        let _span = grainy_core::timing::Span::new("gpu.present");
        self.check_frame(frame)?;
        if let Some(lut) = lut {
            if lut.owner != self.owner {
                return Err(Error::ForeignFrame);
            }
        }
        let (crop_x, crop_y, out_w, out_h) = match crop {
            Some(c) => {
                if c.width == 0
                    || c.height == 0
                    || (c.x as usize + c.width as usize) > frame.width
                    || (c.y as usize + c.height as usize) > frame.height
                {
                    return Err(Error::Input("present crop out of frame bounds".into()));
                }
                (c.x, c.y, c.width, c.height)
            }
            None => (0, 0, frame.width as u32, frame.height as u32),
        };
        if out_w > self.limits.max_texture_dimension_2d || out_h > self.limits.max_texture_dimension_2d {
            return Err(Error::Limit(format!(
                "present texture {out_w}x{out_h} exceeds max_texture_dimension_2d {}",
                self.limits.max_texture_dimension_2d
            )));
        }

        let texture = Arc::new(self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Grainy zero-copy preview"),
            size: wgpu::Extent3d { width: out_w, height: out_h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        }));
        let view = Arc::new(texture.create_view(&wgpu::TextureViewDescriptor::default()));

        let stride_u32 = out_w.div_ceil(16);
        let clip_bytes = ((stride_u32 as u64) * (out_h as u64) * 4).max(16);
        let clip_bits = Arc::new(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grainy clip bits"),
            size: clip_bytes,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        }));

        let stats_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grainy present stats"),
            size: STATS_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grainy present stats readback"),
            size: STATS_BYTES,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let hist_step = if compute_histogram { (out_h / 300).max(1) } else { 0 };
        let mut flags = 2u32; // write packed 2-bit clip_bits
        if lut.is_some_and(|l| l.has_display) {
            flags |= 1;
        }
        if lut.is_some_and(|l| l.has_srgb) {
            flags |= 4;
        }
        let info_words: [u32; 8] =
            [frame.width as u32, frame.height as u32, crop_x, crop_y, out_w, out_h, hist_step, flags];
        let info_buf =
            self.buffer_init("Grainy present info", bytemuck::cast_slice(&info_words), wgpu::BufferUsages::STORAGE)?;

        let lut_buf = lut.map_or(&self.empty_lut, |l| &l.buffer);
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Grainy present"),
            layout: &self.present_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: frame.buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: lut_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: info_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 4, resource: clip_bits.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: stats_buf.as_entire_binding() },
            ],
        });

        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.clear_buffer(&stats_buf, 0, None);
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.present_pipelines[0]);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(out_w.div_ceil(16), out_h.div_ceil(16), 1);
        }
        encoder.copy_buffer_to_buffer(&stats_buf, 0, &staging, 0, STATS_BYTES);
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
        let words: Vec<u32> = mapped.as_chunks::<4>().0.iter().map(|b| u32::from_le_bytes(*b)).collect();
        drop(mapped);
        staging.unmap();
        self.check()?;

        let mut bins = [[0u32; 128]; 3];
        for c in 0..3 {
            bins[c].copy_from_slice(&words[c * 128..(c + 1) * 128]);
        }
        let chan = words[384];
        let channels = (
            [(chan & 1) != 0, (chan & 2) != 0, (chan & 4) != 0],
            [(chan & 8) != 0, (chan & 16) != 0, (chan & 32) != 0],
        );

        Ok(Presented { texture, view, clip_bits, width: out_w as usize, height: out_h as usize, bins, channels })
    }

    /// Generate a premultiplied-alpha `Rgba8Unorm` clipping overlay texture on the GPU directly
    /// from the packed 2-bit clip buffer produced by [`Self::present_frame`].
    pub fn render_clip_overlay(
        &self,
        clip_bits: &wgpu::Buffer,
        width: usize,
        height: usize,
        shadows: bool,
        highlights: bool,
    ) -> Result<(Arc<wgpu::Texture>, Arc<wgpu::TextureView>)> {
        self.check()?;
        let (out_w, out_h) = (width as u32, height as u32);
        let texture = Arc::new(self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Grainy GPU clipping overlay"),
            size: wgpu::Extent3d { width: out_w, height: out_h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        }));
        let view = Arc::new(texture.create_view(&wgpu::TextureViewDescriptor::default()));
        let info_words: [u32; 8] = [out_w, out_h, u32::from(shadows), u32::from(highlights), 0, 0, 0, 0];
        let info_buf = self.buffer_init(
            "Grainy clip overlay info",
            bytemuck::cast_slice(&info_words),
            wgpu::BufferUsages::STORAGE,
        )?;
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Grainy clip overlay"),
            layout: &self.present_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.empty_storage.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.empty_lut.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: info_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 4, resource: clip_bits.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: self.empty_stats.as_entire_binding() },
            ],
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.present_pipelines[1]);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(out_w.div_ceil(16), out_h.div_ceil(16), 1);
        }
        self.queue.submit([encoder.finish()]);
        Ok((texture, view))
    }

    /// On-demand readback of an `Rgba8Unorm` GPU texture into tightly packed RGBA8 bytes
    /// (used only when an interactive picker such as the eyedropper or loupe samples the frame).
    pub fn readback_texture_rgba(&self, texture: &wgpu::Texture, width: usize, height: usize) -> Result<Vec<u8>> {
        self.check()?;
        let unpadded_bpr = width * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
        let padded_bpr = unpadded_bpr.div_ceil(align) * align;
        let size = (padded_bpr * height) as u64;
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grainy RGBA texture readback"),
            size: size.max(16),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bpr as u32),
                    rows_per_image: Some(height as u32),
                },
            },
            wgpu::Extent3d { width: width as u32, height: height as u32, depth_or_array_layers: 1 },
        );
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
        let mut out = vec![0u8; unpadded_bpr * height];
        for y in 0..height {
            out[y * unpadded_bpr..(y + 1) * unpadded_bpr]
                .copy_from_slice(&mapped[y * padded_bpr..y * padded_bpr + unpadded_bpr]);
        }
        drop(mapped);
        staging.unmap();
        self.check()?;
        Ok(out)
    }

    /// Reads a single RGBA8 pixel `(x, y)` from a presented zero-copy GPU texture (256-byte staging copy,
    /// `< 0.05 ms`), used for live cursor `R / G / B %` readout without reading back the full frame.
    pub fn read_pixel_rgba8(&self, texture: &wgpu::Texture, x: u32, y: u32) -> Result<[u8; 4]> {
        self.check()?;
        let size = texture.size();
        if x >= size.width || y >= size.height {
            return Err(Error::Input("pixel coordinates out of bounds".into()));
        }
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as u64;
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grainy 1x1 pixel readback"),
            size: align,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Grainy 1x1 pixel copy") });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(align as u32),
                    rows_per_image: Some(1),
                },
            },
            wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = mpsc::sync_channel(1);
        staging.slice(..4).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.wait()?;
        rx.recv_timeout(Duration::from_secs(5))
            .map_err(|e| Error::Device(e.to_string()))?
            .map_err(|e| Error::Device(e.to_string()))?;
        let mapped = staging.slice(..4).get_mapped_range().map_err(|e| Error::Device(e.to_string()))?;
        let px = [mapped[0], mapped[1], mapped[2], mapped[3]];
        drop(mapped);
        staging.unmap();
        Ok(px)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AdapterKind;
    use grainy_core::Image;

    #[test]
    fn gpu_present_matches_cpu_rgba_bins_clipping_and_overlay() {
        let Some(gpu) = Gpu::with_adapter(AdapterKind::Hardware).expect("GPU init") else {
            return;
        };
        let (h, w) = (317, 421);
        let mut seed = 0x1234_5678u32;
        let image = Image::from_shape_fn((h, w, 3), |(y, x, c)| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            if y == 0 && x < 5 {
                0.0
            } else if y == h - 1 && x >= w - 5 {
                1.0
            } else if y == 1 && x == 0 && c == 1 {
                0.001 // shadow clip on G
            } else if y == 2 && x == 0 && c == 2 {
                0.999 // highlight clip on B
            } else {
                (seed as f32 / u32::MAX as f32) * 0.96 + 0.02
            }
        });
        let frame = gpu.upload(&image).unwrap();
        let presented = gpu.present_frame(&frame, None, None, true).unwrap();
        assert_eq!((presented.width, presented.height), (w, h));
        let gpu_rgba = gpu.readback_texture_rgba(&presented.texture, w, h).unwrap();

        let to_byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        let low = (1.0f64 / 510.0) as f32;
        let high = (1.0f64 - 1.0f64 / 510.0) as f32;
        let mut cpu_rgba = vec![255u8; w * h * 4];
        let mut cpu_bins = [[0u32; 128]; 3];
        let mut cpu_chan = ([false; 3], [false; 3]);
        let step = (h / 300).max(1);
        for y in 0..h {
            for x in 0..w {
                let at = (y * w + x) * 4;
                for c in 0..3 {
                    let v = image[[y, x, c]];
                    let b = to_byte(v);
                    cpu_rgba[at + c] = b;
                    if v < low {
                        cpu_chan.0[c] = true;
                    }
                    if v >= high {
                        cpu_chan.1[c] = true;
                    }
                    if y % step == 0 && x % step == 0 {
                        cpu_bins[c][usize::from(b >> 1)] += 1;
                    }
                }
            }
        }
        assert_eq!(gpu_rgba, cpu_rgba, "GPU RGBA8 must match CPU byte-for-byte");
        assert_eq!(presented.bins, cpu_bins, "GPU histogram bins must match CPU identically");
        assert_eq!(presented.channels, cpu_chan, "GPU clipping channel flags must match CPU");

        // Verify crop trimming (for 1:1 halo tiles) and clipping overlay texture generation.
        let crop = PresentCrop { x: 17, y: 13, width: 100, height: 80 };
        let cropped = gpu.present_frame(&frame, None, Some(crop), false).unwrap();
        let cropped_rgba = gpu.readback_texture_rgba(&cropped.texture, 100, 80).unwrap();
        for y in 0..80 {
            let src_row = &cpu_rgba[((y + 13) * w + 17) * 4..((y + 13) * w + 117) * 4];
            let dst_row = &cropped_rgba[y * 400..(y + 1) * 400];
            assert_eq!(dst_row, src_row, "cropped tile row {y} mismatch");
        }

        let (clip_tex, _) = gpu.render_clip_overlay(&presented.clip_bits, w, h, true, true).unwrap();
        let clip_rgba = gpu.readback_texture_rgba(&clip_tex, w, h).unwrap();
        // Pixel (0, 0) is pure black -> shadow overlay (premultiplied (40,90,255,160))
        assert_eq!(clip_rgba[3], 160);
        // Pixel (w-1, h-1) is pure white -> highlight overlay (premultiplied (255,40,30,150))
        let last = ((h - 1) * w + (w - 1)) * 4;
        assert_eq!(clip_rgba[last + 3], 150);
    }

    #[test]
    fn gpu_present_3d_lut_tetrahedral_interpolation_is_accurate() {
        let Some(gpu) = Gpu::with_adapter(AdapterKind::Hardware).expect("GPU init") else {
            return;
        };
        // Smooth nonlinear gamma + channel matrix transform (mimics sRGB -> Display P3).
        let transform = |rgb: [f32; 3]| -> [f32; 3] {
            let lin = rgb.map(|v| v.clamp(0.0, 1.0).powf(2.2));
            let m = [
                0.8225 * lin[0] + 0.1775 * lin[1],
                0.0332 * lin[0] + 0.9668 * lin[1],
                0.0171 * lin[0] + 0.0724 * lin[1] + 0.9105 * lin[2],
            ];
            m.map(|v| v.clamp(0.0, 1.0).powf(1.0 / 2.2))
        };
        let grid = lut33_input_grid();
        let mut lut_rgb = Vec::with_capacity(grid.len());
        for c in grid.as_chunks::<3>().0 {
            lut_rgb.extend_from_slice(&transform(*c));
        }
        let lut = gpu.upload_monitor_lut(Some(&lut_rgb), None).unwrap();
        let image = Image::from_shape_fn((64, 64, 3), |(y, x, c)| match c {
            0 => x as f32 / 63.0,
            1 => y as f32 / 63.0,
            _ => ((x + y) % 64) as f32 / 63.0,
        });
        let frame = gpu.upload(&image).unwrap();
        let presented = gpu.present_frame(&frame, Some(&lut), None, false).unwrap();
        let rgba = gpu.readback_texture_rgba(&presented.texture, 64, 64).unwrap();
        let mut max_diff = 0i32;
        for y in 0..64 {
            for x in 0..64 {
                let expected = transform([image[[y, x, 0]], image[[y, x, 1]], image[[y, x, 2]]]);
                let at = (y * 64 + x) * 4;
                for c in 0..3 {
                    let exp_b = (expected[c].clamp(0.0, 1.0) * 255.0 + 0.5) as i32;
                    max_diff = max_diff.max((rgba[at + c] as i32 - exp_b).abs());
                }
            }
        }
        assert!(max_diff <= 1, "33^3 tetrahedral 3D LUT max byte diff {max_diff} > 1");
    }
}
