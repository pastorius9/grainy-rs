// GPU Spectral Film Look (all 82 stocks + separable Reflect101 Halation + partial-strength DRT/PV4/rolloff blend).
override film_stage: u32 = 0u;

const LOG10E: f32 = 0.4342944819032518;
const LN10: f32 = 2.302585092994046;
const FILM_NODES: u32 = 33u;
const FILM_SAMPLES: u32 = 2048u;
const FILM_SLOPES_OFF: u32 = 6144u;   // 3 * 2048
const FILM_CUBE_OFF: u32 = 12288u;    // 6 * 2048

fn reflect101(idx: i32, len: i32) -> u32 {
    if len <= 1 { return 0u; }
    var i = idx;
    loop {
        if i < 0 { i = -i; }
        else if i >= len { i = 2 * len - 2 - i; }
        else { break; }
    }
    return u32(i);
}

fn mat3_mul(v: vec3<f32>, off: u32) -> vec3<f32> {
    return vec3(
        (v.x * p[off] + v.y * p[off + 1u]) + v.z * p[off + 2u],
        (v.x * p[off + 3u] + v.y * p[off + 4u]) + v.z * p[off + 5u],
        (v.x * p[off + 6u] + v.y * p[off + 7u]) + v.z * p[off + 8u],
    );
}

fn filmic_shoulder_plain(rgb: vec3<f32>) -> vec3<f32> {
    let c = max(rgb, vec3(0.0));
    let peak = max(c.r, max(c.g, c.b));
    let knee = 0.72;
    if peak <= knee { return c; }
    let headroom = 1.0 - knee;
    let pv = (peak - knee) / headroom;
    let p2 = pv * pv;
    let s_peak = knee + headroom * pv / sqrt(sqrt(1.0 + p2 * p2));
    let scaled = c * (s_peak / peak);
    let d = clamp((peak - 0.85) / 2.2, 0.0, 1.0);
    let desat = d * d * 0.65;
    return scaled + (vec3(s_peak) - scaled) * desat;
}

fn film_drt_tone(x_in: f32) -> f32 {
    var x = max(x_in, 0.0);
    let mid = 0.18;
    let shoulder = p[60];
    if shoulder < 0.0 {
        let stops = log2(max(x, 1e-12) / mid);
        if stops > 0.0 {
            x = mid * exp2(stops * (1.0 + 0.3 * shoulder));
        }
    }
    let m = p[57];
    var y = m * pow(x / (x + p[56]), p[55]);
    if m > 1.0 { y = min(y, 1.0); }
    let toe = p[58];
    if toe > 0.0 { y = (1.0 + toe) * y * y / (y + toe); }
    return y;
}

fn film_drt_flare(y: f32) -> f32 {
    let fl = max(-p[58], 0.0);
    if fl != 0.0 { return fl + (1.0 - fl) * y; }
    return y;
}

fn film_drt_pixel(rgb_in: vec3<f32>) -> vec3<f32> {
    let rgb = max(rgb_in, vec3(0.0));
    let peak = max(max(rgb.r, rgb.g), rgb.b);
    let level_raw = film_drt_tone(peak);
    let ratio = level_raw / max(peak, 1e-9);
    let level = film_drt_flare(level_raw);
    let start = p[59];
    let toward0 = clamp((level - start) / (1.0 - start), 0.0, 1.0);
    let toward = toward0 * toward0;
    let out_rgb = vec3(film_drt_flare(rgb.r * ratio), film_drt_flare(rgb.g * ratio), film_drt_flare(rgb.b * ratio));
    return out_rgb + (vec3(level) - out_rgb) * toward;
}

fn cube_at(idx: u32) -> vec3<f32> {
    let o = FILM_CUBE_OFF + idx * 3u;
    return vec3(extra0[o], extra0[o + 1u], extra0[o + 2u]);
}

