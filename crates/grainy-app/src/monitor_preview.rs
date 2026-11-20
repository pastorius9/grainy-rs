//! Exact RGBA8 monitor presentation. For an RGB matrix/shaper profile each output is monotone
//! in each input (monotone curves before and after a fixed matrix). Near-sRGB monitors therefore
//! have many input intervals where an output byte is fixed for *all* values of the other channels.
//! LittleCMS supplies both the interval bounds and every ambiguous result.

use grainy_cms::{Destination, Profile};
use grainy_core::Image;
use rayon::prelude::*;

use crate::histogram::{rgba, to_byte};

pub(super) const CELLS: usize = 8192;
pub(super) const UNCERTAIN: u16 = 256;

#[derive(Default)]
pub(crate) struct MonitorPreview {
    cached: Option<(Profile, Option<Vec<[u16; 3]>>)>,
}

impl MonitorPreview {
    pub(super) fn is_cached(&self, profile: &Profile) -> bool {
        self.cached.as_ref().is_some_and(|(p, _)| p == profile)
    }

    pub(super) fn table(&mut self, profile: &Profile) -> Result<Option<&[[u16; 3]]>, String> {
        if self.cached.as_ref().is_none_or(|(p, _)| p != profile) {
            self.cached = Some((profile.clone(), bounds(profile)?));
        }
        Ok(self.cached.as_ref().and_then(|(_, table)| table.as_deref()))
    }

    pub(crate) fn rgba(&mut self, image: &Image, space: &str, profile: &Profile) -> Result<Vec<u8>, String> {
        if space != "ProPhoto" {
            if let Some(table) = self.table(profile)? {
                return bounded_rgba(image, profile, table);
            }
        }
        grainy_cms::output_rgb(image, space, Destination::Icc(profile))
            .map(|shown| rgba(&shown))
            .map_err(|e| e.to_string())
    }
}

fn u32_at(data: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_be_bytes(data.get(at..at.checked_add(4)?)?.try_into().ok()?) as usize)
}

// Accept only unambiguous RGB monitor matrix profiles with monotonically sampled output TRCs.
// A LUT, parametric curve, duplicate component or unrecognised processing tag takes the ordinary path.
fn matrix_shaper(data: &[u8]) -> Option<()> {
    if data.get(12..24)? != b"mntrRGB XYZ " {
        return None;
    }
    let count = u32_at(data, 128)?;
    let tags = data.get(132..132usize.checked_add(count.checked_mul(12)?)?)?;
    let mut found = 0u8;
    for t in tags.as_chunks::<12>().0 {
        let offset = u32_at(t, 4)?;
        let body = data.get(offset..offset.checked_add(u32_at(t, 8)?)?)?;
        let bit = match &t[..4] {
            b"rTRC" | b"gTRC" | b"bTRC" => {
                if body.get(..4)? != b"curv" {
                    return None;
                }
                let n = u32_at(body, 8)?;
                let samples = body.get(12..12usize.checked_add(n.checked_mul(2)?)?)?;
                if n < 2 || !samples.as_chunks::<2>().0.is_sorted() {
                    return None;
                }
                match t[0] {
                    b'r' => 1,
                    b'g' => 2,
                    _ => 4,
                }
            }
            b"rXYZ" | b"gXYZ" | b"bXYZ" if body.get(..4)? == b"XYZ " && body.len() == 20 => match t[0] {
                b'r' => 8,
                b'g' => 16,
                _ => 32,
            },
            b"cprt" | b"desc" | b"wtpt" | b"bkpt" | b"dmnd" | b"dmdd" | b"vued" | b"view" | b"lumi" | b"meas"
            | b"tech" | b"chad" | b"chrm" => continue,
            _ => return None,
        };
        if found & bit != 0 {
            return None;
        }
        found |= bit;
    }
    (found == 63).then_some(())
}

