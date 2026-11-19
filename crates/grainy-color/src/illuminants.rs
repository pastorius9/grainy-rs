//! DNG custom illuminants and three-light interpolation: port of `luma/illuminants.py`.
//!
//! Interpolation and observer normalisation follow Adobe DNG SDK 1.7.1 (as the oracle does). The CIE 1931
//! observer table in `assets/CIE_xyz_1931_2deg.csv` is CC BY-SA 4.0, see `assets/ILLUMINANTS-NOTICE.txt`.
//! All arithmetic is `f64`, as in the oracle.

use std::sync::LazyLock;

use crate::{ColorError, Result};

/// Port of `STANDARD_XY`: chromaticity of the standard EXIF/DNG light-source codes.
pub const STANDARD_XY: [(i64, [f64; 2]); 19] = [
    (1, [0.3324, 0.3474]),
    (2, [0.37208, 0.37529]),
    (3, [0.4476, 0.4074]),
    (4, [0.3324, 0.3474]),
    (9, [0.3324, 0.3474]),
    (10, [0.3127, 0.3290]),
    (11, [0.2990, 0.3149]),
    (12, [0.31310, 0.33727]),
    (13, [0.34588, 0.35875]),
    (14, [0.37417, 0.37281]),
    (15, [0.40910, 0.39430]),
    (16, [0.44018, 0.40329]),
    (17, [0.4476, 0.4074]),
    (18, [0.348483, 0.351747]),
    (19, [0.310061, 0.316150]),
    (20, [0.3324, 0.3474]),
    (21, [0.3127, 0.3290]),
    (22, [0.2990, 0.3149]),
    (23, [0.3457, 0.3585]),
];

/// Robertson isotemperature lines (mired, u, v, slope): the oracle's `rawcolor.LINES` with the 325-mired `u`
/// replaced by the SDK value .24702 (`_sdk_temperature_table`).
const LINES: [[f64; 4]; 31] = [
    [0.0, 0.18006, 0.26352, -0.24341],
    [10.0, 0.18066, 0.26589, -0.25479],
    [20.0, 0.18133, 0.26846, -0.26876],
    [30.0, 0.18208, 0.27119, -0.28539],
    [40.0, 0.18293, 0.27407, -0.30470],
    [50.0, 0.18388, 0.27709, -0.32675],
    [60.0, 0.18494, 0.28021, -0.35156],
    [70.0, 0.18611, 0.28342, -0.37915],
    [80.0, 0.18740, 0.28668, -0.40955],
    [90.0, 0.18880, 0.28997, -0.44278],
    [100.0, 0.19032, 0.29326, -0.47888],
    [125.0, 0.19462, 0.30141, -0.58204],
    [150.0, 0.19962, 0.30921, -0.70471],
    [175.0, 0.20525, 0.31647, -0.84901],
    [200.0, 0.21142, 0.32312, -1.0182],
    [225.0, 0.21807, 0.32909, -1.2168],
    [250.0, 0.22511, 0.33439, -1.4512],
    [275.0, 0.23247, 0.33904, -1.7298],
    [300.0, 0.24010, 0.34308, -2.0637],
    [325.0, 0.24702, 0.34655, -2.4681],
    [350.0, 0.25591, 0.34951, -2.9641],
    [375.0, 0.26400, 0.35200, -3.5814],
    [400.0, 0.27218, 0.35407, -4.3633],
    [425.0, 0.28039, 0.35577, -5.3762],
    [450.0, 0.28863, 0.35714, -6.7262],
    [475.0, 0.29685, 0.35823, -8.5955],
    [500.0, 0.30505, 0.35907, -11.324],
    [525.0, 0.31320, 0.35968, -15.628],
    [550.0, 0.32129, 0.36011, -23.325],
    [575.0, 0.32931, 0.36038, -40.770],
    [600.0, 0.33724, 0.36051, -116.45],
];

