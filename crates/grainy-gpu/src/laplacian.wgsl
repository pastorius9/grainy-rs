// Process Version 4 GPU Fast Local Laplacian Pyramid Clarity (Paris et al. 2011 / Aubry et al. 2014).
// 1-channel float32 pyramid stages dispatched inside the shared Batch CommandEncoder (zero readbacks).

@group(0) @binding(0) var<storage, read> src0: array<f32>;
@group(0) @binding(1) var<storage, read> src1: array<f32>;
@group(0) @binding(2) var<storage, read> src2: array<f32>;
@group(0) @binding(3) var<storage, read_write> dst: array<f32>;
@group(0) @binding(4) var<storage, read> lp: array<f32>;

fn reflect101(i: i32, n: i32) -> u32 {
    if n <= 1 {
        return 0u;
    }
    let period = 2 * n - 2;
    let j = ((i % period) + period) % period;
    return u32(select(period - j, j, j < n));
}

fn upsample_at(buf_is_src2: bool, sw: u32, sh: u32, dw: u32, dh: u32, x: u32, y: u32) -> f32 {
    let fx = clamp((f32(x) + 0.5) * (f32(sw) / f32(max(dw, 1u))) - 0.5, 0.0, f32(sw - 1u));
    let fy = clamp((f32(y) + 0.5) * (f32(sh) / f32(max(dh, 1u))) - 0.5, 0.0, f32(sh - 1u));
    let x0 = u32(fx);
    let y0 = u32(fy);
    let x1 = min(x0 + 1u, sw - 1u);
    let y1 = min(y0 + 1u, sh - 1u);
    let tx = fx - f32(x0);
    let ty = fy - f32(y0);
    let i00 = y0 * sw + x0;
    let i10 = y0 * sw + x1;
    let i01 = y1 * sw + x0;
    let i11 = y1 * sw + x1;
    var v00: f32;
    var v10: f32;
    var v01: f32;
    var v11: f32;
    if buf_is_src2 {
        v00 = src2[i00];
        v10 = src2[i10];
        v01 = src2[i01];
        v11 = src2[i11];
    } else {
        v00 = src0[i00];
        v10 = src0[i10];
        v01 = src0[i01];
        v11 = src0[i11];
    }
    let top = v00 + (v10 - v00) * tx;
    let bot = v01 + (v11 - v01) * tx;
    return top + (bot - top) * ty;
}

// lp: [w, h]
@compute @workgroup_size(16, 16, 1)
fn extract_luma(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = u32(lp[0]);
    let h = u32(lp[1]);
    if id.x >= w || id.y >= h {
        return;
    }
    let pix = id.y * w + id.x;
    let i = pix * 3u;
    let y = 0.2126 * src0[i] + 0.7152 * src0[i + 1u] + 0.0722 * src0[i + 2u];
    dst[pix] = clamp(y, 0.0, 1.0);
}

// lp: [w, h, gk, boost, inv_2s2]
@compute @workgroup_size(16, 16, 1)
fn remap_luma(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = u32(lp[0]);
    let h = u32(lp[1]);
    if id.x >= w || id.y >= h {
        return;
    }
    let pix = id.y * w + id.x;
    let y = src0[pix];
    let d = y - lp[2];
    dst[pix] = y + lp[3] * d * exp(-d * d * lp[4]);
}

// lp: [w, h, dw, dh] -> horizontal 5-tap binomial [1,4,6,4,1]/16 at 2*dx
@compute @workgroup_size(16, 16, 1)
fn down_h(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = i32(lp[0]);
    let h = u32(lp[1]);
    let dw = u32(lp[2]);
    if id.x >= dw || id.y >= h {
        return;
    }
    let cx = i32(id.x * 2u);
    let row = id.y * u32(w);
    let s = src0[row + reflect101(cx - 2, w)]
        + 4.0 * src0[row + reflect101(cx - 1, w)]
        + 6.0 * src0[row + reflect101(cx, w)]
        + 4.0 * src0[row + reflect101(cx + 1, w)]
        + src0[row + reflect101(cx + 2, w)];
    dst[id.y * dw + id.x] = s * (1.0 / 16.0);
}

// lp: [dw, h, dw, dh] -> vertical 5-tap binomial [1,4,6,4,1]/16 at 2*dy
@compute @workgroup_size(16, 16, 1)
fn down_v(@builtin(global_invocation_id) id: vec3<u32>) {
    let dw = u32(lp[0]);
    let h = i32(lp[1]);
    let dh = u32(lp[3]);
    if id.x >= dw || id.y >= dh {
        return;
    }
    let cy = i32(id.y * 2u);
    let x = id.x;
    let s = src0[reflect101(cy - 2, h) * dw + x]
        + 4.0 * src0[reflect101(cy - 1, h) * dw + x]
        + 6.0 * src0[reflect101(cy, h) * dw + x]
        + 4.0 * src0[reflect101(cy + 1, h) * dw + x]
        + src0[reflect101(cy + 2, h) * dw + x];
    dst[id.y * dw + x] = s * (1.0 / 16.0);
}

// src0 = g_cur(lw, lh), src1 = r_cur(lw, lh), src2 = r_next(nw, nh), dst = l_out(lw, lh)
// lp: [lw, lh, nw, nh, k, k_max]
@compute @workgroup_size(16, 16, 1)
fn accum_laplacian(@builtin(global_invocation_id) id: vec3<u32>) {
    let lw = u32(lp[0]);
    let lh = u32(lp[1]);
    if id.x >= lw || id.y >= lh {
        return;
    }
    let nw = u32(lp[2]);
    let nh = u32(lp[3]);
    let k = u32(lp[4]);
    let k_max = lp[5];
    let pix = id.y * lw + id.x;
    let t = clamp(src0[pix], 0.0, 1.0) * k_max;
    let k0 = min(u32(floor(t)), u32(k_max) - 1u);
    let f = t - f32(k0);
    var acc = select(dst[pix], 0.0, k == 0u);
    if k == k0 || k == k0 + 1u {
        let up = upsample_at(true, nw, nh, lw, lh, id.x, id.y);
        let band = src1[pix] - up;
        acc += band * select(f, 1.0 - f, k == k0);
    }
    dst[pix] = acc;
}

// src0 = cur(sw, sh), src1 = l_out(dw, dh), dst = next_cur(dw, dh)
// lp: [sw, sh, dw, dh]
@compute @workgroup_size(16, 16, 1)
fn collapse_add(@builtin(global_invocation_id) id: vec3<u32>) {
    let sw = u32(lp[0]);
    let sh = u32(lp[1]);
    let dw = u32(lp[2]);
    let dh = u32(lp[3]);
    if id.x >= dw || id.y >= dh {
        return;
    }
    let pix = id.y * dw + id.x;
    let up = upsample_at(false, sw, sh, dw, dh, id.x, id.y);
    dst[pix] = up + src1[pix];
}

// src0 = rgb_in (3*n), src1 = orig_luma (n), src2 = new_luma (n), dst = rgb_out (3*n)
// lp: [w, h]
@compute @workgroup_size(16, 16, 1)
fn apply_luma(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = u32(lp[0]);
    let h = u32(lp[1]);
    if id.x >= w || id.y >= h {
        return;
    }
    let pix = id.y * w + id.x;
    let i = pix * 3u;
    let scale = (clamp(src2[pix], 0.0, 1.0) + 0.03) / (src1[pix] + 0.03);
    dst[i] = max(src0[i] * scale, 0.0);
    dst[i + 1u] = max(src0[i + 1u] * scale, 0.0);
    dst[i + 2u] = max(src0[i + 2u] * scale, 0.0);
}
