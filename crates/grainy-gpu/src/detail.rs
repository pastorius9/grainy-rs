//! Port of native_gpu._detail_params/_gaussian/detail and processing's noise filters.
//! Wavelet luminance-noise statistics run on the GPU via double-float reductions (zero host readbacks).
use crate::device::{Batch, BufferSlice};
use crate::{Error, Frame, Gpu, Result};
use grainy_core::{Image, Settings};

pub(crate) const KEYS: &[&str] = &[
    "noise_luma",
    "noise_color",
    "dehaze",
    "texture",
    "defringe",
    "monochrome",
    "clarity",
    "sharpen",
    "sharpen_radius",
    "sharpen_detail",
    "sharpen_mask",
    "vignette",
    "detail_version",
    "vignette_midpoint",
    "vignette_roundness",
    "vignette_feather",
    "vignette_highlights",
    "hdr",
    "wavelet_profiles",
];

fn gaussian(sigma: f64) -> Vec<[f32; 2]> {
    let sigma = sigma.max(0.05);
    let n = ((sigma * 8.0 + 1.0).round_ties_even() as usize) | 1;
    let r = n / 2;
    let values: Vec<f64> = (0..n)
        .map(|i| {
            let x = i as f64 - r as f64;
            (-0.5 * x * x / (sigma * sigma)).exp()
        })
        .collect();
    let sum: f64 = values.iter().sum();
    values.iter().map(|v| [(v / sum) as f32, 0.0]).collect()
}
fn constants(s: &Settings, w: usize, h: usize, scale: f64) -> Result<Option<Vec<f32>>> {
    if !scale.is_finite() || scale <= 0.0 {
        return Err(Error::Input("pixel_scale must be finite and positive".into()));
    }
    if s.bool("hdr") {
        return Ok(None);
    }
    // Bound shader loop lengths before any float-to-integer conversions or allocation.
    if scale > 32.0 || s.f64("sharpen_radius") * scale > 256.0 || (h.min(w) as f64 / 80.0) > 256.0 {
        return Ok(None);
    }
    let mut p = vec![0.0; 32];
    p[0] = w as f32;
    p[1] = h as f32;
    let flags = u32::from(s.bool("texture"))
        | (u32::from(s.bool("monochrome")) << 1)
        | (u32::from(s.f64("sharpen_detail") < 100.0) << 2)
        | (u32::from(s.bool("sharpen_mask")) << 3)
        | (u32::from(s.bool("defringe")) << 4)
        | (u32::from(s.f64("detail_version") >= 2.0) << 6);
    p[3] = flags as f32;
    p[4] = s.f32("texture");
    p[5] = s.f32("clarity");
    p[6] = s.f32("sharpen");
    p[7] = ((1.0 - s.f64("sharpen_detail") / 100.0) * 0.012).max(1e-6) as f32;
    p[8] = (0.002 + s.f64("sharpen_mask") / 200.0) as f32;
    p[9] = scale as f32;
    p[10] = (s.f64("vignette") / 220.0) as f32;
    p[15] = (s.f64("dehaze") / 100.0 * 0.8) as f32;
    p[16] = s.f32("defringe");
    p[17] = (scale * scale) as f32;
    p[18] = (3.0 * scale).round_ties_even() as f32;
    let v = grainy_effects::vignette_shape(s, w, h);
    p[19] = v.round as f32;
    p[20] = v.p as f32;
    p[21] = v.scale as f32;
    p[22] = v.exponent as f32;
    p[23] = v.protect as f32;
    p[24] = v.aspect_x as f32;
    p[25] = v.aspect_y as f32;
    if !p.iter().all(|v| v.is_finite()) {
        return Err(Error::Input("non-finite detail constants".into()));
    }
    Ok(Some(p))
}

impl Gpu {
    pub(crate) fn detail_supported(&self, s: &Settings, w: usize, h: usize, scale: f64) -> Result<bool> {
        Ok(constants(s, w, h, scale)?.is_some())
    }