/// Unit direction vectors of the isotemperature lines: `rawcolor.DIRECTIONS` (`[1, slope]` normalised).
static DIRECTIONS: LazyLock<[[f64; 2]; 31]> = LazyLock::new(|| {
    LINES.map(|line| {
        let norm = (1.0 + line[3] * line[3]).sqrt();
        [1.0 / norm, line[3] / norm]
    })
});

fn bad(message: &'static str) -> ColorError {
    ColorError::Illuminant(message)
}

fn normalize(v: [f64; 2]) -> [f64; 2] {
    let norm = (v[0] * v[0] + v[1] * v[1]).sqrt();
    [v[0] / norm, v[1] / norm]
}

/// Port of `xyz_to_xy`: XYZ white to xy chromaticity. Errors for non-finite values or a sum <= 0.
pub fn xyz_to_xy(xyz: [f64; 3]) -> Result<[f64; 2]> {
    let sum = xyz[0] + xyz[1] + xyz[2];
    if !xyz.iter().all(|v| v.is_finite()) || sum <= 0.0 {
        return Err(bad("camera white point is not valid"));
    }
    Ok([xyz[0] / sum, xyz[1] / sum])
}

/// Port of `xy_to_xyz`: xy chromaticity to XYZ with Y = 1. Both coordinates are clipped to [1e-6, .999999] and
/// scaled down together when their sum exceeds .999999.
pub fn xy_to_xyz(xy: [f64; 2]) -> Result<[f64; 3]> {
    if !xy.iter().all(|v| v.is_finite()) {
        return Err(bad("illuminant chromaticity is not valid"));
    }
    let mut a = xy.map(|v| v.clamp(1e-6, 0.999999));
    let sum = a[0] + a[1];
    if sum > 0.999999 {
        let scale = 0.999999 / sum;
        a = a.map(|v| v * scale);
    }
    let (x, y) = (a[0], a[1]);
    Ok([x / y, 1.0, (1.0 - x - y) / y])
}

/// Port of `temperature_tint`: SDK Robertson method, xy to `(kelvin, tint)`. Errors for non-finite input or
/// `1.5 - x + 6y <= 0`.
pub fn temperature_tint(xy: [f64; 2]) -> Result<(f64, f64)> {
    let (x, y) = (xy[0], xy[1]);
    if !xy.iter().all(|v| v.is_finite()) || 1.5 - x + 6.0 * y <= 0.0 {
        return Err(bad("cannot compute a colour temperature from this chromaticity"));
    }
    let directions = &*DIRECTIONS;
    let denominator = 1.5 - x + 6.0 * y;
    let uv = [2.0 * x / denominator, 3.0 * y / denominator];
    let distances: Vec<f64> = LINES
        .iter()
        .zip(directions.iter())
        .map(|(line, d)| (uv[1] - line[2]) * d[0] - (uv[0] - line[1]) * d[1])
        .collect();
    let i = (1..distances.len()).find(|&k| distances[k] <= 0.0).unwrap_or(30);
    let dt = (-distances[i]).max(0.0);
    let f = if i == 1 { 0.0 } else { dt / (distances[i - 1] + dt) };
    let mired = LINES[i - 1][0] * f + LINES[i][0] * (1.0 - f);
    let locus = [LINES[i - 1][1] * f + LINES[i][1] * (1.0 - f), LINES[i - 1][2] * f + LINES[i][2] * (1.0 - f)];
    let direction = normalize([
        directions[i - 1][0] * f + directions[i][0] * (1.0 - f),
        directions[i - 1][1] * f + directions[i][1] * (1.0 - f),
    ]);
    let tint = -((uv[0] - locus[0]) * direction[0] + (uv[1] - locus[1]) * direction[1]) * 3000.0;
    Ok((1e6 / mired, tint))
}

