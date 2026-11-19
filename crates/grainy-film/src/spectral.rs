//! The spectral model of `luma/film_look.py`: CIE colorimetry, lights, sampled curves, chromatic adaptation
//! and dye sets. All float64, as the oracle.

use std::sync::OnceLock;

use serde_json::Value;

use crate::numeric::{interp, interp_lr, inv3, lstsq, mul3, mulv3, Matrix};
use crate::{embedded, Error, Result};

/// Scene reflectance exposed at the stock's reference exposure (`film_look.GREY`).
pub const GREY: f64 = 0.18;
/// The print's luminance of that grey, relative to the paper's white (`film_look.PRINT_GREY`).
pub const PRINT_GREY: f64 = 0.18;
/// Peak wavelengths of Status A red, green, blue densitometry (`film_look.STATUS_A`).
pub const STATUS_A: [f64; 3] = [610.0, 546.0, 436.0];
/// linear sRGB to XYZ (`film_look.SRGB_XYZ`).
pub const SRGB_XYZ: [[f64; 3]; 3] =
    [[0.4124564, 0.3575761, 0.1804375], [0.2126729, 0.7151522, 0.0721750], [0.0193339, 0.1191920, 0.9503041]];
/// The Bradford cone response matrix (`film_look.BRADFORD`).
pub const BRADFORD: [[f64; 3]; 3] = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];

/// The D65 white of sRGB (`film_look.D65`, the row sums of `SRGB_XYZ`).
pub fn d65() -> [f64; 3] {
    SRGB_XYZ.map(|r| r[0] + r[1] + r[2])
}

/// `film_look.XYZ_SRGB`.
pub fn xyz_srgb() -> [[f64; 3]; 3] {
    inv3(&SRGB_XYZ)
}

/// The CIE tables of `assets/film/colorimetry.json` on the 380–780 nm grid (port of `film_look.colorimetry`).
#[derive(Debug)]
pub struct Colorimetry {
    /// Wavelengths in nm.
    pub wavelengths: Vec<f64>,
    /// CIE 1931 colour-matching functions, n × 3.
    pub cmf: Vec<[f64; 3]>,
    /// Daylight components S0, S1, S2, n × 3.
    pub daylight: Vec<[f64; 3]>,
    /// Reflectance spectra of linear sRGB red, green, blue, n × 3.
    pub basis: Vec<[f64; 3]>,
}

fn rows3(value: &Value) -> Vec<[f64; 3]> {
    value
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|r| {
                    let n = |i: usize| r.get(i).and_then(Value::as_f64).unwrap_or(f64::NAN);
                    [n(0), n(1), n(2)]
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Port of `film_look.colorimetry` (parsed once).
pub fn colorimetry() -> &'static Colorimetry {
    static DATA: OnceLock<Colorimetry> = OnceLock::new();
    DATA.get_or_init(|| {
        let data: Value = serde_json::from_str(embedded::COLORIMETRY).unwrap_or(Value::Null);
        Colorimetry {
            wavelengths: data["wavelengths"]
                .as_array()
                .map(|a| a.iter().map(|v| v.as_f64().unwrap_or(f64::NAN)).collect())
                .unwrap_or_default(),
            cmf: rows3(&data["cmf"]),
            daylight: rows3(&data["daylight"]),
            basis: rows3(&data["basis"]),
        }
    })
}

/// Port of `film_look.wavelengths`.
pub fn wavelengths() -> &'static [f64] {
    &colorimetry().wavelengths
}

/// Port of `film_look.daylight`: CIE daylight of a correlated colour temperature (CIE 015).
pub fn daylight(kelvin: f64) -> Vec<f64> {
    let t = kelvin;
    let x = if t <= 7000.0 {
        -4.6070e9 / t.powf(3.0) + 2.9678e6 / t.powf(2.0) + 0.09911e3 / t + 0.244063
    } else {
        -2.0064e9 / t.powf(3.0) + 1.9018e6 / t.powf(2.0) + 0.24748e3 / t + 0.237040
    };
    let y = -3.0 * x * x + 2.870 * x - 0.275;
    let m = 0.0241 + 0.2562 * x - 0.7341 * y;
    let m1 = (-1.3515 - 1.7703 * x + 5.9114 * y) / m;
    let m2 = (0.0300 - 31.4424 * x + 30.0717 * y) / m;
    colorimetry().daylight.iter().map(|s| s[0] + m1 * s[1] + m2 * s[2]).collect()
}

/// Port of `film_look.blackbody`: Planck's law, 1 at 560 nm.
pub fn blackbody(kelvin: f64) -> Vec<f64> {
    let w: Vec<f64> = wavelengths().iter().map(|v| v * 1e-9).collect();
    let radiance: Vec<f64> =
        w.iter().map(|&w| 1.0 / (w.powf(5.0) * ((1.4388e-2 / (w * kelvin)).exp() - 1.0))).collect();
    let at = interp(560e-9, &w, &radiance);
    radiance.iter().map(|r| r / at).collect()
}

