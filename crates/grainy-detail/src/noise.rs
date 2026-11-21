//! Noise reduction, ported from luma/processing.py: colour noise (`chroma_guided`, a colour-guided filter of a*
//! and b*) and luminance noise (`luma_wavelet` for `detail_version` 3, `luma_noise_reduction` for 1 and 2), with
//! their parameters and the wavelet's measured noise profiles. All float32 like the oracle, with the OpenCV
//! filters ported in `crate::cv`.

use grainy_core::{Image, Plane};
use grainy_tone::interp::interp;
use ndarray::{Array2, Axis, Zip};

use crate::cv::{self, Mat};
use crate::np;

/// Levels of the à trous wavelet (`WAVELET_LEVELS`).
pub const WAVELET_LEVELS: usize = 5;
/// Finest levels whose noise is measured; coarser ones halve per level (`WAVELET_ESTIMATED`).
pub const WAVELET_ESTIMATED: usize = 3;
/// Brightness bins of the noise profile, L* 0..100 (`WAVELET_BINS`).
pub const WAVELET_BINS: usize = 8;
/// B3-spline taps (`_B3`: `[1, 4, 6, 4, 1] / 16` in float32).
pub const B3: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];

/// Port of luma/processing.py `wavelet_kernel`: the B3-spline taps of an à trous level with `2^level - 1` zeros
/// between them (`4·2^level + 1` taps).
pub fn wavelet_kernel(level: u32) -> Vec<f32> {
    let step = 1usize << level;
    let mut k = vec![0f32; 4 * step + 1];
    for (i, &b) in B3.iter().enumerate() {
        k[i * step] = b;
    }
    k
}

/// Port of luma/processing.py `_atrous_smooth`: `cv2.sepFilter2D` of the plane with the level's kernel on both
/// axes (reflect-101).
pub fn atrous_smooth(x: &Plane, level: u32) -> Plane {
    cv::sep_filter(&Mat::from_plane(x), &wavelet_kernel(level)).into_plane()
}

/// Port of luma/processing.py `_box`: `cv2.boxFilter` of size `2r+1` (normalized, reflect-101) on every channel.
pub fn box_filter(a: &Image, r: usize) -> Image {
    let k = 2 * r + 1;
    grainy_imgproc::box_filter(a, (k, k), None, true, grainy_imgproc::Border::Reflect101)
        .unwrap_or_else(|_| Mat::from_image(a).into_image())
}

/// Port of luma/processing.py `chroma_guided_params`: (box radius, eps of the L* guide, eps of the chroma guide,
/// blend) for a colour noise reduction `amount` at `pixel_scale`.
pub fn chroma_guided_params(amount: f64, pixel_scale: f64) -> (usize, f32, f32, f32) {
    let s = 2f64.min(0f64.max(amount / 50.0));
    let eps = (0.0005 + 0.02 * s) as f32;
    let r = ((2.0 + 14.0 * s) * pixel_scale).round_ties_even().max(1.0) as usize;
    (r, eps, 8.0 * eps, 1f64.min(2.0 * s) as f32)
}

