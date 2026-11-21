use crate::{params, Error, Frame, Gpu, Result};
use grainy_core::{Image, Settings};
use grainy_tone::ToneBase;
use serde_json::Value;

/// Port of `engine.develop`'s first unsupported GPU stage. Later stages must stay on the CPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuStage {
    /// Tone (film, DRT or unsupported tone parameters).
    Tone,
    /// Colour (HDR, photo filter or unsupported colour parameters).
    Color,
    /// Detail (unsupported filter radius or scale).
    Detail,
}

/// Port of the resident prefix of `engine.develop`; `frame` is the input of `fallback`, or the finished result.
pub struct ResidentResult {
    /// Last supported GPU stage, absent when tone needs the CPU.
    pub frame: Option<Frame>,
    /// First stage to evaluate on the CPU, if any.
    pub fallback: Option<CpuStage>,
}

/// Execution counts for `native_gpu.develop_resident`; callers can inspect which slider stages were reused.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StageRuns {
    /// Full image uploads (one when the resident is created or replaced).
    pub uploads: u64,
    /// CPU process-3 base analyses; Highlights/Shadows edits reuse the base.
    pub analyses: u64,
    /// Tone dispatches.
    pub tone: u64,
    /// Colour dispatches.
    pub color: u64,
    /// Detail-chain evaluations.
    pub detail: u64,
    /// Uploads of the generated grain field; tone/colour slider edits reuse it.
    pub grain_uploads: u64,
    /// Grain/final-clip dispatches.
    pub finish: u64,
}

/// Port of the per-owner slots in `native_gpu.develop_resident`, with structural keys and explicit ownership.
pub struct Resident {
    source: Image,
    frame: Frame,
    tone: Option<(Vec<Value>, Frame)>,
    color: Option<(Vec<Value>, Frame)>,
    detail: Option<(Vec<Value>, Frame)>,
    finish: Option<(Vec<Value>, Frame)>,
    grain: Option<(Vec<Value>, wgpu::Buffer)>,
    base: Option<(Vec<Value>, ToneBase)>,
    runs: StageRuns,
}

impl Resident {
    /// Port of `native_gpu.develop_resident`'s geometry upload; the caller owns the lifetime of its cache.
    pub fn new(gpu: &Gpu, image: &Image) -> Result<Self> {
        Ok(Self {
            frame: gpu.upload(image)?,
            source: image.clone(),
            tone: None,
            color: None,
            detail: None,
            finish: None,
            grain: None,
            base: None,
            runs: StageRuns { uploads: 1, ..Default::default() },
        })
    }
    /// Port of the geometry-key invalidation in `native_gpu.develop_resident`; failed uploads leave the old cache intact.
    pub fn replace(&mut self, gpu: &Gpu, image: &Image) -> Result<()> {
        let replacement = Self::new(gpu, image)?;
        let uploads = self.runs.uploads + 1;
        *self = replacement;
        self.runs.uploads = uploads;
        Ok(())
    }
    /// Execution statistics of `native_gpu.develop_resident` for verification and slider diagnostics.
    pub fn runs(&self) -> StageRuns {
        self.runs
    }
}

impl Gpu {
    /// Port of `native_gpu.develop_resident`: recompute only the first changed stage and its successors.
    /// No source image upload occurs here. Luminance-noise changes read back statistics; other sliders remain resident.
    pub fn develop_resident(&self, r: &mut Resident, s: &Settings, pixel_scale: f64) -> Result<Option<Frame>> {
        let result = self.develop_resident_prefix(r, s, pixel_scale, true)?;
        Ok(if result.fallback.is_none() { result.frame } else { None })
    }