    fn detail_pass_table(
        &self,
        batch: &mut Batch<'_>,
        source: &Frame,
        p: &[f32],
        stage: u32,
        table: &BufferSlice,
        extra: [Option<&Frame>; 4],
    ) -> Result<Frame> {
        let mut p = p.to_vec();
        p[2] = stage as f32;
        for f in extra.iter().flatten() {
            self.check_frame(f)?;
            if f.shape() != source.shape() {
                return Err(Error::Input("detail intermediate shape differs".into()));
            }
        }
        batch.dispatch_with_table(source, 3 + stage as usize, &p, table, extra.map(|f| f.map(|f| &f.buffer)))
    }

    fn detail_pass(
        &self,
        batch: &mut Batch<'_>,
        source: &Frame,
        p: &[f32],
        stage: u32,
        taps: &[[f32; 2]],
        extra: [Option<&Frame>; 4],
    ) -> Result<Frame> {
        let table = batch.stage_table(taps)?;
        self.detail_pass_table(batch, source, p, stage, &table, extra)
    }

    #[allow(clippy::too_many_arguments)]
    fn filter_with_table(
        &self,
        batch: &mut Batch<'_>,
        source: &Frame,
        p: &[f32],
        radius: usize,
        stride: usize,
        mode: u32,
        table: &BufferSlice,
    ) -> Result<Frame> {
        let mut p = p.to_vec();
        p[11] = radius as f32;
        p[14] = stride as f32;
        p[31] = mode as f32;
        let tmp = self.detail_pass_table(batch, source, &p, 0, table, [None; 4])?;
        self.detail_pass_table(batch, &tmp, &p, 1, table, [None; 4])
    }

    fn filter(&self, batch: &mut Batch<'_>, source: &Frame, p: &[f32], taps: &[[f32; 2]]) -> Result<Frame> {
        let table = batch.stage_table(taps)?;
        self.filter_with_table(batch, source, p, taps.len() / 2, 1, 0, &table)
    }

    /// Port of `native_gpu.detail_tools_on_gpu`: false when luminance-noise statistics require host reductions.
    pub fn detail_tools_on_gpu(&self, s: &Settings, pixel_scale: f64) -> bool {
        pixel_scale.is_finite()
            && pixel_scale > 0.0
            && pixel_scale <= 32.0
            && !s.bool("hdr")
            && !(s.bool("noise_luma") && (3.0 * pixel_scale).round_ties_even() > 0.0)
    }

    /// Port of `native_gpu.detail`, without grain.
    pub fn detail(&self, image: &Image, s: &Settings, pixel_scale: f64) -> Result<Option<Image>> {
        let f = self.upload(image)?;
        self.detail_frame(&f, s, pixel_scale)?.map(|f| self.readback(&f)).transpose()
    }

    /// Port of the detail slot in `native_gpu.develop_resident`; all pixel filtering and wavelet statistics stay on GPU.
    pub fn detail_frame(&self, frame: &Frame, s: &Settings, pixel_scale: f64) -> Result<Option<Frame>> {
        let mut batch = self.batch();
        let out = self.detail_in_batch(&mut batch, frame, s, pixel_scale)?;
        batch.submit()?;
        Ok(out)
    }

