//! Port of `film_look.Stock` and `film_look.stock`: a film stock built from its datasheet curves into
//! per-layer tables over log exposure and a density-to-colour cube, built once and cached.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use rayon::prelude::*;
use serde_json::Value;

use crate::numeric::{interp, linspace, linspace_f32, lstsq, polyfit_slope, Matrix};
use crate::spectral::{colorimetry, d65, light, points, sampled, Dyes, GREY, PRINT_GREY};
use crate::{embedded, Error, Result};

/// Log10 exposure span of the per-layer tables around the reference exposure (`film_look.SPAN`).
pub const SPAN: (f64, f64) = (-6.0, 4.0);
/// Samples of the per-layer tables (`film_look.SAMPLES`).
pub const SAMPLES: usize = 2048;
/// Nodes per axis of the density cube (`film_look.NODES`).
pub const NODES: usize = 33;
/// The smallest exposure and colour value (`film_look.TINY`).
pub const TINY: f64 = 1e-7;
/// ln 10 (`film_look.LN10`).
pub const LN10: f64 = std::f64::consts::LN_10;

/// A built film stock (port of `film_look.Stock`). Every table the oracle keeps is public, to locate a
/// mismatch.
#[derive(Debug)]
pub struct Stock {
    /// The stock's data file.
    pub data: Value,
    /// 'colour-negative', 'colour-slide' or 'bw-negative'.
    pub kind: String,
    /// 1 for black-and-white, else 3.
    pub channels: usize,
    /// The reference log exposure (moved by `place_grey` for slides).
    pub reference: f64,
    /// Linear sRGB to each layer's exposure (channels × 3), from the layer sensitivities.
    pub matrix: Vec<[f64; 3]>,
    /// The log exposure grid, float32.
    pub grid: Vec<f32>,
    /// The grid step (float64).
    pub dx: f64,
    /// Output density per layer over the grid (channels × SAMPLES), float32.
    pub tables: Vec<Vec<f32>>,
    /// Forward differences of `tables`, the last 0.
    pub slopes: Vec<Vec<f32>>,
    /// Printer lights of a negative (channels).
    pub lights: Option<Vec<f64>>,
    /// The paper's white density (black-and-white).
    pub paper_white: Option<f64>,
    /// The dyes the colour is read through: the slide's own or the paper's.
    pub dyes: Option<Dyes>,
    /// The cube's lowest densities.
    pub lo: [f32; 3],
    /// The cube's highest densities.
    pub hi: [f32; 3],
    /// The cube's node spacing.
    pub step: [f32; 3],
    /// 1/step.
    pub inverse_step: [f32; 3],
    /// -log10 XYZ at the NODES³ density nodes (index `(i*NODES + j)*NODES + k`), float32.
    pub cube: Vec<[f32; 3]>,
}

fn f(value: &Value) -> f64 {
    value.as_f64().unwrap_or(f64::NAN)
}

impl Stock {
    /// Port of `Stock.__init__(data, paper)`.
    pub fn new(data: Value, paper: Option<&Value>) -> Result<Self> {
        let id = data["id"].as_str().unwrap_or("").to_owned();
        let kind = data["kind"].as_str().unwrap_or("").to_owned();
        let channels = if kind == "bw-negative" { 1 } else { 3 };
        let lines: &[&str] = if channels == 1 { &["v"] } else { &["r", "g", "b"] };
        let reference = f(&data["log_h_ref"]);
        let matrix = exposure_matrix(&data, lines)?;
        let x = linspace(reference + SPAN.0, reference + SPAN.1, SAMPLES);
        let grid: Vec<f32> = x.iter().map(|&v| v as f32).collect();
        let dx = x[1] - x[0];
        let mut density: Vec<Vec<f64>> = lines
            .iter()
            .map(|k| {
                let (cx, cy) = points(&data["curves"][*k]);
                x.iter().map(|&v| interp(v, &cx, &cy)).collect()
            })
            .collect();
        if kind == "colour-negative" {
            density = printing_density(&density, &x, reference);
        }
        let mut stock = Stock {
            data: Value::Null,
            kind: kind.clone(),
            channels,
            reference,
            matrix,
            grid,
            dx,
            tables: Vec::new(),
            slopes: Vec::new(),
            lights: None,
            paper_white: None,
            dyes: None,
            lo: [0.0; 3],
            hi: [0.0; 3],
            step: [0.0; 3],
            inverse_step: [0.0; 3],
            cube: Vec::new(),
        };
        if kind == "colour-slide" {
            let minimum = [0, 1, 2].map(|j| density[j].iter().copied().fold(f64::INFINITY, f64::min));
            let view = data.get("view").and_then(Value::as_str).unwrap_or("D50");
            stock.dyes = Some(Dyes::new(&data["dyes"], minimum, view)?);
            stock.tables = density.iter().map(|d| d.iter().map(|&v| v as f32).collect()).collect();
        } else if kind == "colour-negative" || kind == "bw-negative" {
            let paper = paper.ok_or_else(|| Error::BadData(format!("{id}: a negative needs a paper")))?;
            stock.tables = stock.printed(&density, paper)?;
        } else {
            return Err(Error::BadData(format!("{id}: cannot simulate a {kind}")));
        }
        stock.slopes = stock
            .tables
            .iter()
            .map(|t| (0..t.len()).map(|i| if i + 1 < t.len() { t[i + 1] - t[i] } else { 0.0 }).collect())
            .collect();
        if channels == 3 {
            stock.build_cube();
        }
        if kind == "colour-slide" {
            stock.place_grey();
        }
        stock.data = data;
        Ok(stock)
    }