/// Port of luma/processing.py `chroma_guided`: colour noise reduction of a Lab image in place, a colour-guided
/// filter (He et al.) of a* and b* whose guide is L* and a 5×5 median of a* and b* (all divided by 100).
pub fn chroma_guided(lab: &mut Image, amount: f64, pixel_scale: f64) {
    let _timing = grainy_core::timing::Span::new("detail.chroma_guided");
    let (r, eps, chroma_eps, blend) = chroma_guided_params(amount, pixel_scale);
    let (h, w, _) = lab.dim();
    let hundred = 100f32;
    let ma = cv::median_blur5(&lab.index_axis(Axis(2), 1).to_owned());
    let mb = cv::median_blur5(&lab.index_axis(Axis(2), 2).to_owned());
    let mut guide = Image::zeros((h, w, 3));
    Zip::from(guide.rows_mut()).and(lab.rows()).and(&ma).and(&mb).par_for_each(|mut g, l, &a, &b| {
        g[0] = l[0] / hundred;
        g[1] = a / hundred;
        g[2] = b / hundred;
    });
    let gi = guide.index_axis(Axis(2), 0);
    let ga = guide.index_axis(Axis(2), 1);
    let gb = guide.index_axis(Axis(2), 2);
    let means = box_filter(&guide, r);
    let mut weighted = Image::zeros((h, w, 3));
    Zip::from(weighted.rows_mut()).and(guide.rows()).par_for_each(|mut o, g| {
        o[0] = g[0] * g[0];
        o[1] = g[1] * g[0];
        o[2] = g[2] * g[0];
    });
    let products = box_filter(&weighted, r);
    Zip::from(weighted.rows_mut()).and(guide.rows()).par_for_each(|mut o, g| {
        o[0] = g[1] * g[1];
        o[1] = g[1] * g[2];
        o[2] = g[2] * g[2];
    });
    let squares = box_filter(&weighted, r);
    drop(weighted);
    // The inverse of the guide's 3×3 covariance (+ eps), per pixel: i11 i12 i13 i22 i23 i33.
    let mut inverse = Array2::<f32>::zeros((h * w, 6));
    let mean_rows = means.view().into_shape_with_order((h * w, 3)).ok();
    let product_rows = products.view().into_shape_with_order((h * w, 3)).ok();
    let square_rows = squares.view().into_shape_with_order((h * w, 3)).ok();
    if let (Some(m), Some(p), Some(q)) = (mean_rows, product_rows, square_rows) {
        Zip::from(inverse.rows_mut()).and(m.rows()).and(p.rows()).and(q.rows()).par_for_each(|mut o, m, p, q| {
            let (m0, m1, m2) = (m[0], m[1], m[2]);
            let a11 = p[0] - m0 * m0 + eps;
            let a12 = p[1] - m0 * m1;
            let a13 = p[2] - m0 * m2;
            let a22 = q[0] - m1 * m1 + chroma_eps;
            let a23 = q[1] - m1 * m2;
            let a33 = q[2] - m2 * m2 + chroma_eps;
            let i11 = a22 * a33 - a23 * a23;
            let i12 = a13 * a23 - a12 * a33;
            let i13 = a12 * a23 - a13 * a22;
            let i22 = a11 * a33 - a13 * a13;
            let i23 = a13 * a12 - a11 * a23;
            let i33 = a11 * a22 - a12 * a12;
            let det = a11 * i11 + a12 * i12 + a13 * i13;
            o[0] = i11 / det;
            o[1] = i12 / det;
            o[2] = i13 / det;
            o[3] = i22 / det;
            o[4] = i23 / det;
            o[5] = i33 / det;
        });
    }
    for c in 1..3 {
        let p = lab.index_axis(Axis(2), c).to_owned();
        let mut stack = Image::zeros((h, w, 3));
        let mut gbp = Plane::zeros((h, w));
        Zip::from(stack.rows_mut()).and(&mut gbp).and(&p).and(&gi).and(&ga).and(&gb).par_for_each(
            |mut o, e, &p, &i, &ga, &gb| {
                o[0] = p;
                o[1] = i * p;
                o[2] = ga * p;
                *e = gb * p;
            },
        );
        let d = box_filter(&stack, r);
        let e = cv::box_filter(&Mat::from_plane(&gbp), 2 * r + 1).into_plane();
        // k (3 channels) and b of the guided filter, then their box means.
        let mut kb = Image::zeros((h, w, 3));
        let mut b = Plane::zeros((h, w));
        let inv = inverse.view().into_shape_with_order((h, w, 6)).ok();
        if let Some(inv) = inv {
            Zip::from(kb.rows_mut()).and(&mut b).and(d.rows()).and(&e).and(means.rows()).and(inv.rows()).par_for_each(
                |mut k, b, d, &e, m, v| {
                    let (m0, m1, m2) = (m[0], m[1], m[2]);
                    let c0 = d[1] - m0 * d[0];
                    let c1 = d[2] - m1 * d[0];
                    let c2 = e - m2 * d[0];
                    let (i11, i12, i13, i22, i23, i33) = (v[0], v[1], v[2], v[3], v[4], v[5]);
                    k[0] = i11 * c0 + i12 * c1 + i13 * c2;
                    k[1] = i12 * c0 + i22 * c1 + i23 * c2;
                    k[2] = i13 * c0 + i23 * c1 + i33 * c2;
                    *b = d[0] - (k[0] * m0 + k[1] * m1 + k[2] * m2);
                },
            );
        }
        let k = box_filter(&kb, r);
        let b = cv::box_filter(&Mat::from_plane(&b), 2 * r + 1).into_plane();
        let mut channel = lab.index_axis_mut(Axis(2), c);
        Zip::from(&mut channel).and(&p).and(k.rows()).and(&b).and(guide.rows()).par_for_each(|out, &p, k, &b, g| {
            let q = k[0] * g[0] + k[1] * g[1] + k[2] * g[2] + b;
            *out = p + (q - p) * blend;
        });
    }
}

