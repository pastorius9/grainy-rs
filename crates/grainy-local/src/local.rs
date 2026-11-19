//! Local adjustments: `luma/processing.py` `apply_local` and `_local_region` (CPU path).

use std::ops::ControlFlow;

use grainy_colorgrade::convert::{hsl_to_rgb_pixel, rgb_to_hsl_pixel, simd_column};
use grainy_core::{Image, Plane, Settings};
use ndarray::{s, ArrayView2, Axis, Zip};
use serde_json::{Map, Value};

use crate::mask::{layer_mask, occupied_bounds};
use crate::np::{clip, dot3, maximum, LUMA};
use crate::value::{bad, enabled, flag, num, numbers, object, Obj};
use crate::{Error, Result, SkyMask, Transform};

/// What a local adjustment needs besides its layers: the frame geometry and the develop state.
pub struct LocalContext<'a> {
    /// The shape (h, w) of the source photo, where shape components are drawn.
    pub source_shape: (usize, usize),
    /// The mask geometry (source pixels to the developed frame).
    pub transform: Transform<'a>,
    /// The developed colour source (source pixels), for auto-masked brushes and sky detection; `None` uses the
    /// frame itself.
    pub source_image: Option<&'a Image>,
    /// The preview's size relative to the original (1 for full size).
    pub pixel_scale: f64,
    /// The photo's `detail_version`: local sharpening and luminance noise reduction use the same methods.
    pub detail_version: i64,
    /// Sky detection for `sky` components.
    pub sky: Option<&'a dyn SkyMask>,
}

/// The input of a point colour inside a layer, as the oracle's `on_point(layer, point, image, mask, origin,
/// shape)` callback receives it.
pub struct PointInput<'a> {
    /// The layer index.
    pub layer: usize,
    /// The point index (`points.len()` for the final call after the last point).
    pub point: usize,
    /// The image the point works on (the layer's padded region).
    pub image: &'a Image,
    /// The layer mask over the same region.
    pub mask: ArrayView2<'a, f32>,
    /// The region's top-left corner `(x0, y0)` in the frame.
    pub origin: (usize, usize),
    /// The frame's shape (h, w).
    pub frame: (usize, usize),
}

/// The point-colour callback; `Break` ends the develop (the oracle raises from it).
pub type PointCallback<'a> = &'a mut dyn FnMut(&PointInput<'_>) -> ControlFlow<()>;
/// The layer-mask callback (`on_mask(index, mask)`).
pub type MaskCallback<'a> = &'a mut dyn FnMut(usize, &Plane);
/// The layer-input callback (`on_layer_input(index, image)`); `Break` ends the develop.
pub type LayerInputCallback<'a> = &'a mut dyn FnMut(usize, &Image) -> ControlFlow<()>;

/// The callbacks of [`apply_local`] (the oracle's `on_mask`, `on_layer_input`, `on_point`). A callback that returns
/// `ControlFlow::Break` stops the work: [`apply_local`] returns [`Error::Interrupted`].
#[derive(Default)]
pub struct LocalHooks<'a> {
    /// Receives every layer's mask (`index`, mask), disabled layers included; with it set, masks of disabled layers
    /// are computed too.
    pub on_mask: Option<MaskCallback<'a>>,
    /// Receives the image each layer starts from (`index`, image), before its mask.
    pub on_layer_input: Option<LayerInputCallback<'a>>,
    /// Receives each point colour's input inside a layer; with it set, every enabled layer calls the point colour
    /// tool (the oracle's colour picker).
    pub on_point: Option<PointCallback<'a>>,
}