/// Port of `temperature_xy`: SDK temperature (and tint) to xy. Errors for non-finite input or `kelvin <= 0`.
pub fn temperature_xy(kelvin: f64, tint: f64) -> Result<[f64; 2]> {
    if !kelvin.is_finite() || !tint.is_finite() || kelvin <= 0.0 {
        return Err(bad("illuminant temperature must be a positive number"));
    }
    let directions = &*DIRECTIONS;
    let r = 1e6 / kelvin;
    // numpy searchsorted(lines[1:, 0], r, side='right'): how many entries are <= r
    let count = LINES[1..].iter().take_while(|l| l[0] <= r).count();
    let i = count.min(29);
    let f = (LINES[i + 1][0] - r) / (LINES[i + 1][0] - LINES[i][0]);
    let mut uv = [LINES[i][1] * f + LINES[i + 1][1] * (1.0 - f), LINES[i][2] * f + LINES[i + 1][2] * (1.0 - f)];
    let direction = normalize([
        directions[i][0] * f + directions[i + 1][0] * (1.0 - f),
        directions[i][1] * f + directions[i + 1][1] * (1.0 - f),
    ]);
    let shift = tint / -3000.0;
    uv[0] += direction[0] * shift;
    uv[1] += direction[1] * shift;
    let (u, v) = (uv[0], uv[1]);
    let denominator = u - 4.0 * v + 2.0;
    Ok([1.5 * u / denominator, v / denominator])
}

static OBSERVER: LazyLock<std::result::Result<Vec<[f64; 4]>, &'static str>> = LazyLock::new(|| {
    let text = include_str!("../assets/CIE_xyz_1931_2deg.csv");
    let mut rows = Vec::with_capacity(471);
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let fields: Vec<f64> = line
            .split(',')
            .map(|f| f.trim().parse::<f64>())
            .collect::<std::result::Result<_, _>>()
            .map_err(|_| "observer table is not numeric")?;
        let row: [f64; 4] = fields.try_into().map_err(|_| "observer table needs four columns")?;
        rows.push(row);
    }
    if rows.len() != 471 || rows.iter().enumerate().any(|(i, r)| r[0] != (360 + i) as f64) {
        return Err("observer table range is wrong");
    }
    Ok(rows)
});

/// Port of `observer`: the CIE 1931 2-degree colour-matching functions, rows `[wavelength nm, x, y, z]` for
/// 360..=830 nm. (The oracle also checks the file's SHA-256; the table is embedded unchanged here.)
pub fn observer() -> Result<&'static [[f64; 4]]> {
    OBSERVER.as_deref().map_err(|message| bad(message))
}

pub use grainy_core::numpy::interp;

