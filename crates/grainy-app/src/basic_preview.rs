//! A display shortcut for channel-independent Basic edits on an unmodified linear source.
//! Tone the endpoints of short input intervals with the reference implementation. An interval can
//! supply a byte only when *every* value in it has the same display, histogram and clipping result.
//! Evaluate the remaining pixels with the normal tone and ICC transforms.

use std::sync::LazyLock;

use grainy_cms::Profile;
use grainy_core::{Image, Settings};
use rayon::prelude::*;

use crate::histogram::{to_byte, Bins, Channels, HIGHLIGHT, SHADOW};
use crate::monitor_preview::{MonitorPreview, CELLS, UNCERTAIN};

const STEPS: usize = 32768;
const UNKNOWN: u8 = 4;
type Entries = [Sample; 3];

#[derive(Clone, Copy)]
struct Sample {
    srgb: u8,
    shown: u8,
    clip: u8,
}

impl Sample {
    fn exact(value: f32, displayed: f32) -> Self {
        let clip = if value < crate::histogram::LOW {
            SHADOW
        } else if value >= crate::histogram::HIGH {
            HIGHLIGHT
        } else {
            0
        };
        Self { srgb: to_byte(value), shown: to_byte(displayed), clip }
    }
}

pub(crate) struct Picture {
    pub rgba: Vec<u8>,
    pub mask: Vec<u8>,
    pub channels: Channels,
    pub bins: Bins,
}

fn eligible(s: &Settings) -> bool {
    static DEFAULTS: LazyLock<Settings> = LazyLock::new(|| grainy_engine::working_settings(&Default::default()));
    s.0.iter().all(|(k, v)| {
        DEFAULTS.value(k) == v
            || matches!(
                k.as_str(),
                "exposure"
                    | "contrast"
                    | "blacks"
                    | "whites"
                    | "temperature"
                    | "tint"
                    | "kelvin"
                    | "kelvin_enabled"
                    | "wb_reference"
                    | "panels_off"
            )
    })
}

fn tone_table(s: &Settings, monitor: Option<&[[u16; 3]]>) -> Result<Vec<Entries>, String> {
    static RAMP: LazyLock<Image> =
        LazyLock::new(|| Image::from_shape_fn((1, STEPS + 1, 3), |(_, x, _)| x as f32 / STEPS as f32));
    let toned = grainy_tone::develop_tone(&RAMP, s, None).map_err(|e| e.to_string())?;
    let values = toned.as_slice().ok_or("noncontiguous tone ramp")?;
    let table = (0..STEPS)
        .into_par_iter()
        .with_min_len(1024)
        .map(|i| {
            std::array::from_fn(|c| {
                let (a, b) = (values[i * 3 + c], values[(i + 1) * 3 + c]);
                let low = (a.min(b) - 2e-6).max(0.);
                let high = (a.max(b) + 2e-6).min(1.);
                let mut sample = Sample::exact(low, low);
                let end = Sample::exact(high, high);
                if sample.srgb != end.srgb || sample.clip != end.clip {
                    sample.clip = UNKNOWN;
                } else if let Some(monitor) = monitor {
                    let index = |x: f32| ((x * CELLS as f32) as usize).min(CELLS - 1);
                    let range = &monitor[index(low)..=index(high)];
                    let byte = range[0][c];
                    if byte == UNCERTAIN || range.iter().any(|entry| entry[c] != byte) {
                        sample.clip = UNKNOWN;
                    } else {
                        sample.shown = byte as u8;
                    }
                }
                sample
            })
        })
        .collect();
    Ok(table)
}

struct Band<'a> {
    rgba: &'a mut [u8],
    mask: &'a mut [u8],
    bins: Bins,
    flags: [u8; 3],
    width: usize,
    first_y: usize,
    step: usize,
}

impl Band<'_> {
    #[inline]
    fn put(&mut self, at: usize, samples: Entries, histogram: bool) {
        let mut mask = 0;
        for (c, s) in samples.into_iter().enumerate() {
            self.rgba[at * 4 + c] = s.shown;
            self.flags[c] |= s.clip;
            mask |= s.clip;
            if histogram {
                self.bins[c][usize::from(s.srgb >> 1)] += 1;
            }
        }
        self.mask[at] = mask;
    }

    fn render(
        &mut self,
        src: &[f32],
        table: &[Entries],
        s: &Settings,
        monitor: Option<(&Profile, &Profile)>,
    ) -> Result<(), String> {
        let unknown = Sample { srgb: 0, shown: 0, clip: UNKNOWN };
        let mut pixels = Vec::new();
        let mut offsets = Vec::new();
        for (y, row) in src.chunks_exact(self.width * 3).enumerate() {
            let sample_row = (self.first_y + y).is_multiple_of(self.step);
            for (x, rgb) in row.as_chunks::<3>().0.iter().enumerate() {
                let samples: Entries = std::array::from_fn(|c| {
                    let v = rgb[c];
                    if (0.0..=1.0).contains(&v) {
                        table[((v * STEPS as f32) as usize).min(STEPS - 1)][c]
                    } else {
                        unknown
                    }
                });
                let at = y * self.width + x;
                let histogram = sample_row && x.is_multiple_of(self.step);
                if samples.iter().any(|s| s.clip == UNKNOWN) {
                    pixels.extend_from_slice(rgb);
                    offsets.push((at, histogram));
                } else {
                    self.put(at, samples, histogram);
                }
            }
        }
        if offsets.is_empty() {
            return Ok(());
        }
        let input = Image::from_shape_vec((1, offsets.len(), 3), pixels).map_err(|e| e.to_string())?;
        let toned = grainy_tone::develop_tone(&input, s, None).map_err(|e| e.to_string())?;
        let rgb = toned.as_slice().ok_or("noncontiguous reference pixels")?;
        let transformed = monitor
            .map(|(src, dst)| grainy_cms::native_convert(rgb, src, dst))
            .transpose()
            .map_err(|e| e.to_string())?;
        let display = transformed.as_deref().unwrap_or(rgb);
        for (i, (at, hist)) in offsets.into_iter().enumerate() {
            self.put(at, std::array::from_fn(|c| Sample::exact(rgb[i * 3 + c], display[i * 3 + c])), hist);
        }
        Ok(())
    }
}