    /// Port of `engine.develop`'s cached GPU stages, returning the supported prefix at a CPU boundary.
    /// `clip = false` preserves out-of-range detail/grain values for CPU masks before the final clip.
    pub fn develop_resident_prefix(
        &self,
        r: &mut Resident,
        s: &Settings,
        pixel_scale: f64,
        clip: bool,
    ) -> Result<ResidentResult> {
        self.check_frame(&r.frame)?;
        if !pixel_scale.is_finite() || pixel_scale <= 0.0 {
            return Err(Error::Input("pixel_scale must be finite and positive".into()));
        }
        let mut batch = self.batch();
        let tone_key = params::key(s, params::TONE_KEYS);
        if r.tone.as_ref().is_none_or(|(key, _)| *key != tone_key) {
            if params::needs_local(s) {
                let keys = [
                    "exposure",
                    "temperature",
                    "tint",
                    "kelvin",
                    "wb_reference",
                    "kelvin_enabled",
                    "lens_vignette",
                    "hdr",
                    "tone_version",
                    "film_look",
                    "film_strength",
                    "working_space",
                    "tone_mapping",
                    "drt_contrast",
                    "drt_toe",
                    "drt_shoulder",
                    "drt_white",
                ];
                let key = params::key(s, &keys);
                if r.base.as_ref().is_none_or(|(k, _)| *k != key) {
                    if grainy_film::active(s) || grainy_tone::mapping::active(s) {
                        return Ok(ResidentResult { frame: None, fallback: Some(CpuStage::Tone) });
                    }
                    let base = grainy_tone::tone_base(&r.source, s, None).map_err(|e| Error::Input(e.to_string()))?;
                    r.base = Some((key, base));
                    r.runs.analyses += 1;
                }
            }
            let Some((p, t)) = params::tone(s, &r.source, r.base.as_ref().map(|(_, b)| b))? else {
                return Ok(ResidentResult { frame: None, fallback: Some(CpuStage::Tone) });
            };
            let frame = batch.dispatch(&r.frame, 0, &p, &t, &[])?;
            r.tone = Some((tone_key, frame));
            r.color = None;
            r.detail = None;
            r.finish = None;
            r.runs.tone += 1;
        }
        let Some(color_params) = params::color(s, r.frame.width, r.frame.height)? else {
            batch.submit()?;
            return Ok(ResidentResult {
                frame: r.tone.as_ref().map(|(_, f)| f.clone()),
                fallback: Some(CpuStage::Color),
            });
        };
        let color_key = params::key(s, params::COLOR_KEYS);
        if r.color.as_ref().is_none_or(|(key, _)| *key != color_key) {
            let source = &r.tone.as_ref().ok_or_else(|| Error::Device("missing tone slot".into()))?.1;
            // SDR tone has already clipped. Without colour operations the shader would only clip again.
            let frame = if color_params.0[2] == 0.0 && color_params.1.is_empty() {
                source.clone()
            } else {
                batch.dispatch(source, 1, &color_params.0, &color_params.1, &[])?
            };
            r.color = Some((color_key, frame));
            r.detail = None;
            r.finish = None;
            r.runs.color += 1;
        }
        let neutral_detail = [
            "noise_luma",
            "noise_color",
            "dehaze",
            "texture",
            "defringe",
            "monochrome",
            "clarity",
            "sharpen",
            "vignette",
        ]
        .iter()
        .all(|key| !s.bool(key));
        let mut detail_key = params::key(s, crate::detail::KEYS);
        detail_key.push(Value::from(pixel_scale));
        if r.detail.as_ref().is_none_or(|(key, _)| *key != detail_key) {
            let source = &r.color.as_ref().ok_or_else(|| Error::Device("missing color slot".into()))?.1;
            if !self.detail_supported(s, source.width, source.height, pixel_scale)? {
                batch.submit()?;
                return Ok(ResidentResult { frame: Some(source.clone()), fallback: Some(CpuStage::Detail) });
            }
            let frame = if neutral_detail {
                source.clone()
            } else {
                let Some(frame) = self.detail_in_batch(&mut batch, source, s, pixel_scale)? else {
                    batch.submit()?;
                    return Ok(ResidentResult { frame: Some(source.clone()), fallback: Some(CpuStage::Detail) });
                };
                frame
            };
            r.detail = Some((detail_key, frame));
            r.finish = None;
            r.runs.detail += 1;
        }
        let mut finish_key = params::key(s, &["grain", "grain_size", "grain_roughness"]);
        finish_key.push(Value::from(pixel_scale));
        let grain_key = finish_key.clone();
        finish_key.push(Value::from(clip));
        if r.finish.as_ref().is_none_or(|(key, _)| *key != finish_key) {
            let source = &r.detail.as_ref().ok_or_else(|| Error::Device("missing detail slot".into()))?.1;
            if s.bool("grain") && r.grain.as_ref().is_none_or(|(key, _)| *key != grain_key) {
                let grain = grainy_effects::settings_grain((source.height, source.width), s, pixel_scale);
                let values: Vec<_> = grain.iter().copied().collect();
                r.grain = Some((grain_key, self.upload_values(&values)?));
                r.runs.grain_uploads += 1;
            }
            let params = [source.width as f32, source.height as f32, f32::from(s.bool("grain")), f32::from(!clip)];
            let frame = if !s.bool("grain") && (!clip || neutral_detail) {
                source.clone()
            } else {
                batch.dispatch_buffers(
                    source,
                    2,
                    &params,
                    &[],
                    [r.grain.as_ref().map(|(_, buffer)| buffer), None, None, None],
                )?
            };
            r.finish = Some((finish_key, frame));
            r.runs.finish += 1;
        }
        batch.submit()?;
        Ok(ResidentResult { frame: r.finish.as_ref().map(|(_, f)| f.clone()), fallback: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_suffix_retains_only_the_supported_prefix() {
        let Some(gpu) = Gpu::with_adapter(crate::AdapterKind::Hardware).expect("GPU initialization") else {
            eprintln!("SKIP resident prefix: no compute adapter");
            return;
        };
        let image = grainy_testkit::input("colours");
        let mut resident = Resident::new(&gpu, &image).unwrap();
        let mut s = grainy_core::defaults();
        s.set("photo_filter_enabled", true);
        s.set("photo_filter", "cooling80");
        let result = gpu.develop_resident_prefix(&mut resident, &s, 1.0, false).unwrap();
        assert_eq!(result.fallback, Some(CpuStage::Color));
        let toned = grainy_tone::develop_tone(&image, &s, None).unwrap();
        grainy_testkit::assert_close(&gpu.readback(&result.frame.unwrap()).unwrap(), &toned, 1e-5, "tone prefix");
        assert_eq!((resident.runs().tone, resident.runs().color, resident.runs().detail), (1, 0, 0));
        s.set("photo_filter_enabled", false);
        s.set("sharpen", 50);
        s.set("sharpen_radius", 1000);
        let result = gpu.develop_resident_prefix(&mut resident, &s, 1.0, false).unwrap();
        assert_eq!(result.fallback, Some(CpuStage::Detail));
        let colored = grainy_colorgrade::develop_color(&toned, &s).unwrap();
        grainy_testkit::assert_close(&gpu.readback(&result.frame.unwrap()).unwrap(), &colored, 1e-5, "colour prefix");
        assert_eq!((resident.runs().tone, resident.runs().color, resident.runs().detail), (1, 1, 0));
        s.set("sharpen", 0);
        s.set("sharpen_radius", 1.0);
        let result = gpu.develop_resident_prefix(&mut resident, &s, 1.0, false).unwrap();
        assert!(result.fallback.is_none());
        assert_eq!((resident.runs().tone, resident.runs().color, resident.runs().detail), (1, 1, 1));
        s.set("film_look", "kodak-portra-400");
        let result = gpu.develop_resident_prefix(&mut resident, &s, 1.0, false).unwrap();
        assert_eq!(result.fallback, Some(CpuStage::Tone));
        assert!(result.frame.is_none());
    }
}
