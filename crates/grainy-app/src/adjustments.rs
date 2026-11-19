//! The Develop sliders: every `add_adjustment` of the oracle (luma/app.py `build_ui`) and every `scalar` of
//! luma/studio.py, with the same keys, Korean labels, ranges and scales, in the order the oracle adds them.
//!
//! A slider runs over the integers `low*scale ..= high*scale` and the value is `position / scale`, as the
//! oracle's `QSlider` does (`Adjustment` in luma/widgets.py).

/// What a double-click on the slider resets to (`Adjustment.default_value`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reset {
    /// `add_adjustment` in luma/app.py leaves `default_value = 0`. Oracle quirk kept: for `negative_range`
    /// (range 3–10, setting default 6.5), `hdr_limit` (default 4) and `sdr_compression` (default 75) a
    /// double-click goes to the slider position nearest 0, not to the setting's default.
    Zero,
    /// `StudioTools.scalar` in luma/studio.py: the setting's default (`defaults()[key]`).
    Default,
    /// An explicit value (the Effects sliders: `default_value = default`).
    Value(f64),
}

/// Where a slider sits in the Develop column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Place {
    /// Basic › the negative's controls (shown while Invert Negative is on).
    Negative,
    /// Basic › White balance.
    WhiteBalance,
    /// Basic › White balance › the RAW sliders (RAW photos only).
    Raw,
    /// Basic › White balance › Advanced White Balance (a fold-out).
    WhiteBalanceDetail,
    /// Basic › Tone › the DRT's Advanced sliders.
    Drt,
    /// Basic › Tone.
    Tone,
    /// Basic › HDR (shown by `crate::hdr::panel` while the HDR mode is on and the photo is in HDR mode).
    Hdr,
    /// Basic › Presence.
    Presence,
    /// Basic, after Presence (vibrance, saturation).
    Color,
    /// A panel's body, by panel key.
    Panel(&'static str),
    /// Lens Corrections › Manual Lens Correction (a fold-out).
    LensManual,
    /// The Crop tool.
    Crop,
}

/// One slider row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spec {
    /// The settings key.
    pub key: &'static str,
    /// The Korean label (source text for `tr`).
    pub label: &'static str,
    pub low: f64,
    pub high: f64,
    /// Slider steps per unit (1, 10 or 100).
    pub scale: f64,
    pub reset: Reset,
    pub place: Place,
}

const fn s(
    key: &'static str,
    label: &'static str,
    low: f64,
    high: f64,
    scale: f64,
    reset: Reset,
    place: Place,
) -> Spec {
    Spec { key, label, low, high, scale, reset, place }
}

use Place::*;
use Reset::{Default as D, Value as V, Zero as Z};