    pub(crate) fn detail_in_batch(
        &self,
        batch: &mut Batch<'_>,
        frame: &Frame,
        s: &Settings,
        pixel_scale: f64,
    ) -> Result<Option<Frame>> {
        self.check_frame(frame)?;
        let Some(p) = constants(s, frame.width, frame.height, pixel_scale)? else {
            return Ok(None);
        };
        let none = [None; 4];
        let mut current = frame.clone();
        if p[18] > 0.0 && (s.bool("noise_luma") || s.bool("noise_color")) {
            current = self.detail_pass(batch, &current, &p, 6, &[], none)?;
            if s.bool("noise_color") {
                current = self.chroma(batch, &current, &p, s.f64("noise_color"), pixel_scale)?;
            }
            if s.bool("noise_luma") {
                current = if s.f64("detail_version") >= 3.0 {
                    self.wavelet(batch, &current, &p, s, pixel_scale)?
                } else {
                    self.bilateral(batch, &current, &p, s.f64("noise_luma"), pixel_scale)?
                };
            }
            current = self.detail_pass(batch, &current, &p, 8, &[], none)?;
        }
        if s.bool("dehaze") {
            let minimum = self.detail_pass(batch, &current, &p, 10, &[], none)?;
            let x = self.detail_pass(batch, &minimum, &p, 11, &[], none)?;
            let y = self.detail_pass(batch, &x, &p, 12, &[], none)?;
            current = self.detail_pass(batch, &current, &p, 13, &[], [Some(&y), None, None, None])?;
        }
        let blurred = if s.bool("texture") {
            Some(self.filter(batch, &current, &p, &gaussian(1.3 * pixel_scale))?)
        } else {
            None
        };
        current = self.detail_pass(batch, &current, &p, 2, &[], [blurred.as_ref(), None, None, None])?;
        if s.bool("defringe") {
            current = self.detail_pass(batch, &current, &p, 14, &[], none)?;
        }
        for (active, sigma, stage) in [
            (s.bool("clarity"), pixel_scale.max(frame.height.min(frame.width) as f64 / 80.0), 3),
            (s.bool("sharpen"), s.f64("sharpen_radius") * pixel_scale, 4),
        ] {
            if active {
                let blurred = self.filter(batch, &current, &p, &gaussian(sigma))?;
                current = self.detail_pass(batch, &current, &p, stage, &[], [Some(&blurred), None, None, None])?;
            }
        }
        if s.bool("vignette") {
            current = self.detail_pass(batch, &current, &p, 5, &[], none)?;
        }
        Ok(Some(current))
    }

    fn chroma(&self, batch: &mut Batch<'_>, lab: &Frame, p: &[f32], amount: f64, scale: f64) -> Result<Frame> {
        let strength = (amount / 50.0).clamp(0.0, 2.0);
        let r = ((2.0 + 14.0 * strength) * scale).round_ties_even().max(1.0) as usize;
        let taps = batch.stage_table(&[[1.0 / (2 * r + 1) as f32, 0.0]])?;
        let none = [None; 4];
        let mut p = p.to_vec();
        p[7] = (0.0005 + 0.02 * strength) as f32;
        p[8] = 8.0 * p[7];
        p[13] = (2.0 * strength).min(1.0) as f32;
        let guide = self.detail_pass(batch, lab, &p, 17, &[], none)?;
        let means = self.filter_with_table(batch, &guide, &p, r, 1, 2, &taps)?;
        let products = self.detail_pass(batch, &guide, &p, 18, &[], none)?;
        let products = self.filter_with_table(batch, &products, &p, r, 1, 2, &taps)?;
        let squares = self.detail_pass(batch, &guide, &p, 19, &[], none)?;
        let squares = self.filter_with_table(batch, &squares, &p, r, 1, 2, &taps)?;
        p[12] = 0.0;
        let i1 = self.detail_pass(batch, &means, &p, 20, &[], [Some(&products), Some(&squares), None, None])?;
        p[12] = 1.0;
        let i2 = self.detail_pass(batch, &means, &p, 20, &[], [Some(&products), Some(&squares), None, None])?;
        let mut current = lab.clone();
        for channel in [1.0, 2.0] {
            p[12] = channel;
            let d = self.detail_pass(batch, &current, &p, 21, &[], [Some(&guide), None, None, None])?;
            let d = self.filter_with_table(batch, &d, &p, r, 1, 2, &taps)?;
            let e = self.detail_pass(batch, &current, &p, 22, &[], [Some(&guide), None, None, None])?;
            let e = self.filter_with_table(batch, &e, &p, r, 1, 3, &taps)?;
            let k = self.detail_pass(batch, &means, &p, 23, &[], [Some(&d), Some(&e), Some(&i1), Some(&i2)])?;
            let b = self.detail_pass(batch, &means, &p, 24, &[], [Some(&k), Some(&d), None, None])?;
            let k = self.filter_with_table(batch, &k, &p, r, 1, 2, &taps)?;
            let b = self.filter_with_table(batch, &b, &p, r, 1, 3, &taps)?;
            current = self.detail_pass(batch, &current, &p, 25, &[], [Some(&guide), Some(&k), Some(&b), None])?;
        }
        Ok(current)
    }