/// Port of `film_look.light`: 'daylight' (D55), 'tungsten' (3200 K), 'D50', 'D55', 'D65' or '<kelvin>K'.
pub fn light(name: &str) -> Result<Vec<f64>> {
    let by_kelvin = |kelvin: f64| if kelvin >= 4000.0 { daylight(kelvin) } else { blackbody(kelvin) };
    match name {
        "daylight" => return Ok(by_kelvin(5503.0)),
        "tungsten" => return Ok(by_kelvin(3200.0)),
        _ => {}
    }
    if let Some(digits) = name.strip_prefix('D') {
        if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
            let n: f64 = digits.parse().map_err(|_| Error::UnknownLight(name.to_owned()))?;
            return Ok(daylight(n * 100.0 * 1.4388 / 1.4380));
        }
    }
    if let Some(number) = name.strip_suffix('K') {
        if let Ok(kelvin) = number.trim().parse::<f64>() {
            return Ok(by_kelvin(kelvin));
        }
    }
    Err(Error::UnknownLight(name.to_owned()))
}

/// Points `[[x, y], ...]` sorted by x (port of the sort in `film_look.curve`/`sampled`).
pub(crate) fn points(value: &Value) -> (Vec<f64>, Vec<f64>) {
    let mut p: Vec<(f64, f64)> = value
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|r| {
                    let n = |i: usize| r.get(i).and_then(Value::as_f64).unwrap_or(f64::NAN);
                    (n(0), n(1))
                })
                .collect()
        })
        .unwrap_or_default();
    p.sort_by(|a, b| a.0.total_cmp(&b.0));
    p.into_iter().unzip()
}

/// Port of `film_look.sampled`: a curve `[[nm, value], ...]` on the wavelength grid, `fill` outside its range
/// (`None`: its end values).
pub fn sampled(curve: &Value, fill: Option<f64>) -> Vec<f64> {
    let (x, y) = points(curve);
    let (left, right) = match fill {
        Some(f) => (f, f),
        None => (y.first().copied().unwrap_or(f64::NAN), y.last().copied().unwrap_or(f64::NAN)),
    };
    wavelengths().iter().map(|&w| interp_lr(w, &x, &y, left, right)).collect()
}

/// Port of `film_look.adaptation`: 3×3 (Bradford) taking colours seen under `white` (XYZ) to D65, with that
/// white at Y = 1.
pub fn adaptation(white: [f64; 3]) -> [[f64; 3]; 3] {
    let source = mulv3(&BRADFORD, white.map(|v| v / white[1]));
    let target = mulv3(&BRADFORD, d65());
    let diag = [0, 1, 2].map(|i| {
        let mut row = [0.0; 3];
        row[i] = target[i] / source[i];
        row
    });
    mul3(&mul3(&inv3(&BRADFORD), &diag), &BRADFORD)
}

/// Port of `film_look.Dyes`: a film's or paper's dye set, densities by wavelength, unmixed from Status A
/// densities, with per-level strengths that keep equal densities neutral.
#[derive(Clone, Debug)]
pub struct Dyes {
    /// Cyan, magenta, yellow dye densities by wavelength (n × 3), each 1 at its own Status A band.
    pub spectra: Vec<[f64; 3]>,
    /// The base (minimum) density by wavelength.
    pub base: Vec<f64>,
    /// The base density at the three Status A bands.
    pub base_bands: [f64; 3],
    /// Status A densities above the base to dye amounts.
    pub unmix: [[f64; 3]; 3],
    /// The viewing light's spectrum.
    pub view: Vec<f64>,
    /// Colour-matching functions times the viewing light (n × 3).
    pub weights: Vec<[f64; 3]>,
    /// Adaptation of the base's colour to D65 white, divided by its Y.
    pub adapt: [[f64; 3]; 3],
    /// Density levels of the neutral calibration.
    pub levels: Vec<f64>,
    /// Dye strengths per level (levels × 3).
    pub strengths: Vec<[f64; 3]>,
}