/// The 58 sliders, in the order the oracle builds them within each place.
pub static ADJUSTMENTS: [Spec; 58] = [
    // luma/app.py build_ui, Basic panel
    s("negative_density", "필름 농도", -3.0, 3.0, 100.0, Z, Negative),
    s("negative_range", "필름 대비", 3.0, 10.0, 10.0, Z, Negative),
    s("temperature", "색온도 (상대값)", -100.0, 100.0, 1.0, Z, WhiteBalance),
    s("tint", "색조", -100.0, 100.0, 1.0, Z, WhiteBalance),
    // luma/studio.py build_color: the RAW white balance and Advanced White Balance
    s("raw_kelvin", "RAW 색온도 K", 2000.0, 50000.0, 1.0, D, Raw),
    s("raw_tint", "RAW 색조", -150.0, 150.0, 10.0, D, Raw),
    s("kelvin", "보정 색온도 K", 2000.0, 50000.0, 1.0, D, WhiteBalanceDetail),
    s("wb_reference", "기준 백색점 K", 2000.0, 50000.0, 1.0, D, WhiteBalanceDetail),
    // Tone: the DRT's advanced sliders, then the tone sliders
    s("drt_contrast", "DRT 대비", -100.0, 100.0, 1.0, Z, Drt),
    s("drt_toe", "토 (어두운 끝)", -100.0, 100.0, 1.0, Z, Drt),
    s("drt_shoulder", "숄더 (밝은 끝)", -100.0, 100.0, 1.0, Z, Drt),
    s("drt_white", "흰색 전환 시작", -100.0, 100.0, 1.0, Z, Drt),
    s("exposure", "노출", -5.0, 5.0, 100.0, Z, Tone),
    s("contrast", "대비", -100.0, 100.0, 1.0, Z, Tone),
    s("highlights", "하이라이트", -100.0, 100.0, 1.0, Z, Tone),
    s("shadows", "그림자", -100.0, 100.0, 1.0, Z, Tone),
    s("whites", "흰색 계열", -200.0, 200.0, 1.0, Z, Tone),
    s("blacks", "검정 계열", -200.0, 200.0, 1.0, Z, Tone),
    s("hdr_limit", "HDR 한계 (스톱)", 0.0, 4.0, 10.0, Z, Hdr),
    s("hdr_brightness", "HDR 영역 밝기", -100.0, 100.0, 1.0, Z, Hdr),
    s("sdr_exposure", "SDR 미리보기 노출", -5.0, 5.0, 100.0, Z, Hdr),
    s("sdr_compression", "SDR 하이라이트 압축", 0.0, 100.0, 1.0, Z, Hdr),
    // Presence: texture is inserted before clarity by luma/studio.py build_detail, dehaze after it
    s("texture", "텍스처", -100.0, 100.0, 1.0, D, Presence),
    s("clarity", "부분 대비", -100.0, 100.0, 1.0, Z, Presence),
    s("dehaze", "안개 제거", -100.0, 100.0, 1.0, D, Presence),
    s("vibrance", "생동감", -100.0, 100.0, 1.0, Z, Color),
    s("saturation", "채도", -100.0, 100.0, 1.0, Z, Color),
    // Film (luma/studio.py build_film)
    s("film_strength", "필름 강도", 0.0, 100.0, 1.0, D, Panel("film")),
    // Color Grading (build_color)
    s("grading_balance", "그레이딩 균형", -100.0, 100.0, 1.0, D, Panel("grading")),
    // Detail: sharpen from luma/app.py, the rest from build_detail
    s("sharpen", "선명하게", 0.0, 150.0, 1.0, Z, Panel("detail")),
    s("sharpen_radius", "선명도 반경", 0.2, 5.0, 100.0, D, Panel("detail")),
    s("sharpen_detail", "선명도 디테일", 0.0, 100.0, 1.0, D, Panel("detail")),
    s("sharpen_mask", "선명도 마스킹", 0.0, 100.0, 1.0, D, Panel("detail")),
    s("noise_luma", "휘도 노이즈 제거", 0.0, 100.0, 1.0, D, Panel("detail")),
    s("noise_color", "색상 노이즈 제거", 0.0, 100.0, 1.0, D, Panel("detail")),
    // Lens Corrections › Manual Lens Correction
    s("distortion", "왜곡 1차", -100.0, 100.0, 1.0, D, LensManual),
    s("distortion_k2", "왜곡 2차", -100.0, 100.0, 1.0, D, LensManual),
    s("distortion_k3", "왜곡 3차", -100.0, 100.0, 1.0, D, LensManual),
    s("lens_vignette", "렌즈 주변광량", -100.0, 100.0, 1.0, D, LensManual),
    s("ca_red", "빨강 색수차", -100.0, 100.0, 1.0, D, LensManual),
    s("ca_blue", "파랑 색수차", -100.0, 100.0, 1.0, D, LensManual),
    s("defringe", "보라색 프린지 제거", 0.0, 100.0, 1.0, D, LensManual),
    // Transform
    s("perspective_v", "수직 원근", -100.0, 100.0, 1.0, D, Panel("transform")),
    s("perspective_h", "수평 원근", -100.0, 100.0, 1.0, D, Panel("transform")),
    s("aspect_scale", "가로 비율", -100.0, 100.0, 1.0, D, Panel("transform")),
    s("shift_x", "가로 이동", -100.0, 100.0, 1.0, D, Panel("transform")),
    s("shift_y", "세로 이동", -100.0, 100.0, 1.0, D, Panel("transform")),
    s("transform_scale", "확대", 50.0, 200.0, 1.0, D, Panel("transform")),
    // Effects (luma/app.py: explicit reset values)
    s("vignette", "비네팅", 0.0, 100.0, 1.0, V(0.0), Panel("effects")),
    s("vignette_midpoint", "중간점", 0.0, 100.0, 1.0, V(50.0), Panel("effects")),
    s("vignette_roundness", "둥글기", -100.0, 100.0, 1.0, V(0.0), Panel("effects")),
    s("vignette_feather", "페더", 0.0, 100.0, 1.0, V(50.0), Panel("effects")),
    s("vignette_highlights", "하이라이트 보호", 0.0, 100.0, 1.0, V(0.0), Panel("effects")),
    s("grain", "그레인", 0.0, 100.0, 1.0, V(0.0), Panel("effects")),
    s("grain_size", "크기", 0.0, 100.0, 1.0, V(25.0), Panel("effects")),
    s("grain_roughness", "거칠기", 0.0, 100.0, 1.0, V(50.0), Panel("effects")),
    // Color Filter
    s("photo_filter_density", "필터 강도", 0.0, 100.0, 1.0, D, Panel("photo_filter")),
    // The Crop tool
    s("straighten", "수평 맞추기", -45.0, 45.0, 10.0, Z, Crop),
];