    fn bilateral(&self, batch: &mut Batch<'_>, lab: &Frame, p: &[f32], amount: f64, scale: f64) -> Result<Frame> {
        // Legacy bilateral mode reads back extrema to build the exact OpenCV float LUT.
        batch.flush()?;
        let host = self.readback(lab)?;
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for pixel in host.lanes(ndarray::Axis(2)) {
            min = min.min(pixel[0]);
            max = max.max(pixel[0]);
        }
        if (max - min).abs() < f32::EPSILON {
            return Ok(lab.clone());
        }
        let strength = amount / 100.0;
        let sigma_color = (2.0 + strength * 16.0).max(1e-10);
        let sigma_space = ((1.0 + strength * 3.0) * scale).max(1e-10);
        let index_scale = 4096.0 / (max - min);
        let mut table = Vec::new();
        for i in 0..4098 {
            let value = i as f64 / f64::from(index_scale);
            table.push([(-0.5 * value * value / (sigma_color * sigma_color)).exp() as f32, 0.0]);
        }
        let offset = table.len();
        let r = (3.0 * scale).round_ties_even().max(1.0) as i32;
        for y in -r..=r {
            for x in -r..=r {
                let d = (x * x + y * y) as f64;
                if d > (r * r) as f64 {
                    continue;
                }
                table.push([y as f32, x as f32]);
                table.push([(-0.5 * d / (sigma_space * sigma_space)).exp() as f32, 0.0]);
            }
        }
        let mut p = p.to_vec();
        p[13] = strength as f32;
        p[28] = index_scale;
        p[29] = offset as f32;
        p[30] = ((table.len() - offset) / 2) as f32;
        self.detail_pass(batch, lab, &p, 7, &table, [None; 4])
    }

    fn wavelet(&self, batch: &mut Batch<'_>, lab: &Frame, p: &[f32], s: &Settings, scale: f64) -> Result<Frame> {
        let none = [None; 4];
        let mut p = p.to_vec();
        p[26] = ((s.f64("noise_luma") / 100.0 * 3.0).powi(2) * scale) as f32;
        let mut current = self.detail_pass(batch, lab, &p, 26, &[], none)?;
        let mut profile = [0f32; 8];
        let profiles = s.value("wavelet_profiles").as_array();
        if let Some(profiles) = profiles {
            if profiles.len() != 3
                || profiles.iter().any(|r| {
                    r.as_array().is_none_or(|a| {
                        a.len() != 8 || a.iter().any(|v| v.as_f64().is_none_or(|v| !(v as f32).is_finite()))
                    })
                })
            {
                return Err(Error::Input("wavelet_profiles must be 3 x 8 numbers".into()));
            }
        }
        let gpu_profile = if profiles.is_none() { Some(batch.alloc_profile()) } else { None };
        let wavelet_taps = BufferSlice::entire(&self.wavelet_taps);
        let box2_taps = BufferSlice::entire(&self.box2_taps);
        for level in 0..5 {
            p[27] = level as f32;
            let step = 1usize << level;
            let smooth = self.filter_with_table(batch, &current, &p, 2, step, 1, &wavelet_taps)?;
            let energy = self.detail_pass(batch, &current, &p, 29, &[], [Some(&smooth), None, None, None])?;
            let table_slice = if let Some(profile_buf) = &gpu_profile {
                if level < 3 {
                    batch.reduce_noise_profile(&energy, profile_buf)?;
                } else {
                    batch.halve_noise_profile(profile_buf)?;
                }
                BufferSlice::entire(profile_buf)
            } else {
                let profiles = profiles.unwrap();
                if level < 3 {
                    for (i, v) in profile.iter_mut().enumerate() {
                        *v = profiles[level][i].as_f64().unwrap_or(0.0) as f32;
                    }
                } else {
                    for v in &mut profile {
                        *v *= 0.5;
                    }
                }
                let table: Vec<_> = profile.iter().map(|&v| [v, 0.0]).collect();
                batch.stage_table(&table)?
            };
            let energy = self.filter_with_table(batch, &energy, &p, 2, 1, 3, &box2_taps)?;
            current = self.detail_pass_table(
                batch,
                &current,
                &p,
                30,
                &table_slice,
                [Some(&smooth), Some(&energy), None, None],
            )?;
        }
        self.detail_pass(batch, lab, &p, 31, &[], [Some(&current), None, None, None])
    }
}