impl Dyes {
    /// Port of `Dyes.__init__(dyes, minimum, view)`.
    pub fn new(dyes: &Value, minimum: [f64; 3], view: &str) -> Result<Self> {
        let wl = wavelengths();
        let n = wl.len();
        let columns: Vec<Vec<f64>> = ["c", "m", "y"]
            .iter()
            .map(|k| dyes.get(*k).map(|c| sampled(c, None)).ok_or_else(|| Error::BadData(format!("dye {k}"))))
            .collect::<Result<_>>()?;
        let mut base = dyes.get("min").map(|c| sampled(c, Some(f64::NAN)));
        // peaks[band][dye]
        let peaks: [[f64; 3]; 3] = STATUS_A.map(|w| [0, 1, 2].map(|j| interp(w, wl, &columns[j])));
        let spectra: Vec<[f64; 3]> = (0..n).map(|i| [0, 1, 2].map(|j| columns[j][i] / peaks[j][j])).collect();
        let unmix = inv3(&[0, 1, 2].map(|b| [0, 1, 2].map(|j| peaks[b][j] / peaks[j][j])));
        let base = match base.take() {
            Some(b) if !b.iter().all(|v| v.is_nan()) => {
                let lowest = b.iter().copied().filter(|v| !v.is_nan()).fold(f64::INFINITY, f64::min);
                b.into_iter().map(|v| if v.is_nan() { lowest } else { v }).collect()
            }
            _ => {
                let xp = [STATUS_A[2], STATUS_A[1], STATUS_A[0]];
                let fp = [minimum[2], minimum[1], minimum[0]];
                wl.iter().map(|&w| interp(w, &xp, &fp)).collect::<Vec<f64>>()
            }
        };
        let base_bands = STATUS_A.map(|w| interp(w, wl, &base));
        let view_light = light(view)?;
        let cmf = &colorimetry().cmf;
        let weights: Vec<[f64; 3]> = cmf.iter().zip(&view_light).map(|(c, v)| c.map(|x| x * v)).collect();
        let mut white = [0.0; 3];
        for (b, w) in base.iter().zip(&weights) {
            let t = 10f64.powf(-b);
            for k in 0..3 {
                white[k] += t * w[k];
            }
        }
        let adapt = adaptation(white).map(|r| r.map(|v| v / white[1]));
        let levels = crate::numeric::linspace(0.05, 3.5, 24);
        let mut dyes = Dyes {
            spectra,
            base,
            base_bands,
            unmix,
            view: view_light,
            weights,
            adapt,
            levels: levels.clone(),
            strengths: vec![[1.0; 3]; levels.len()],
        };
        let mut strength = [1.0f64; 3];
        let d65 = d65();
        let target = d65.map(|v| (v / d65[1]).log10());
        for (i, &level) in levels.iter().enumerate() {
            let densities = dyes.base_bands.map(|b| b + level);
            let error = |s: [f64; 3]| {
                let c = dyes.colour_with(densities, s);
                [0, 1, 2].map(|k| -(c[k].max(1e-30)).log10() - (level - target[k]))
            };
            for _ in 0..40 {
                let e = error(strength);
                if e.iter().fold(0.0f64, |m, v| m.max(v.abs())) < 1e-7 {
                    break;
                }
                // jacobian[:, h] = (error(strength*(1 + h)) - e)/1e-6
                let mut jacobian: Matrix = vec![vec![0.0; 3]; 3];
                for h in 0..3 {
                    let mut s = strength;
                    s[h] *= 1.0 + 1e-6;
                    let eh = error(s);
                    for k in 0..3 {
                        jacobian[k][h] = (eh[k] - e[k]) / 1e-6;
                    }
                }
                let step = lstsq(&jacobian, &e.map(|v| -v));
                for k in 0..3 {
                    strength[k] = (strength[k] * step[k].clamp(-0.5, 0.5).exp()).clamp(0.05, 20.0);
                }
            }
            dyes.strengths[i] = strength;
        }
        Ok(dyes)
    }

    /// Port of `Dyes._colour`: XYZ of total Status A densities with given dye strengths.
    pub(crate) fn colour_with(&self, densities: [f64; 3], strength: [f64; 3]) -> [f64; 3] {
        let d = [0, 1, 2].map(|k| densities[k] - self.base_bands[k]);
        let amounts = [0, 1, 2].map(|j| {
            let a = d[0] * self.unmix[j][0] + d[1] * self.unmix[j][1] + d[2] * self.unmix[j][2];
            (if a.is_nan() { a } else { a.max(0.0) }) * strength[j]
        });
        let mut xyz = [0.0; 3];
        for ((s, b), w) in self.spectra.iter().zip(&self.base).zip(&self.weights) {
            let spectrum = amounts[0] * s[0] + amounts[1] * s[1] + amounts[2] * s[2] + b;
            let t = 10f64.powf(-spectrum);
            for k in 0..3 {
                xyz[k] += t * w[k];
            }
        }
        [0, 1, 2].map(|c| xyz[0] * self.adapt[c][0] + xyz[1] * self.adapt[c][1] + xyz[2] * self.adapt[c][2])
    }

    /// Port of `Dyes.colour`: XYZ (D65, white = 1) of total Status A densities.
    pub fn colour(&self, densities: [f64; 3]) -> [f64; 3] {
        let level = ((densities[0] - self.base_bands[0])
            + (densities[1] - self.base_bands[1])
            + (densities[2] - self.base_bands[2]))
            / 3.0;
        let strength = [0, 1, 2].map(|j| {
            let column: Vec<f64> = self.strengths.iter().map(|s| s[j]).collect();
            interp(level, &self.levels, &column)
        });
        self.colour_with(densities, strength)
    }
}