    /// Port of `Stock.printed`: per-layer print densities over the grid, with printer lights that print a mid
    /// grey neutral at PRINT_GREY.
    fn printed(&mut self, density: &[Vec<f64>], paper: &Value) -> Result<Vec<Vec<f32>>> {
        let x: Vec<f64> = self.grid.iter().map(|&v| f64::from(v)).collect();
        let grey: Vec<f64> = density.iter().map(|d| interp(self.reference, &x, d)).collect();
        let keys: &[&str] = if self.channels == 1 { &["v"] } else { &["r", "g", "b"] };
        let mut curves: Vec<(Vec<f64>, Vec<f64>)> = keys.iter().map(|k| points(&paper["curves"][*k])).collect();
        let top = curves
            .iter()
            .map(|(_, y)| y.iter().copied().fold(f64::NEG_INFINITY, f64::max))
            .fold(f64::INFINITY, f64::min);
        for (_, y) in curves.iter_mut() {
            for v in y.iter_mut() {
                *v = v.min(top);
            }
        }
        let lowest = |y: &[f64]| y.iter().copied().fold(f64::INFINITY, f64::min);
        let print = |lights: &[f64], j: usize, d: f64| interp(lights[j] - d, &curves[j].0, &curves[j].1);
        let target_density = -PRINT_GREY.log10();
        let mut lights: Vec<f64> =
            curves.iter().zip(&grey).map(|((cx, cy), g)| interp(target_density + lowest(cy), cy, cx) + g).collect();
        let n = lights.len();
        let dyes = if self.channels == 1 {
            self.paper_white = Some(lowest(&curves[0].1));
            None
        } else {
            let minimum = [0, 1, 2].map(|j| lowest(&curves[j].1));
            let view = paper.get("view").and_then(Value::as_str).unwrap_or("D50");
            Some(Dyes::new(&paper["dyes"], minimum, view)?)
        };
        let white = lowest(&curves[0].1);
        let d65 = d65();
        let error = |l: &[f64]| -> Vec<f64> {
            let p: Vec<f64> = (0..n).map(|j| print(l, j, grey[j])).collect();
            match &dyes {
                None => vec![10f64.powf(-(p[0] - white)) - PRINT_GREY],
                Some(dyes) => {
                    let c = dyes.colour([p[0], p[1], p[2]]);
                    (0..3).map(|k| c[k] - PRINT_GREY * d65[k] / d65[1]).collect()
                }
            }
        };
        for _ in 0..40 {
            let e = error(&lights);
            if e.iter().fold(0.0f64, |m, v| m.max(v.abs())) < 1e-6 {
                break;
            }
            let mut jacobian: Matrix = vec![vec![0.0; n]; n];
            for h in 0..n {
                let mut l = lights.clone();
                l[h] += 1e-4;
                let eh = error(&l);
                for k in 0..n {
                    jacobian[k][h] = (eh[k] - e[k]) / 1e-4;
                }
            }
            let step = lstsq(&jacobian, &e);
            for (l, s) in lights.iter_mut().zip(step) {
                *l -= s;
            }
        }
        let tables = (0..n).map(|j| density[j].iter().map(|&d| print(&lights, j, d) as f32).collect()).collect();
        self.lights = Some(lights);
        self.dyes = dyes;
        Ok(tables)
    }

