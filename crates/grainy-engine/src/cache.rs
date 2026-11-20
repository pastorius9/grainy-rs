//! Port of luma/render_cache.py: the bounded cache of intermediate frames that one preview worker owns.
//!
//! A develop with a cache reuses the stages whose settings did not change, without approximating their pixels:
//! moving a detail slider re-runs only the detail stage. Switching to another source drops every entry. Exports
//! do not use a cache.

use std::sync::Arc;

use grainy_core::{Image, Settings};

/// Settings of the RAW camera colour stage (`rawcolor.RAW_KEYS` and `negative`).
pub const RAW_KEYS: [&str; 11] = [
    "raw_mode",
    "raw_kelvin",
    "raw_tint",
    "raw_neutral",
    "dcp_profile",
    "dcp_huesat",
    "dcp_look",
    "dcp_tone",
    "dcp_exposure",
    "working_space",
    "negative",
];
/// Settings of the geometry stage (`engine.GEOMETRY_KEYS`).
pub const GEOMETRY_KEYS: [&str; 25] = [
    "retouch",
    "rotation",
    "straighten",
    "flip",
    "crop",
    "distortion",
    "distortion_k2",
    "distortion_k3",
    "perspective_h",
    "perspective_v",
    "aspect_scale",
    "shift_x",
    "shift_y",
    "transform_scale",
    "guides",
    "ca_red",
    "ca_blue",
    "lensfun",
    "lensfun_enabled",
    "lensfun_distortion",
    "lensfun_tca",
    "lensfun_vignette",
    "lensfun_scale",
    "upright",
    "upright_crop",
];
/// Settings of the tone stage (`engine.TONE_KEYS`).
pub const TONE_KEYS: [&str; 23] = [
    "exposure",
    "temperature",
    "tint",
    "kelvin",
    "wb_reference",
    "kelvin_enabled",
    "lens_vignette",
    "shadows",
    "highlights",
    "blacks",
    "whites",
    "contrast",
    "curve",
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
/// Settings of the colour stage (`engine.COLOR_KEYS`).
pub const COLOR_KEYS: [&str; 16] = [
    "saturation",
    "vibrance",
    "mixer_mode",
    "hsl",
    "rgb_curves",
    "parametric",
    "calibration",
    "point_colors",
    "grading",
    "grading_balance",
    "camera_matrix",
    "photo_filter",
    "photo_filter_enabled",
    "photo_filter_density",
    "photo_filter_luminosity",
    "monochrome",
];
/// Settings of the detail stage (`engine.DETAIL_KEYS`).
pub const DETAIL_KEYS: [&str; 20] = [
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
    "grain",
    "detail_version",
    "vignette_midpoint",
    "vignette_roundness",
    "vignette_feather",
    "vignette_highlights",
    "grain_size",
    "grain_roughness",
];

/// Port of `render_cache.settings_key`: a text that is equal exactly when the values of `keys` are equal.
/// (The oracle writes sorted JSON; nested objects keep their stored order here, which can only cost a miss.)
pub fn settings_key(s: &Settings, keys: &[&str]) -> String {
    let mut sorted = keys.to_vec();
    sorted.sort_unstable();
    let mut out = String::from("{");
    for (index, key) in sorted.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push('"');
        out.push_str(key);
        out.push_str("\":");
        let value = s.value(key);
        if *key == "dcp_profile" && value.is_object() {
            // Hash the actual record, never trust its self-reported digest. Later stages nest this key.
            use sha2::{Digest, Sha256};
            let mut hash = Sha256::new();
            serde_json::to_writer(&mut hash, value).expect("writing a JSON value to a digest cannot fail");
            out.push_str(&format!("#{:x}", hash.finalize()));
        } else {
            out.push_str(&value.to_string());
        }
    }
    out.push('}');
    out
}

fn bytes_of(image: &Image) -> usize {
    image.len() * std::mem::size_of::<f32>()
}

/// Port of `render_cache.DevelopmentCache`.
#[derive(Debug)]
pub struct DevelopmentCache {
    base_bytes: usize,
    max_bytes: usize,
    per_source: f64,
    ceiling: usize,
    bytes: usize,
    hits: u64,
    misses: u64,
    source: Option<u64>,
    version: u64,
    entries: Vec<(String, String, Arc<Image>)>,
}

