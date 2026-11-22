// Single-pass GPU Catmull-Rom Bicubic geometry warp:
// combines crop, straighten, flip, 90-deg rotation, perspective H/V, scale/aspect/shift, radial k1..k3, and lateral CA
// in a single high-quality resampling pass (eliminating multi-pass CPU bilinear degradation).

fn catmull_rom(t: f32) -> vec4<f32> {
    let t2 = t * t;
    let t3 = t2 * t;
    return vec4(
        -0.5 * t + t2 - 0.5 * t3,
        1.0 - 2.5 * t2 + 1.5 * t3,
        0.5 * t + 2.0 * t2 - 1.5 * t3,
        -0.5 * t2 + 0.5 * t3,
    );
}

fn fetch_zero_ch(x: i32, y: i32, w: i32, h: i32, ch: u32) -> f32 {
    if x < 0 || y < 0 || x >= w || y >= h {
        return 0.0;
    }
    return source[(u32(y) * u32(w) + u32(x)) * 3u + ch];
}

fn sample_bicubic_ch(sx: f32, sy: f32, w: i32, h: i32, ch: u32) -> f32 {
    if sx < -0.5 || sy < -0.5 || sx > f32(w) - 0.5 || sy > f32(h) - 0.5 {
        return 0.0;
    }
    let ix = i32(floor(sx));
    let iy = i32(floor(sy));
    let wx = catmull_rom(sx - f32(ix));
    let wy = catmull_rom(sy - f32(iy));
    var sum = 0.0;
    for (var j = 0; j < 4; j++) {
        let yy = iy + j - 1;
        let row = wx.x * fetch_zero_ch(ix - 1, yy, w, h, ch)
            + wx.y * fetch_zero_ch(ix, yy, w, h, ch)
            + wx.z * fetch_zero_ch(ix + 1, yy, w, h, ch)
            + wx.w * fetch_zero_ch(ix + 2, yy, w, h, ch);
        sum += wy[j] * row;
    }
    return sum;
}

fn sample_bicubic_rgb(sx: f32, sy: f32, w: i32, h: i32) -> vec3<f32> {
    if sx < -0.5 || sy < -0.5 || sx > f32(w) - 0.5 || sy > f32(h) - 0.5 {
        return vec3(0.0);
    }
    let ix = i32(floor(sx));
    let iy = i32(floor(sy));
    let wx = catmull_rom(sx - f32(ix));
    let wy = catmull_rom(sy - f32(iy));
    var sum = vec3(0.0);
    for (var j = 0; j < 4; j++) {
        let yy = iy + j - 1;
        var row = vec3(0.0);
        for (var i = 0; i < 4; i++) {
            let xx = ix + i - 1;
            if xx >= 0 && yy >= 0 && xx < w && yy < h {
                let idx = (u32(yy) * u32(w) + u32(xx)) * 3u;
                row += vec3(source[idx], source[idx + 1u], source[idx + 2u]) * wx[i];
            }
        }
        sum += row * wy[j];
    }
    return sum;
}

fn optical_map(x: f32, y: f32, sw: f32, sh: f32, ca: f32) -> vec2<f32> {
    let scale = max(sw, sh) * 0.5;
    let half_w = (sw - 1.0) * 0.5;
    let half_h = (sh - 1.0) * 0.5;
    let u0 = (x - half_w) / scale;
    let v0 = (y - half_h) / scale;
    let den = max(0.2, 1.0 + p[12] * u0 + p[13] * v0);
    let u = (u0 / den + p[16]) / p[14] / p[15];
    let v = (v0 / den + p[17]) / p[14];
    let r2 = u * u + v * v;
    let factor = 1.0 + p[18] * r2 + p[19] * r2 * r2 + p[20] * r2 * r2 * r2;
    let k = 1.0 + ca;
    return vec2(u * factor * k * scale + half_w, v * factor * k * scale + half_h);
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let out_w = u32(p[0]);
    let out_h = u32(p[1]);
    if id.x >= out_w || id.y >= out_h { return; }
    let src_w = p[2];
    let src_h = p[3];
    let flags = u32(p[4]);
    let turns = u32(p[5]);
    let rot_w = p[8];
    let rot_h = p[9];

    // 1. Crop offset into post-rotation/straighten canvas
    var x = f32(id.x) + p[6];
    var y = f32(id.y) + p[7];

    // 2. Inverse straighten rotation around canvas center
    if (flags & 2u) != 0u {
        let cx = (rot_w - 1.0) * 0.5;
        let cy = (rot_h - 1.0) * 0.5;
        let dx = x - cx;
        let dy = y - cy;
        let ca = p[10];
        let sa = p[11];
        x = dx * ca + dy * sa + cx;
        y = -dx * sa + dy * ca + cy;
    }

    // 3. Remove UseCrop::All straighten padding
    x -= p[23];
    y -= p[24];
    let pre_pad_w = rot_w - 2.0 * p[23];
    let pre_pad_h = rot_h - 2.0 * p[24];
    if x < -0.5 || y < -0.5 || x > pre_pad_w - 0.5 || y > pre_pad_h - 0.5 {
        store_rgb((id.y * out_w + id.x) * 3u, vec3(0.0));
        return;
    }

    // 4. Inverse horizontal flip
    if (flags & 4u) != 0u {
        x = (pre_pad_w - 1.0) - x;
    }

    // 5. Inverse rot90 (turns = (-rotation) % 4 counterclockwise; forward maps src -> rot)
    var ox = x;
    var oy = y;
    if turns == 1u {
        // forward: out[y, x] = src[x, src_w - 1 - y]
        ox = (src_w - 1.0) - y;
        oy = x;
    } else if turns == 2u {
        ox = (src_w - 1.0) - x;
        oy = (src_h - 1.0) - y;
    } else if turns == 3u {
        // forward: out[y, x] = src[src_h - 1 - x, y]
        ox = y;
        oy = (src_h - 1.0) - x;
    }

    let sw_i = i32(src_w);
    let sh_i = i32(src_h);
    let out_idx = (id.y * out_w + id.x) * 3u;

    // 6. Optional optical warp (perspective, shift, scale, aspect, distortion k1..k3, lateral CA)
    if (flags & 1u) != 0u {
        if (flags & 8u) != 0u {
            let pr = optical_map(ox, oy, src_w, src_h, p[21]);
            let pg = optical_map(ox, oy, src_w, src_h, 0.0);
            let pb = optical_map(ox, oy, src_w, src_h, p[22]);
            let rgb = vec3(
                sample_bicubic_ch(pr.x, pr.y, sw_i, sh_i, 0u),
                sample_bicubic_ch(pg.x, pg.y, sw_i, sh_i, 1u),
                sample_bicubic_ch(pb.x, pb.y, sw_i, sh_i, 2u),
            );
            store_rgb(out_idx, rgb);
        } else {
            let pt = optical_map(ox, oy, src_w, src_h, 0.0);
            store_rgb(out_idx, sample_bicubic_rgb(pt.x, pt.y, sw_i, sh_i));
        }
    } else {
        store_rgb(out_idx, sample_bicubic_rgb(ox, oy, sw_i, sh_i));
    }
}