/// Port of `custom_xy`: the white point of a DNG custom light source (tag 52532 payload): kind 0 = xy
/// chromaticity, kind 1 = sampled spectrum. `little_endian` is the TIFF byte order of the payload.
pub fn custom_xy(payload: &[u8], little_endian: bool) -> Result<[f64; 2]> {
    if !(2..=8022).contains(&payload.len()) {
        return Err(bad("custom illuminant data has the wrong size"));
    }
    let u16_at = |at: usize| {
        let b = [payload[at], payload[at + 1]];
        if little_endian {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        }
    };
    let u32_at = |at: usize| {
        let b = [payload[at], payload[at + 1], payload[at + 2], payload[at + 3]];
        if little_endian {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        }
    };
    // `count` rationals starting at `offset`; the caller has checked the length.
    let rational = |offset: usize, count: usize| -> Result<Vec<f64>> {
        (0..count)
            .map(|k| {
                let (n, d) = (u32_at(offset + 8 * k), u32_at(offset + 8 * k + 4));
                if d == 0 {
                    Err(bad("custom illuminant denominator is zero"))
                } else {
                    Ok(n as f64 / d as f64)
                }
            })
            .collect()
    };
    match u16_at(0) {
        0 => {
            if payload.len() != 18 {
                return Err(bad("custom illuminant chromaticity data has the wrong length"));
            }
            let xy = rational(2, 2)?;
            if xy.iter().any(|v| *v < 1e-6 || *v > 0.999999) {
                return Err(bad("illuminant chromaticity is out of range"));
            }
            xyz_to_xy(xy_to_xyz([xy[0], xy[1]])?)
        }
        1 => {
            if payload.len() < 22 {
                return Err(bad("illuminant spectrum is incomplete"));
            }
            let count = u32_at(2) as usize;
            if !(2..=1000).contains(&count) || payload.len() != 22 + 8 * count {
                return Err(bad("illuminant spectrum sample count is wrong"));
            }
            let start_step = rational(6, 2)?;
            let (start, step) = (start_step[0], start_step[1]);
            if start <= 0.0 || step <= 0.0 {
                return Err(bad("illuminant wavelength and step must be positive"));
            }
            let samples = rational(22, count)?;
            let table = observer()?;
            let wavelengths: Vec<f64> = (0..count).map(|k| start + k as f64 * step).collect();
            let mut xyz = [0.0f64; 3];
            let mut norm = [0.0f64; 3];
            for row in table {
                let power = interp(row[0], &wavelengths, &samples);
                for c in 0..3 {
                    xyz[c] += row[1 + c] * power;
                    norm[c] += row[1 + c];
                }
            }
            let xyz = [xyz[0] / norm[0], xyz[1] / norm[1], xyz[2] / norm[2]];
            if xyz.iter().any(|v| *v <= 0.0) {
                return Err(bad("illuminant spectrum has no valid white point"));
            }
            xyz_to_xy(xyz)
        }
        _ => Err(bad("unsupported custom illuminant data kind")),
    }
}

/// Port of `light_xy`: chromaticity of a DNG light-source code. Code 255 reads `payload` (error when it is
/// missing), 24 is 3200 K, standard codes come from [`STANDARD_XY`], unknown codes give `Ok(None)`.
pub fn light_xy(code: i64, payload: Option<&[u8]>, little_endian: bool) -> Result<Option<[f64; 2]>> {
    if code == 255 {
        let payload = payload.ok_or_else(|| bad("custom illuminant data has the wrong size"))?;
        return custom_xy(payload, little_endian).map(Some);
    }
    if code == 24 {
        return temperature_xy(3200.0, 0.0).map(Some);
    }
    Ok(STANDARD_XY.iter().find(|(c, _)| *c == code).map(|(_, xy)| *xy))
}

/// Port of `triple_weights`: interpolation weights of three calibration illuminants (`whites`, as xy) for a
/// camera white `white` (XYZ). Distances are measured in (min(1500/T, 1), tint/200) space, as in the SDK.
pub fn triple_weights(whites: &[[f64; 2]; 3], white: [f64; 3]) -> Result<[f64; 3]> {
    let (t, tint) = temperature_tint(xyz_to_xy(white)?)?;
    let mut points = [[0.0f64; 2]; 3];
    for (point, xy) in points.iter_mut().zip(whites) {
        let (kelvin, light_tint) = temperature_tint(*xy)?;
        *point = [(1500.0 / kelvin).min(1.0), light_tint / 200.0];
    }
    let target = [(1500.0 / t).min(1.0), tint / 200.0];
    let mut weights = [0.0f64; 3];
    for (w, p) in weights.iter_mut().zip(&points) {
        let distance = (p[0] - target[0]).powi(2) + (p[1] - target[1]).powi(2);
        *w = 1.0 / (distance + 1e-8);
    }
    let total: f64 = weights.iter().sum();
    weights = weights.map(|w| w / total);
    weights = weights.map(|w| w * w * (3.0 - 2.0 * w));
    weights = weights.map(|w| ((w - 0.02) / 0.98).clamp(0.0, 1.0));
    let total: f64 = weights.iter().sum();
    weights = weights.map(|w| w / total);
    weights[2] = (1.0 - weights[0] - weights[1]).max(0.0);
    Ok(weights)
}