/// Port of luma/processing.py `apply_local` (without its cache): the mask layers in order, each one's adjustments
/// applied where its mask covers the frame. A later layer's range mask reads the result of the earlier layers.
///
/// Cache: the oracle memoises each layer's mask (`mask:{index}`, keyed by the geometry for shape-only layers and
/// by the chained image state otherwise) and each layer's result (`local:{index}`, keyed by the mask key and the
/// adjustments); a cache would wrap the [`layer_mask`] and [`local_region`] calls below.
pub fn apply_local(
    image: &Image,
    layers: &[Value],
    ctx: &LocalContext<'_>,
    hooks: &mut LocalHooks<'_>,
) -> Result<Image> {
    let mut image = image.clone();
    for (index, layer) in layers.iter().enumerate() {
        let layer = object(layer, "masks")?;
        if let Some(hook) = hooks.on_layer_input.as_mut() {
            if hook(index, &image).is_break() {
                return Err(Error::Interrupted);
            }
        }
        let on = enabled(layer);
        if !on && hooks.on_mask.is_none() {
            continue;
        }
        let mask = layer_mask(&image, ctx.source_shape, layer, ctx.transform, ctx.source_image, ctx.sky)?;
        if let Some(hook) = hooks.on_mask.as_mut() {
            hook(index, &mask);
        }
        if !on {
            continue;
        }
        let mut edit = match layer.get("adjustments") {
            None => Map::new(),
            Some(v) => object(v, "adjustments")?.clone(),
        };
        if ctx.detail_version > 1 && (flag(&edit, "sharpen") || flag(&edit, "noise_luma")) {
            edit.insert("detail_version".into(), Value::from(ctx.detail_version));
        }
        let on_point = hooks.on_point.as_mut().map(|f| &mut **f as PointCallback<'_>);
        image = local_region(&image, &mask, &edit, index, ctx.pixel_scale, on_point)?;
    }
    Ok(image)
}

/// A curve of `local_curves`: the channel it changes (0 = all three) and its points as float64.
struct Curve {
    channel: usize,
    xs: Vec<f64>,
    ys: Vec<f64>,
}

fn curves(edit: &Obj) -> Result<Vec<Curve>> {
    let Some(value) = edit.get("local_curves") else { return Ok(Vec::new()) };
    let list = value.as_array().ok_or_else(|| bad("local_curves", value))?;
    let mut out = Vec::new();
    for (channel, points) in list.iter().enumerate() {
        let rows = points.as_array().ok_or_else(|| bad("local_curves", points))?;
        if rows.is_empty() {
            return Err(bad("local_curves", points));
        }
        let mut xs = Vec::with_capacity(rows.len());
        let mut ys = Vec::with_capacity(rows.len());
        for row in rows {
            let p = numbers(row, "local_curves", 2)?;
            xs.push(p[0] as f32);
            ys.push(p[1] as f32);
        }
        if xs == ys {
            continue;
        }
        if channel > 3 {
            return Err(bad("local_curves", value));
        }
        out.push(Curve {
            channel,
            xs: xs.into_iter().map(f64::from).collect(),
            ys: ys.into_iter().map(f64::from).collect(),
        });
    }
    Ok(out)
}

fn hsl_groups(edit: &Obj) -> Result<Option<Vec<Vec<f64>>>> {
    let Some(value) = edit.get("hsl") else { return Ok(None) };
    let list = value.as_array().ok_or_else(|| bad("hsl", value))?;
    let groups: Vec<Vec<f64>> = list.iter().map(|g| numbers(g, "hsl", 3)).collect::<Result<_>>()?;
    if !groups.iter().any(|g| g.iter().any(|&v| v != 0.0)) {
        return Ok(None);
    }
    Ok(Some(groups.into_iter().take(8).collect()))
}

/// Python's `math.ceil` of a float, as an integer.
fn ceil(x: f64) -> i64 {
    x.ceil() as i64
}

/// Port of luma/processing.py `_local_region` (CPU path, without its cache): the layer's adjustments computed on
/// the padded rectangle where `mask` is positive (the padding covers the reach of the detail filters), blended into
/// the frame by the mask.
///
/// Cache: the oracle memoises the region's `to_linear` (`linear:{index}`) and the whole-frame noise profile of the
/// wavelet noise reduction (`noise:{index}`); a cache would wrap those two computations.
pub fn local_region(
    image: &Image,
    mask: &Plane,
    edit: &Obj,
    index: usize,
    pixel_scale: f64,
    on_point: Option<PointCallback<'_>>,
) -> Result<Image> {
    let (frame_h, frame_w, _) = image.dim();
    if mask.dim() != (frame_h, frame_w) {
        return Err(Error::Shape(format!(
            "a mask of {:?} does not fit the frame {:?}",
            mask.dim(),
            (frame_h, frame_w)
        )));
    }
    let Some((r0, r1, c0, c1)) = occupied_bounds(mask) else { return Ok(image.clone()) };
    let detail = ["texture", "dehaze", "noise_luma", "noise_color", "sharpen"].iter().any(|k| flag(edit, k));
    let mut pad = if detail {
        ceil(32.0 * pixel_scale).max(4)
    } else if flag(edit, "clarity") {
        ceil(10.0 * pixel_scale)
    } else {
        0
    };
    if flag(edit, "noise_color") {
        // chroma_guided reaches two box radii plus the median's 2 pixels.
        let (radius, ..) = grainy_detail::chroma_guided_params(num(edit, "noise_color", 0.0)?, pixel_scale);
        pad = pad.max(2 * radius as i64 + 3);
    }
    let version = num(edit, "detail_version", 1.0)?;
    let wavelet = flag(edit, "noise_luma") && version >= 3.0;
    if wavelet {
        pad = pad.max(72); // luma_wavelet: five levels reach 62 pixels, the energy box and guide 2 more
    }
    let y0 = (r0 as i64 - pad).max(0) as usize;
    let y1 = (r1 as i64 + pad).min(frame_h as i64).max(y0 as i64) as usize;
    let x0 = (c0 as i64 - pad).max(0) as usize;
    let x1 = (c1 as i64 + pad).min(frame_w as i64).max(x0 as i64) as usize;
    let region = image.slice(s![y0..y1, x0..x1, ..]).to_owned();
    let linear = grainy_color::to_linear(&region);

    let hdr = flag(edit, "hdr");
    let gain = 2f64.powf(num(edit, "exposure", 0.0)?) as f32;
    let contrast = (1.0 + num(edit, "contrast", 0.0)? / 125.0) as f32;
    let temperature = num(edit, "temperature", 0.0)?;
    let tint = num(edit, "tint", 0.0)?;
    let shift = [temperature as f32 / 400.0, (-tint * 0.5) as f32 / 400.0, (-temperature) as f32 / 400.0];
    let saturation = (1.0 + num(edit, "saturation", 0.0)? / 100.0) as f32;
    let shadows = (num(edit, "shadows", 0.0)? / 300.0) as f32;
    let highlights = (num(edit, "highlights", 0.0)? / 300.0) as f32;
    let levels = if flag(edit, "whites") || flag(edit, "blacks") {
        let black = num(edit, "blacks", 0.0)? / 200.0;
        let white = 1.0 + num(edit, "whites", 0.0)? / 200.0;
        Some(((white - black) as f32, black as f32))
    } else {
        None
    };
    let curves = curves(edit)?;
    let hue = if flag(edit, "hue") { Some((num(edit, "hue", 0.0)? / 360.0) as f32) } else { None };
    let width = x1 - x0;

    // Per-pixel tone and colour, in the oracle's order; `gains` keeps the HDR highlight gain of each pixel.
    let mut a = linear;
    let mut gains = Plane::ones((y1 - y0, width));
    Zip::indexed(a.lanes_mut(Axis(2))).and(&mut gains).par_for_each(|(_, x), mut p, g| {
        let mut v = [0f32; 3];
        for c in 0..3 {
            v[c] = grainy_color::srgb_encode(p[c] * gain);
            v[c] = (v[c] - 0.5) * contrast + 0.5;
            v[c] += shift[c];
        }
        let gray = dot3(&v, &LUMA);
        for value in &mut v {
            *value = gray + (*value - gray) * saturation;
        }
        let dark = 1.0 - clip(gray, 0.0, 1.0);
        let dark = dark * dark;
        for value in &mut v {
            *value += shadows * dark;
        }
        let bright = clip(gray, 0.0, 1.0);
        let bright = bright * bright;
        for value in &mut v {
            *value += highlights * bright;
        }
        if let Some((scale, black)) = levels {
            for value in &mut v {
                *value = *value * scale + black;
            }
        }
        // Local colour tools work on bounded chroma; highlight intensity survives them.
        if hdr {
            *g = maximum(1.0, maximum(maximum(v[0], v[1]), v[2]));
            for value in &mut v {
                *value /= *g;
            }
        }
        for curve in &curves {
            let interp = |x: f32| grainy_tone::interp::interp(f64::from(x), &curve.xs, &curve.ys) as f32;
            if curve.channel == 0 {
                v = v.map(interp);
            } else {
                v[curve.channel - 1] = interp(v[curve.channel - 1]);
            }
        }
        if let Some(turn) = hue {
            let simd = simd_column(x, width);
            let (h, s, l) = rgb_to_hsl_pixel(&v, simd);
            v = hsl_to_rgb_pixel(h + turn, s, l, simd);
        }
        p[0] = v[0];
        p[1] = v[1];
        p[2] = v[2];
    });
    if let Some(groups) = hsl_groups(edit)? {
        a = grainy_colorgrade::mix_hsl(&a, &groups);
    }
    let point_colors = match edit.get("point_colors") {
        Some(v) if grainy_core::settings::truthy(v) => {
            let list = v.as_array().ok_or_else(|| bad("point_colors", v))?;
            list.iter().map(|p| object(p, "point_colors").cloned()).collect::<Result<Vec<_>>>()?
        }
        _ => Vec::new(),
    };
    if !point_colors.is_empty() || on_point.is_some() {
        let region_mask = mask.slice(s![y0..y1, x0..x1]);
        let mut stopped = false;
        a = match on_point {
            Some(callback) => {
                let mut hook = |point: usize, value: &Image| {
                    if !stopped {
                        let input = PointInput {
                            layer: index,
                            point,
                            image: value,
                            mask: region_mask,
                            origin: (x0, y0),
                            frame: (frame_h, frame_w),
                        };
                        stopped = callback(&input).is_break();
                    }
                };
                grainy_colorgrade::point_color::apply(a, &point_colors, Some(&mut hook))?
            }
            None => grainy_colorgrade::point_color::apply(a, &point_colors, None)?,
        };
        if stopped {
            return Err(Error::Interrupted);
        }
    }
    if hdr {
        Zip::from(a.lanes_mut(Axis(2))).and(&gains).par_for_each(|mut p, &g| p.mapv_inplace(|v| v * g));
    }
    if flag(edit, "clarity") {
        let blurred = grainy_detail::blur(&a, 2.0 * pixel_scale);
        let amount = num(edit, "clarity", 0.0)? as f32;
        Zip::from(&mut a).and(&blurred).par_for_each(|v, &b| *v += (*v - b) * amount / 100.0);
    }
    if detail {
        let mut options = edit.clone();
        if wavelet {
            // Noise is measured on the whole frame, so the result does not depend on the mask's extent.
            let lab = grainy_color::rgb_to_lab(&grainy_color::clip01(image));
            let profiles = grainy_detail::wavelet_profiles(&lab.index_axis(Axis(2), 0).to_owned());
            let rows: Vec<Value> = profiles
                .iter()
                .map(|row| Value::Array(row.iter().map(|&v| Value::from(f64::from(v))).collect()))
                .collect();
            options.insert("wavelet_profiles".into(), Value::Array(rows));
        }
        // {**EXTRA_DEFAULTS, **edit}: keys the layer does not set read their defaults.
        let options = Settings(options);
        a = if hdr {
            grainy_tone::hdr::color_operation(&a, |bounded| grainy_detail::detail_tools(bounded, &options, pixel_scale))
        } else {
            grainy_detail::detail_tools(&a, &options, pixel_scale)
        };
        if flag(edit, "sharpen") {
            a = grainy_detail::sharpen(&a, num(edit, "sharpen", 0.0)?, 0.8, 50.0, 0.0, pixel_scale, version >= 2.0);
        }
    }
    let mut result = image.clone();
    Zip::from(result.slice_mut(s![y0..y1, x0..x1, ..]).lanes_mut(Axis(2)))
        .and(region.lanes(Axis(2)))
        .and(a.lanes(Axis(2)))
        .and(mask.slice(s![y0..y1, x0..x1]))
        .par_for_each(|mut out, r, v, &w| {
            for c in 0..3 {
                let value = if hdr { maximum(v[c], 0.0) } else { clip(v[c], 0.0, 1.0) };
                out[c] = r[c] * (1.0 - w) + value * w;
            }
        });
    Ok(result)
}