    /// Port of `Stock.place_grey`: a slide's reference exposure moves to where its neutral scale passes
    /// PRINT_GREY.
    fn place_grey(&mut self) {
        let Some(dyes) = &self.dyes else { return };
        let y: Vec<f64> = (0..SAMPLES)
            .into_par_iter()
            .map(|i| dyes.colour([0, 1, 2].map(|j| f64::from(self.tables[j][i])))[1])
            .collect();
        let mut order: Vec<usize> = (0..SAMPLES).collect();
        order.sort_by(|&a, &b| y[a].total_cmp(&y[b]));
        let xp: Vec<f64> = order.iter().map(|&i| y[i]).collect();
        let fp: Vec<f64> = order.iter().map(|&i| f64::from(self.grid[i])).collect();
        self.reference = interp(PRINT_GREY, &xp, &fp);
    }

    /// Port of `Stock.build_cube`: -log10 XYZ on a NODES³ grid of densities.
    fn build_cube(&mut self) {
        let Some(dyes) = &self.dyes else { return };
        for j in 0..3 {
            let t = &self.tables[j];
            self.lo[j] = t.iter().copied().fold(f32::INFINITY, f32::min);
            self.hi[j] = t.iter().copied().fold(f32::NEG_INFINITY, f32::max).max(self.lo[j] + 1e-3);
        }
        let axes: Vec<Vec<f32>> = (0..3).map(|j| linspace_f32(self.lo[j], self.hi[j], NODES)).collect();
        self.cube = (0..NODES * NODES * NODES)
            .into_par_iter()
            .map(|index| {
                let (a, b, c) = (index / (NODES * NODES), index / NODES % NODES, index % NODES);
                let xyz = dyes.colour([f64::from(axes[0][a]), f64::from(axes[1][b]), f64::from(axes[2][c])]);
                xyz.map(|v| (-(if v.is_nan() { v } else { v.max(TINY) }).log10()) as f32)
            })
            .collect();
        for j in 0..3 {
            self.step[j] = (self.hi[j] - self.lo[j]) / (NODES - 1) as f32;
            self.inverse_step[j] = 1.0 / self.step[j];
        }
    }

    /// Port of `Stock.densities`: per-layer output densities of one linear sRGB scene colour (float32).
    pub fn densities(&self, rgb: [f32; 3]) -> [f32; 3] {
        let offset = (self.reference - GREY.log10() - f64::from(self.grid[0])) as f32;
        let scale = (1.0 / self.dx) as f32;
        let top = (SAMPLES as f64 - 1.001) as f32;
        let mut out = [0.0f32; 3];
        for j in 0..self.channels {
            let m = self.matrix[j].map(|v| v as f32);
            let e = rgb[2].mul_add(m[2], rgb[1].mul_add(m[1], rgb[0] * m[0]));
            let x = (if e.is_nan() { e } else { e.max(TINY as f32) }).log10();
            let u = ((x + offset) * scale).max(0.0).min(top);
            let i = u as usize;
            let t = u - i as f32;
            out[j] = self.tables[j][i] + self.slopes[j][i] * t;
        }
        out
    }

    /// Port of `Stock.xyz`: XYZ (D65, white = 1) of one linear sRGB scene colour, as float32 (the oracle
    /// computes colour stocks in float64 and the caller casts).
    pub fn xyz(&self, rgb: [f32; 3]) -> [f32; 3] {
        let d = self.densities(rgb);
        let ln10 = -(LN10 as f32);
        if self.channels == 1 {
            let white = self.paper_white.unwrap_or(0.0) as f32;
            let y = crate::npmath::exp(ln10 * (d[0] - white));
            let d65 = d65();
            return [0, 1, 2].map(|k| y * (d65[k] / d65[1]) as f32);
        }
        let top = (NODES as f64 - 1.001) as f32;
        let mut i = [0usize; 3];
        let mut t = [0f64; 3];
        for k in 0..3 {
            let v = ((d[k] - self.lo[k]) * self.inverse_step[k]).max(0.0).min(top);
            i[k] = v as usize;
            t[k] = f64::from(v - i[k] as f32);
        }
        let base = (i[0] * NODES + i[1]) * NODES + i[2];
        let (n2, n1) = (NODES * NODES, NODES);
        let (t0, t1, t2) = (t[0], t[1], t[2]);
        let (s0, s1, s2) = (1.0 - t0, 1.0 - t1, 1.0 - t2);
        let c = |index: usize, k: usize| f64::from(self.cube[index][k]);
        let ln10 = f64::from(ln10);
        [0, 1, 2].map(|k| {
            let mut v = ((c(base, k) * s0 + c(base + n2, k) * t0) * s2
                + (c(base + 1, k) * s0 + c(base + n2 + 1, k) * t0) * t2)
                * s1;
            v += ((c(base + n1, k) * s0 + c(base + n2 + n1, k) * t0) * s2
                + (c(base + n1 + 1, k) * s0 + c(base + n2 + n1 + 1, k) * t0) * t2)
                * t1;
            (ln10 * v).exp() as f32
        })
    }
}