/// The slider of `key`.
pub fn spec(key: &str) -> Option<&'static Spec> {
    ADJUSTMENTS.iter().find(|a| a.key == key)
}

/// The sliders of one place, in order.
pub fn at(place: Place) -> impl Iterator<Item = &'static Spec> {
    ADJUSTMENTS.iter().filter(move |a| a.place == place)
}

impl Spec {
    /// Slider range in steps.
    pub fn steps(&self) -> (i64, i64) {
        ((self.low * self.scale) as i64, (self.high * self.scale) as i64)
    }

    /// Decimals of the number field: `ceil(log10(scale))`.
    pub fn decimals(&self) -> usize {
        self.scale.log10().ceil().max(0.0) as usize
    }

    /// The slider position of a value (`round(value*scale)` clamped to the range, as `QSlider.setValue`).
    pub fn position(&self, value: f64) -> i64 {
        let (low, high) = self.steps();
        // Python's round() is round-half-even.
        let scaled = value * self.scale;
        let rounded = scaled.round();
        let rounded = if (scaled - scaled.trunc()).abs() == 0.5 { 2.0 * (scaled / 2.0).round() } else { rounded };
        (rounded as i64).clamp(low, high)
    }

    /// The value at a slider position.
    pub fn value_at(&self, position: i64) -> f64 {
        position as f64 / self.scale
    }

    /// The value a double-click resets to, after the slider has clamped it.
    pub fn reset_value(&self) -> f64 {
        let target = match self.reset {
            Reset::Zero => 0.0,
            Reset::Default => grainy_core::defaults().f64(self.key),
            Reset::Value(v) => v,
        };
        self.value_at(self.position(target))
    }

    /// The number field's text: `f'{value:+.{decimals}f}'`.
    pub fn format(&self, value: f64) -> String {
        format!("{:+.*}", self.decimals(), value)
    }

    /// Reads a typed number (an optional sign, a decimal point or comma), clamped to the range and snapped to a
    /// slider step, as the oracle's spin box and `number_changed` do. `None` when it is not a number.
    pub fn parse(&self, text: &str) -> Option<f64> {
        let cleaned: String = text.trim().replace(',', ".").chars().filter(|c| !c.is_whitespace()).collect();
        let number: f64 = cleaned.parse().ok()?;
        if !number.is_finite() {
            return None;
        }
        Some(self.value_at(self.position(number.clamp(self.low, self.high))))
    }
}