/// Port of luma/processing.py `_noise_profile`: the noise scale of one wavelet level per brightness bin of the
/// guide (L*): mean |detail| clipped at 2.5× its global mean, per bin; bins with too few pixels use the overall
/// value. float64 sums like numpy's, float32 result.
pub fn noise_profile(detail: &Plane, guide: &Plane) -> [f32; WAVELET_BINS] {
    let a: Vec<f32> = detail.iter().map(|v| v.abs()).collect();
    let limit = (2.5 * np::mean_f64(&a)) as f32;
    let clipped: Vec<f32> = a.iter().map(|&v| np::minimum(v, limit)).collect();
    let scale = (WAVELET_BINS as f64 / 100.0) as f32;
    let mut sums = [0f64; WAVELET_BINS];
    let mut counts = [0usize; WAVELET_BINS];
    for (&g, &c) in guide.iter().zip(&clipped) {
        let index = np::f32_to_i32(g * scale).clamp(0, WAVELET_BINS as i32 - 1) as usize;
        sums[index] += f64::from(c);
        counts[index] += 1;
    }
    let overall = np::mean_f64(&clipped);
    let minimum = 256usize.max(clipped.len() / 2000);
    let mut out = [0f32; WAVELET_BINS];
    for (slot, (&s, &n)) in out.iter_mut().zip(sums.iter().zip(&counts)) {
        *slot = if n > minimum { (s / n.max(1) as f64) as f32 } else { overall as f32 };
    }
    out
}

/// Port of luma/processing.py `wavelet_lambda`: the shrinkage strength `(amount/100·3)²·pixel_scale`.
pub fn wavelet_lambda(amount: f64, pixel_scale: f64) -> f32 {
    ((amount / 100.0 * 3.0).powi(2) * pixel_scale) as f32
}

/// Port of luma/processing.py `wavelet_profiles`: the measured noise profiles of the estimated levels of L*.
pub fn wavelet_profiles(lightness: &Plane) -> Vec<[f32; WAVELET_BINS]> {
    let mut x = lightness.to_owned();
    let mut guide: Option<Plane> = None;
    let mut profiles = Vec::with_capacity(WAVELET_ESTIMATED);
    for level in 0..WAVELET_ESTIMATED {
        let smooth = atrous_smooth(&x, level as u32);
        let g = guide.get_or_insert_with(|| smooth.clone());
        profiles.push(noise_profile(&(&x - &smooth), g));
        x = smooth;
    }
    profiles
}

/// Port of luma/processing.py `luma_wavelet` (detail_version 3): à trous B3-spline wavelet of L* whose levels
/// are scaled by E/(E + λσ²), E the 5×5 mean energy of the level and σ its noise per brightness (measured on this
/// frame unless `profiles` are given).
pub fn luma_wavelet(
    lightness: &Plane,
    amount: f64,
    pixel_scale: f64,
    profiles: Option<&[[f32; WAVELET_BINS]]>,
) -> Plane {
    let _timing = grainy_core::timing::Span::new("detail.luma_wavelet");
    let lam = wavelet_lambda(amount, pixel_scale);
    let mut x = lightness.to_owned();
    let mut out = Plane::zeros(x.raw_dim());
    let mut guide: Option<Plane> = None;
    let mut profile = [0f32; WAVELET_BINS];
    let centres: Vec<f64> =
        (0..WAVELET_BINS).map(|i| f64::from((i as f32 + 0.5) * (100.0 / WAVELET_BINS as f64) as f32)).collect();
    for level in 0..WAVELET_LEVELS {
        let smooth = atrous_smooth(&x, level as u32);
        let d = &x - &smooth;
        let g = guide.get_or_insert_with(|| smooth.clone());
        if level >= WAVELET_ESTIMATED {
            profile.iter_mut().for_each(|v| *v *= 0.5);
        } else {
            profile = match profiles.and_then(|p| p.get(level)) {
                Some(p) => *p,
                None => noise_profile(&d, g),
            };
        }
        let fp: Vec<f64> = profile.iter().map(|&v| f64::from(v)).collect();
        let squares = d.mapv(|v| v * v);
        let energy = cv::box_filter(&Mat::from_plane(&squares), 5).into_plane();
        Zip::from(&mut out).and(&d).and(&energy).and(&*g).par_for_each(|o, &d, &e, &g| {
            let sigma = interp(f64::from(g), &centres, &fp) as f32 * 1.25;
            *o += d * (e / (e + lam * sigma * sigma + 1e-12));
        });
        x = smooth;
    }
    out + &x
}

/// Port of luma/processing.py `luma_noise_reduction` (detail_version 1 and 2): a bilateral filter of L*
/// (diameter `2·round(3·pixel_scale)+1`), blended by the slider strength.
pub fn luma_noise_reduction(lightness: &Plane, amount: f64, pixel_scale: f64) -> Plane {
    let strength = amount / 100.0;
    let diameter = 2 * (3.0 * pixel_scale).round_ties_even() as i32 + 1;
    let filtered =
        cv::bilateral_filter(lightness, diameter, 2.0 + strength * 16.0, (1.0 + strength * 3.0) * pixel_scale);
    let keep = (1.0 - strength) as f32;
    let take = strength as f32;
    let mut out = lightness.to_owned();
    Zip::from(&mut out).and(&filtered).par_for_each(|o, &f| *o = *o * keep + f * take);
    out
}