impl Default for DevelopmentCache {
    fn default() -> Self {
        Self::new(128 * 1024 * 1024)
    }
}

impl DevelopmentCache {
    /// A cache with a fixed budget in bytes.
    pub fn new(max_bytes: usize) -> Self {
        Self::growing(max_bytes, 0.0, None)
    }

    /// `per_source > 0` grows the budget to that many copies of each bound source, up to `ceiling`. A
    /// full-resolution frame larger than a fixed budget otherwise disables the cache, and every slider event
    /// recomputes the whole chain.
    pub fn growing(max_bytes: usize, per_source: f64, ceiling: Option<usize>) -> Self {
        Self {
            base_bytes: max_bytes,
            max_bytes,
            per_source: per_source.max(0.0),
            ceiling: ceiling.map_or(max_bytes, |ceiling| ceiling.max(max_bytes)),
            bytes: 0,
            hits: 0,
            misses: 0,
            source: None,
            version: 0,
            entries: Vec::new(),
        }
    }

    /// Binds the cache to a source. The oracle compares source objects; here the caller gives each loaded
    /// source its own `token` (a counter is enough). A different token drops every entry.
    pub fn bind(&mut self, token: u64, source_bytes: usize) {
        if self.source != Some(token) {
            self.entries.clear();
            self.bytes = 0;
            self.source = Some(token);
            self.version += 1;
            if self.per_source > 0.0 {
                let wanted = (self.per_source * source_bytes as f64) as usize;
                self.max_bytes = self.base_bytes.max(wanted.min(self.ceiling));
            }
        }
    }

    /// The budget in bytes.
    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    /// Bytes held now.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Lookups answered from the cache.
    pub fn hits(&self) -> u64 {
        self.hits
    }

    /// Stage results that had to be computed.
    pub fn misses(&self) -> u64 {
        self.misses
    }

    /// Changes with the bound source; keys of results kept outside the cache (on the GPU) include it.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// The cached value of `stage` when its key is `key`; a stale entry of the stage is dropped.
    pub fn lookup(&mut self, stage: &str, key: &str) -> Option<Arc<Image>> {
        let position = self.entries.iter().position(|(name, _, _)| name == stage)?;
        let entry = self.entries.remove(position);
        if entry.1 == key {
            let value = Arc::clone(&entry.2);
            self.entries.push(entry);
            self.hits += 1;
            return Some(value);
        }
        self.bytes -= bytes_of(&entry.2);
        None
    }

    /// Drops the entries of these stages.
    pub fn discard(&mut self, stages: &[&str]) {
        for stage in stages {
            if let Some(position) = self.entries.iter().position(|(name, _, _)| name == stage) {
                let entry = self.entries.remove(position);
                self.bytes -= bytes_of(&entry.2);
            }
        }
    }

    /// Keeps a computed stage result when it fits. The prefix that already fits stays: evicting it to store
    /// later stages would cause a full chain of misses on every following slider event.
    pub fn store(&mut self, stage: &str, key: &str, value: Image) -> Arc<Image> {
        self.misses += 1;
        let value = Arc::new(value);
        let size = bytes_of(&value);
        if self.bytes + size <= self.max_bytes && self.entries.len() < 64 {
            self.entries.push((stage.to_owned(), key.to_owned(), Arc::clone(&value)));
            self.bytes += size;
        }
        value
    }

    /// The cached value, or the computed one (stored when it fits).
    pub fn evaluate<E>(
        &mut self,
        stage: &str,
        key: &str,
        calculate: impl FnOnce() -> Result<Image, E>,
    ) -> Result<Arc<Image>, E> {
        match self.lookup(stage, key) {
            Some(value) => Ok(value),
            None => Ok(self.store(stage, key, calculate()?)),
        }
    }
}

#[cfg(test)]
#[test]
fn embedded_profile_cache_key_is_bounded_and_covers_the_payload() {
    let mut s = grainy_core::defaults();
    let data = "a".repeat(65536);
    s.set("dcp_profile", serde_json::json!({"data":data,"sha256":"same"}));
    let first = settings_key(&s, &RAW_KEYS);
    assert!(first.len() < 1024);
    s.set("dcp_profile", serde_json::json!({"data":"different","sha256":"same"}));
    assert_ne!(settings_key(&s, &RAW_KEYS), first);
}