pub(crate) fn render(
    input: &Image,
    s: &Settings,
    monitor: Option<&Profile>,
    cache: &mut MonitorPreview,
) -> Result<Option<Picture>, String> {
    let (h, w, channels) = input.dim();
    if w == 0 || h == 0 || channels != 3 || w * h < 128 * 1024 {
        return Ok(None);
    }
    if !eligible(s) {
        return Ok(None);
    }
    let monitor_table = match monitor {
        Some(p) => match cache.table(p)? {
            Some(t) => Some(t),
            None => return Ok(None),
        },
        None => None,
    };
    let Some(data) = input.as_slice() else { return Ok(None) };
    let table = tone_table(s, monitor_table)?;
    let srgb = grainy_cms::profile("sRGB").map_err(|e| e.to_string())?;
    let rows = (32768 / w).max(1);
    let block = rows * w;
    let mut rgba = vec![255; w * h * 4];
    let mut mask = vec![0; w * h];
    let bands: Vec<_> = rgba
        .par_chunks_mut(block * 4)
        .zip(mask.par_chunks_mut(block))
        .zip(data.par_chunks(block * 3))
        .enumerate()
        .map(|(i, ((out, mask), src))| {
            let mut band = Band {
                rgba: out,
                mask,
                flags: [0; 3],
                bins: [[0; 128]; 3],
                width: w,
                first_y: i * rows,
                step: (h / 300).max(1),
            };
            band.render(src, &table, s, monitor.map(|p| (&srgb, p)))?;
            Ok::<_, String>((band.bins, band.flags))
        })
        .collect::<Result<_, _>>()?;
    let mut bins = [[0; 128]; 3];
    let mut channels = ([false; 3], [false; 3]);
    for (counts, flags) in bands {
        for c in 0..3 {
            channels.0[c] |= flags[c] & SHADOW != 0;
            channels.1[c] |= flags[c] & HIGHLIGHT != 0;
            for (sum, n) in bins[c].iter_mut().zip(counts[c]) {
                *sum += n;
            }
        }
    }
    Ok(Some(Picture { rgba, mask, channels, bins }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use grainy_engine::{develop, Options, Output};

    #[test]
    fn basic_intervals_and_sparse_fallback_equal_reference_bytes_and_statistics() {
        let mut state = 123u32;
        let image = Image::from_shape_fn((333, 409, 3), |(_, x, c)| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            match x % 8 {
                0 => (x as f32 + c as f32) / STEPS as f32,
                1 => f32::from_bits((0.0031308_f32).to_bits() + c as u32),
                2 => state as f32 / u32::MAX as f32 * 4. - 1.,
                _ => state as f32 / u32::MAX as f32,
            }
        });
        let profile = grainy_cms::rgb_profile(
            Some([0.3127, 0.3290]),
            Some([0.64, 0.33, 0.30, 0.60, 0.15, 0.06]),
            grainy_cms::TransferFunction::Table(&grainy_cms::srgb_transfer_table()),
        )
        .unwrap();
        let mut cache = MonitorPreview::default();
        for ev in [-5., 0., 0.7, 5.] {
            let mut s = grainy_core::defaults();
            s.set("exposure", ev);
            s.set("temperature", 32.);
            s.set("tint", -21.);
            s.set("blacks", -16.);
            s.set("whites", 72.);
            s.set("contrast", 85.);
            let reference =
                develop(&image, &s.0, &Options { output: Output::Named("sRGB"), ..Options::default() }).unwrap();
            let c = crate::histogram::convert(&reference);
            for monitor in [None, Some(&profile)] {
                let expected = match monitor {
                    None => c.rgba.clone(),
                    Some(p) => crate::histogram::rgba(
                        &grainy_cms::output_rgb(&reference, "sRGB", grainy_cms::Destination::Icc(p)).unwrap(),
                    ),
                };
                let fast = render(&image, &s, monitor, &mut cache).unwrap().unwrap();
                assert_eq!(expected.iter().zip(&fast.rgba).map(|(a, b)| a.abs_diff(*b)).max(), Some(0));
                assert_eq!(fast.channels, c.channels);
                assert_eq!(fast.mask, c.mask);
                assert_eq!(fast.bins, crate::histogram::bins(&c.rgba, c.width, c.height));
            }
        }
        for (key, value) in [
            ("shadows", serde_json::json!(15)),
            ("crop", serde_json::json!([0.1, 0.1, 0.9, 0.9])),
            ("working_space", serde_json::json!("ProPhoto")),
            ("grain", serde_json::json!(25)),
        ] {
            let mut s = grainy_core::defaults();
            s.set(key, value);
            assert!(render(&image, &s, None, &mut cache).unwrap().is_none());
        }
    }
}
