use crate::database::{Calibration, Lens};

fn cubic(p: [f32; 4], t: f32) -> f32 {
    // Uniform Catmull-Rom. Missing neighbours are linearly extrapolated in the values.
    let [p0, p1, p2, p3] = p;
    p1 + 0.5 * t * (p2 - p0 + t * (2. * p0 - 5. * p1 + 4. * p2 - p3 + t * (3. * (p1 - p2) + p3 - p0)))
}

fn focal(entries: &[Calibration], value: f32, distortion: bool) -> Option<Calibration> {
    let model = &entries.first()?.model;
    let mut items: Vec<_> = entries.iter().filter(|c| &c.model == model).collect();
    items.sort_by(|a, b| a.focal.total_cmp(&b.focal));
    let first = *items.first()?;
    let last = *items.last()?;
    if value <= first.focal {
        return Some(first.clone());
    }
    if value >= last.focal {
        return Some(last.clone());
    }
    if let Some(c) = items.iter().find(|c| c.focal == value) {
        return Some((*c).clone());
    }
    let right = items.partition_point(|c| c.focal < value);
    if right == 0 || right == items.len() {
        return None;
    }
    let left = right - 1;
    let a = items[left];
    let b = items[right];
    if a.model != b.model {
        return Some(if value - a.focal <= b.focal - value { a } else { b }.clone());
    }
    let t = (value - a.focal) / (b.focal - a.focal);
    let mut out = a.clone();
    out.focal = value;
    for k in 0..out.terms.len() {
        let weighted = distortion || (a.model == "poly3" && k >= 2);
        let term = |c: &Calibration| c.terms.get(k).copied().unwrap_or(0.) * if weighted { c.focal } else { 1. };
        let p1 = term(a);
        let p2 = term(b);
        let p0 = left
            .checked_sub(1)
            .and_then(|i| items.get(i))
            .filter(|c| c.model == a.model)
            .map_or(2. * p1 - p2, |c| term(c));
        let p3 = items.get(right + 1).filter(|c| c.model == a.model).map_or(2. * p2 - p1, |c| term(c));
        out.terms[k] = cubic([p0, p1, p2, p3], t) / if weighted { value } else { 1. };
    }
    Some(out)
}

impl Lens {
    /// Port of `optics.profile_lens(...).interpolate_distortion(focal)`.
    pub fn interpolate_distortion(&self, value: f32) -> Option<Calibration> {
        if !value.is_finite() {
            return None;
        }
        focal(&self.distortion, value, true)
    }
    /// Port of `optics.profile_lens(...).interpolate_tca(focal)`.
    pub fn interpolate_tca(&self, value: f32) -> Option<Calibration> {
        if !value.is_finite() {
            return None;
        }
        focal(&self.tca, value, false)
    }
    /// Port of `optics.profile_lens(...).interpolate_vignetting(focal, aperture, distance)`.
    pub fn interpolate_vignetting(&self, focal: f32, aperture: f32, distance: f32) -> Option<Calibration> {
        if ![focal, aperture, distance].iter().all(|x| x.is_finite()) || aperture <= 0. || distance <= 0. {
            return None;
        }
        let mut result = self.vignetting.first()?.clone();
        result.focal = focal;
        result.aperture = aperture;
        result.distance = distance;
        result.terms.fill(0.);
        let range = if self.max_focal > self.min_focal { self.max_focal - self.min_focal } else { 1. };
        let mut total = 0f64;
        let mut closest = f32::INFINITY;
        let mut sums = vec![0f64; result.terms.len()];
        for c in &self.vignetting {
            let df = (focal - c.focal) / range;
            let da = 4. / aperture - 4. / c.aperture;
            let dd = 0.1 / distance - 0.1 / c.distance;
            let d = (df * df + da * da + dd * dd).sqrt();
            closest = closest.min(d);
            if d < 0.0001 {
                return Some(c.clone());
            }
            let weight = f64::from(d).powf(-3.5);
            total += weight;
            for (s, t) in sums.iter_mut().zip(&c.terms) {
                *s += weight * f64::from(*t);
            }
        }
        if closest > 1. || total == 0. {
            return None;
        }
        for (t, s) in result.terms.iter_mut().zip(sums) {
            *t = (s / total) as f32;
        }
        Some(result)
    }
}