fn stock_xyz(rgb_lin: vec3<f32>, is_bw: bool) -> vec3<f32> {
    let offset = p[40];
    let scale = p[41];
    let top = p[42];
    let e0 = dot(rgb_lin, vec3(p[31], p[32], p[33]));
    let x0 = log(max(e0, 1e-7)) * LOG10E;
    let u0 = clamp((x0 + offset) * scale, 0.0, top);
    let i0 = u32(u0);
    let d0 = extra0[i0] + extra0[FILM_SLOPES_OFF + i0] * (u0 - f32(i0));
    if is_bw {
        let y = exp(-LN10 * (d0 - p[49]));
        return vec3(y * p[61], y * p[62], y * p[63]);
    }
    let e1 = dot(rgb_lin, vec3(p[34], p[35], p[36]));
    let x1 = log(max(e1, 1e-7)) * LOG10E;
    let u1 = clamp((x1 + offset) * scale, 0.0, top);
    let i1 = u32(u1);
    let d1 = extra0[FILM_SAMPLES + i1] + extra0[FILM_SLOPES_OFF + FILM_SAMPLES + i1] * (u1 - f32(i1));

    let e2 = dot(rgb_lin, vec3(p[37], p[38], p[39]));
    let x2 = log(max(e2, 1e-7)) * LOG10E;
    let u2 = clamp((x2 + offset) * scale, 0.0, top);
    let i2 = u32(u2);
    let d2 = extra0[2u * FILM_SAMPLES + i2] + extra0[FILM_SLOPES_OFF + 2u * FILM_SAMPLES + i2] * (u2 - f32(i2));

    let top_n = f32(FILM_NODES) - 1.001;
    let v0 = clamp((d0 - p[43]) * p[46], 0.0, top_n);
    let v1 = clamp((d1 - p[44]) * p[47], 0.0, top_n);
    let v2 = clamp((d2 - p[45]) * p[48], 0.0, top_n);
    let c0 = u32(v0);
    let c1 = u32(v1);
    let c2 = u32(v2);
    let t0 = v0 - f32(c0);
    let t1 = v1 - f32(c1);
    let t2 = v2 - f32(c2);
    let s0 = 1.0 - t0;
    let s1 = 1.0 - t1;
    let s2 = 1.0 - t2;

    let n1 = FILM_NODES;
    let n2 = FILM_NODES * FILM_NODES;
    let base = (c0 * n1 + c1) * n1 + c2;
    let a00 = cube_at(base) * s0 + cube_at(base + n2) * t0;
    let a01 = cube_at(base + 1u) * s0 + cube_at(base + n2 + 1u) * t0;
    let a10 = cube_at(base + n1) * s0 + cube_at(base + n2 + n1) * t0;
    let a11 = cube_at(base + n1 + 1u) * s0 + cube_at(base + n2 + n1 + 1u) * t0;
    let neg_log_xyz = (a00 * s2 + a01 * t2) * s1 + (a10 * s2 + a11 * t2) * t1;
    return exp(-LN10 * neg_log_xyz);
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = u32(p[0]);
    let h = u32(p[1]);
    if id.x >= w || id.y >= h { return; }

    if film_stage == 0u {
        // Stage 0: exposure * white_balance * lens_vignette -> prepared_linear
        let index = (id.y * w + id.x) * 3u;
        let flags = u32(p[2]);
        var a = load_rgb(index) * p[7] * vec3(p[4], p[5], p[6]);
        if (flags & 8u) != 0u {
            var x = select(-1.0, -1.0 + 2.0 * f32(id.x) / f32(max(w, 2u) - 1u), w > 1u);
            var y = select(-1.0, -1.0 + 2.0 * f32(id.y) / f32(max(h, 2u) - 1u), h > 1u);
            if (flags & 256u) != 0u {
                x = -1.0 + 2.0 * (p[9] + ((f32(id.x) + 0.5) / f32(w)) * p[11]);
                y = -1.0 + 2.0 * (p[10] + ((f32(id.y) + 0.5) / f32(h)) * p[12]);
            }
            a *= max(0.1, 1.0 + (x * x + y * y) * p[8]);
        }
        store_rgb(index, a);
        return;
    }

    if film_stage == 1u {
        // Stage 1: halation threshold extraction -> scalar plane in output[y*w + x]
        let index = (id.y * w + id.x) * 3u;
        let rgb = mat3_mul(load_rgb(index), 4u);
        let lum = luma(rgb);
        output[id.y * w + id.x] = max(lum - p[3], 0.0);
        return;
    }

    if film_stage == 2u {
        // Stage 2: 1D horizontal Reflect101 Gaussian blur on scalar plane
        let klen = u32(p[2]);
        let rad = i32(klen / 2u);
        let row = id.y * w;
        var sum = 0.0;
        for (var k = 0u; k < klen; k++) {
            let sx = reflect101(i32(id.x) + i32(k) - rad, i32(w));
            sum += source[row + sx] * table[k].x;
        }
        output[row + id.x] = sum;
        return;
    }

    if film_stage == 3u {
        // Stage 3: 1D vertical Reflect101 Gaussian blur on scalar plane
        let klen = u32(p[2]);
        let rad = i32(klen / 2u);
        var sum = 0.0;
        for (var k = 0u; k < klen; k++) {
            let sy = reflect101(i32(id.y) + i32(k) - rad, i32(h));
            sum += source[sy * w + id.x] * table[k].x;
        }
        output[id.y * w + id.x] = sum;
        return;
    }

    // Stage 4: film stock evaluation + halation + optional blend with plain DRT/PV4/rolloff -> sRGB encoded
    let index = (id.y * w + id.x) * 3u;
    let flags = u32(p[2]);
    let strength = p[3];
    let p_in = load_rgb(index);
    var p_film = p_in;
    if (flags & 2u) != 0u {
        let spread = aux[id.y * w + id.x];
        let glow_srgb = vec3(spread * p[50], spread * p[51], spread * p[52]);
        p_film = p_in + mat3_mul(glow_srgb, 22u);
    }
    let film_lin_srgb = mat3_mul(p_film, 4u);
    let xyz = stock_xyz(film_lin_srgb, (flags & 1u) != 0u);
    let film_work = mat3_mul(xyz, 13u);
    let film_srgb = vec3(srgb(film_work.r), srgb(film_work.g), srgb(film_work.b));
    if strength >= 1.0 {
        store_rgb(index, film_srgb);
        return;
    }
    var plain = p_in;
    if (flags & 4u) != 0u {
        plain = film_drt_pixel(plain);
    } else if (flags & 8u) != 0u {
        plain = filmic_shoulder_plain(plain);
    } else if (flags & 16u) != 0u {
        let rw = p[53];
        let rs = p[54];
        for (var c = 0u; c < 3u; c++) {
            if plain[c] > 0.5 {
                let u = (plain[c] - 0.5) / (rw - 0.5);
                plain[c] = 0.5 + 0.5 * (rs * u / (1.0 + (rs - 1.0) * u));
            }
        }
    }
    let plain_srgb = vec3(srgb(plain.r), srgb(plain.g), srgb(plain.b));
    store_rgb(index, plain_srgb + (film_srgb - plain_srgb) * strength);
}