/// Port of `Stock.exposure_matrix`: linear sRGB to each layer's relative exposure under the light the film
/// is balanced for.
fn exposure_matrix(data: &Value, lines: &[&str]) -> Result<Vec<[f64; 3]>> {
    let c = colorimetry();
    let sensitivity = data.get("sensitivity").filter(|v| v.as_object().is_some_and(|m| !m.is_empty()));
    let columns: Vec<Vec<f64>> = match sensitivity {
        Some(s) if lines.iter().all(|k| s.get(*k).is_some()) => lines
            .iter()
            .map(|k| sampled(&s[*k], Some(f64::NEG_INFINITY)).into_iter().map(|v| 10f64.powf(v)).collect())
            .collect(),
        _ => lines.iter().map(|_| c.cmf.iter().map(|r| r[1]).collect()).collect(),
    };
    let balance = data.get("balance").and_then(Value::as_str).unwrap_or("daylight");
    let spectrum = light(balance)?;
    Ok(columns
        .iter()
        .map(|s| {
            let mut m = [0.0; 3];
            for ((sv, lv), basis) in s.iter().zip(&spectrum).zip(&c.basis) {
                let weight = sv * lv;
                for k in 0..3 {
                    m[k] += weight * basis[k];
                }
            }
            let sum = m[0] + m[1] + m[2];
            m.map(|v| v / sum)
        })
        .collect())
}

/// Port of `Stock.printing_density`: each layer's contrast matched to the green layer's around the
/// reference exposure.
fn printing_density(density: &[Vec<f64>], x: &[f64], reference: f64) -> Vec<Vec<f64>> {
    let near: Vec<usize> = (0..x.len()).filter(|&i| x[i] >= reference - 1.0 && x[i] <= reference + 1.0).collect();
    let xs: Vec<f64> = near.iter().map(|&i| x[i]).collect();
    let slopes: Vec<f64> =
        density.iter().map(|d| polyfit_slope(&xs, &near.iter().map(|&i| d[i]).collect::<Vec<_>>())).collect();
    density
        .iter()
        .zip(&slopes)
        .map(|(d, s)| {
            let at = interp(reference, x, d);
            let scale = slopes[1] / s.max(1e-6);
            d.iter().map(|v| at + (v - at) * scale).collect()
        })
        .collect()
}

fn find(table: &[(&str, &'static str)], name: &str) -> Option<&'static str> {
    table.iter().find(|(id, _)| *id == name).map(|(_, text)| *text)
}

/// Port of `film_look.stock`: the built stock `name`, built on first use and kept.
pub fn stock(name: &str) -> Result<Arc<Stock>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Stock>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut built = cache.lock().map_err(|_| Error::BadData("stock cache poisoned".into()))?;
    if let Some(s) = built.get(name) {
        return Ok(s.clone());
    }
    let text = find(embedded::STOCKS, name).ok_or_else(|| Error::UnknownStock(name.to_owned()))?;
    let data: Value = serde_json::from_str(text).map_err(|e| Error::BadData(format!("{name}: {e}")))?;
    let paper = match data.get("print").and_then(Value::as_str) {
        Some(p) if !p.is_empty() => {
            let text = find(embedded::PAPERS, p).ok_or_else(|| Error::BadData(format!("{name}: no paper {p}")))?;
            Some(serde_json::from_str::<Value>(text).map_err(|e| Error::BadData(format!("{p}: {e}")))?)
        }
        _ => None,
    };
    let s = Arc::new(Stock::new(data, paper.as_ref())?);
    built.insert(name.to_owned(), s.clone());
    Ok(s)
}
