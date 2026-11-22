// GPU Local Mask adjustments (Zero-Copy in VRAM, matching grainy-local/src/local.rs).
override local_stage: u32 = 0u;

const LOCAL_CENTERS = array<f32, 8>(0.0, 1.0 / 12.0, 1.0 / 6.0, 1.0 / 3.0, 0.5, 2.0 / 3.0, 0.75, 5.0 / 6.0);

fn local_band(h: f32, center: f32, width: f32) -> f32 {
    return max(0.0, 1.0 - abs(wrap(h - center + 0.5) - 0.5) / width);
}

fn local_reflect101(idx: i32, len: i32) -> u32 {
    if len <= 1 { return 0u; }
    let period = 2 * len - 2;
    let m = ((idx % period) + period) % period;
    return u32(select(period - m, m, m < len));
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = u32(p[0]);
    let h = u32(p[1]);
    if id.x >= w || id.y >= h { return; }
    let pix = id.y * w + id.x;
    let index = pix * 3u;

    if local_stage == 0u {
        // Stage 0: per-pixel local tone/colour/curves/hue/HSL; optional immediate mask blend.
        let flags = u32(p[2]);
        let blend_now = (flags & 8u) != 0u;
        let clip_final = (flags & 16u) != 0u;
        let orig = load_rgb(index);
        let mask_w = aux[pix];
        if blend_now && mask_w <= 0.0 {
            store_rgb(index, select(orig, clip(orig), clip_final));
            return;
        }
        let gain = p[3];
        let contrast = p[4];
        let shift = vec3(p[5], p[6], p[7]);
        let sat = p[8];
        let shadows = p[9];
        let highlights = p[10];

        var v = vec3(
            srgb(lin_srgb(orig.r) * gain),
            srgb(lin_srgb(orig.g) * gain),
            srgb(lin_srgb(orig.b) * gain),
        );
        v = (v - vec3(0.5)) * contrast + vec3(0.5) + shift;
        let gray = luma(v);
        v = vec3(gray) + (v - vec3(gray)) * sat;
        let dark0 = 1.0 - clamp(gray, 0.0, 1.0);
        let dark = dark0 * dark0;
        let bright0 = clamp(gray, 0.0, 1.0);
        let bright = bright0 * bright0;
        v += vec3(shadows * dark + highlights * bright);
        if (flags & 1u) != 0u {
            v = v * p[11] + vec3(p[12]);
        }
        // Local curves: channel 0 = master, 1..3 = R/G/B.
        if p[18] > 0.0 {
            let st = u32(p[14]);
            let ct = u32(p[18]);
            v = vec3(interp(v.r, st, ct), interp(v.g, st, ct), interp(v.b, st, ct));
        }
        for (var c = 0u; c < 3u; c++) {
            let ct = u32(p[19u + c]);
            if ct > 0u {
                v[c] = interp(v[c], u32(p[15u + c]), ct);
            }
        }
        let simd = id.x < w - w % 8u;
        if (flags & 2u) != 0u {
            let hls = rgb_hls(v, simd);
            v = hls_rgb(hls.x + p[13], hls.y, hls.z, simd);
        }
        if (flags & 4u) != 0u {
            let hls = rgb_hls(clip(v), simd);
            var dh = 0.0;
            var ds = 0.0;
            var dl = 0.0;
            for (var g = 0u; g < 8u; g++) {
                let w0 = local_band(hls.x, LOCAL_CENTERS[g], 0.125);
                let wt = w0 * w0;
                let j = 22u + g * 3u;
                dh += wt * p[j] / 600.0;
                ds += wt * p[j + 1u] / 100.0;
                dl += wt * p[j + 2u] / 200.0;
            }
            v = hls_rgb(wrap(hls.x + dh), clamp(hls.y + dl, 0.0, 1.0), clamp(hls.z * (1.0 + ds), 0.0, 1.0), simd);
        }
        if blend_now {
            let out_v = orig * (1.0 - mask_w) + clip(v) * mask_w;
            store_rgb(index, select(out_v, clip(out_v), clip_final));
        } else {
            store_rgb(index, v);
        }
        return;
    }

    if local_stage == 1u {
        // Stage 1: 1D horizontal Reflect101 Gaussian blur on RGB frame (for local clarity).
        let klen = u32(p[2]);
        let rad = i32(klen / 2u);
        var sum = vec3(0.0);
        for (var k = 0u; k < klen; k++) {
            let sx = local_reflect101(i32(id.x) + i32(k) - rad, i32(w));
            sum += load_rgb((id.y * w + sx) * 3u) * table[k].x;
        }
        store_rgb(index, sum);
        return;
    }

    if local_stage == 2u {
        // Stage 2: 1D vertical Reflect101 Gaussian blur + local clarity + mask blend.
        let klen = u32(p[2]);
        let rad = i32(klen / 2u);
        let clarity = p[3] / 100.0;
        let clip_final = p[4] != 0.0;
        let mask_w = aux[pix];
        let orig = vec3(extra0[index], extra0[index + 1u], extra0[index + 2u]);
        if mask_w <= 0.0 {
            store_rgb(index, select(orig, clip(orig), clip_final));
            return;
        }
        var blur = vec3(0.0);
        for (var k = 0u; k < klen; k++) {
            let sy = local_reflect101(i32(id.y) + i32(k) - rad, i32(h));
            blur += load_rgb((sy * w + id.x) * 3u) * table[k].x;
        }
        let adj = vec3(extra1[index], extra1[index + 1u], extra1[index + 2u]);
        let v = adj + (adj - blur) * clarity;
        let out_v = orig * (1.0 - mask_w) + clip(v) * mask_w;
        store_rgb(index, select(out_v, clip(out_v), clip_final));
    }
}