#[cfg(test)]
fn noise_profile(residual: &Image) -> [f32; 8] {
    let count = residual.dim().0 * residual.dim().1;
    let mean = residual.lanes(ndarray::Axis(2)).into_iter().map(|p| f64::from(p[1].abs())).sum::<f64>() / count as f64;
    let limit = (2.5 * mean) as f32;
    let mut sums = [0f64; 8];
    let mut counts = [0usize; 8];
    for p in residual.lanes(ndarray::Axis(2)) {
        let bin = ((p[2] * 0.08) as i32).clamp(0, 7) as usize;
        sums[bin] += f64::from(p[1].abs().min(limit));
        counts[bin] += 1;
    }
    let overall = sums.iter().sum::<f64>() / count as f64;
    let minimum = 256.max(count / 2000);
    std::array::from_fn(|i| if counts[i] > minimum { (sums[i] / counts[i] as f64) as f32 } else { overall as f32 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gaussian_matches_oracle() {
        let case = grainy_testkit::Case::open("gpu", "gaussian");
        for (i, sigma) in case.json()["sigmas"].as_array().unwrap().iter().enumerate() {
            let kernel = super::gaussian(sigma.as_f64().unwrap());
            let actual = ndarray::Array1::from_iter(kernel.iter().map(|p| p[0]));
            grainy_testkit::assert_close(&actual, &case.array(&format!("kernel_{i}")), 1e-7, "gaussian taps");
        }
    }

    #[test]
    fn gpu_noise_profile_matches_cpu_f64_reduction() {
        let Some(gpu) = Gpu::with_adapter(crate::AdapterKind::Hardware).expect("GPU init") else {
            return;
        };
        for (h, w) in [(1, 1), (1, 9), (9, 1), (160, 240), (683, 1024)] {
            let energy = Image::from_shape_fn((h, w, 3), |(y, x, c)| match c {
                0 => 0.0,
                1 => (((x * 37 + y * 91) % 1009) as f32 / 1009.0 - 0.5) * 14.0,
                _ => ((x * 13 + y * 29) % 101) as f32,
            });
            let expected = noise_profile(&energy);
            let frame = gpu.upload(&energy).unwrap();
            let mut batch = gpu.batch();
            let profile = batch.alloc_profile();
            batch.reduce_noise_profile(&frame, &profile).unwrap();
            batch.submit().unwrap();
            let got = gpu.readback_profile(&profile).unwrap();
            for i in 0..8 {
                let diff = (got[i] - expected[i]).abs();
                assert!(diff < 1e-5, "{h}x{w} bin {i}: gpu={} cpu={} diff={diff}", got[i], expected[i]);
            }
        }
    }
}