fn bounds(profile: &Profile) -> Result<Option<Vec<[u16; 3]>>, String> {
    if matrix_shaper(profile.bytes()).is_none() {
        return Ok(None);
    }
    let srgb = grainy_cms::profile("sRGB").map_err(|e| e.to_string())?;
    let mut probes = Vec::with_capacity((CELLS + 1) * 36);
    for i in 0..=CELLS {
        for c in 0..3 {
            for corner in 0..4 {
                let mut rgb = [0.; 3];
                rgb[c] = i as f32 / CELLS as f32;
                rgb[(c + 1) % 3] = (corner & 1) as f32;
                rgb[(c + 2) % 3] = (corner >> 1) as f32;
                probes.extend(rgb);
            }
        }
    }
    let values = grainy_cms::native_convert(&probes, &srgb, profile).map_err(|e| e.to_string())?;
    let mut table = vec![[UNCERTAIN; 3]; CELLS];
    let mut fixed = 0;
    for (i, entry) in table.iter_mut().enumerate() {
        for (c, value) in entry.iter_mut().enumerate() {
            let mut low = f32::INFINITY;
            let mut high = f32::NEG_INFINITY;
            for end in [i, i + 1] {
                for corner in 0..4 {
                    let v = values[end * 36 + c * 12 + corner * 3 + c];
                    low = low.min(v);
                    high = high.max(v);
                }
            }
            // Leave values near a byte boundary to LittleCMS, including float roundoff.
            let byte = to_byte(low - 1e-5);
            if byte == to_byte(high + 1e-5) {
                *value = u16::from(byte);
                fixed += 1;
            }
        }
    }
    // Widely different primaries mix channels. A full transform is faster in that case.
    Ok((fixed > 2 * CELLS).then_some(table))
}

fn bounded_rgba(image: &Image, profile: &Profile, table: &[[u16; 3]]) -> Result<Vec<u8>, String> {
    let srgb = grainy_cms::profile("sRGB").map_err(|e| e.to_string())?;
    let data = image.as_standard_layout();
    let src = data.as_slice().unwrap_or_default();
    let mut rgba = vec![255; src.len() / 3 * 4];
    rgba.par_chunks_mut(4096 * 4).zip(src.par_chunks(4096 * 3)).try_for_each(|(out, input)| {
        let mut uncertain = Vec::new();
        let mut offsets = Vec::new();
        for (i, (dst, rgb)) in out.as_chunks_mut::<4>().0.iter_mut().zip(input.as_chunks::<3>().0).enumerate() {
            let byte = |c: usize| table[((rgb[c] * CELLS as f32) as usize).min(CELLS - 1)][c];
            let bytes = [byte(0), byte(1), byte(2)];
            if bytes.contains(&UNCERTAIN) || rgb.iter().any(|v| !(0.0..=1.0).contains(v)) {
                offsets.push(i * 4);
                uncertain.extend_from_slice(rgb);
            } else {
                dst[..3].copy_from_slice(&bytes.map(|v| v as u8));
            }
        }
        if !uncertain.is_empty() {
            let exact = grainy_cms::native_convert(&uncertain, &srgb, profile).map_err(|e| e.to_string())?;
            for (at, rgb) in offsets.into_iter().zip(exact.as_chunks::<3>().0) {
                out[at..at + 3].copy_from_slice(&[to_byte(rgb[0]), to_byte(rgb[1]), to_byte(rgb[2])]);
            }
        }
        Ok::<_, String>(())
    })?;
    Ok(rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_profile_byte_bounds_match_littlecms_including_cell_and_byte_edges() {
        let curves = grainy_cms::srgb_transfer_table();
        let profile = grainy_cms::rgb_profile(
            Some([0.3127, 0.3290]),
            Some([0.64004, 0.3300, 0.3000, 0.6001, 0.1500, 0.0600]),
            grainy_cms::TransferFunction::Table(&curves),
        )
        .unwrap();
        let table = bounds(&profile).unwrap().expect("near-sRGB matrix/shaper shortcut");
        let mut random = 123456789u32;
        let image = Image::from_shape_fn((192, 517, 3), |(y, x, c)| {
            random = random.wrapping_mul(1664525).wrapping_add(1013904223);
            match y % 5 {
                0 => ((x + c) % 256) as f32 / 255.,
                1 => (((x + c) % 255) as f32 + 0.5) / 255.,
                2 => ((random as usize % CELLS) as f32) / CELLS as f32,
                _ => random as f32 / u32::MAX as f32,
            }
        });
        let expected = grainy_cms::output_rgb(&image, "sRGB", Destination::Icc(&profile)).unwrap();
        assert_eq!(bounded_rgba(&image, &profile, &table).unwrap(), rgba(&expected));
        assert!(bounds(&Profile::from_bytes(vec![0; 128])).unwrap().is_none());
    }
}
