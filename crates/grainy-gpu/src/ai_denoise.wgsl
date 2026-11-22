// GPU Local AI RAW Denoise Engine: Opponent-Space 7x7 Non-Local Means (3x3 Patch Collaborative Denoiser).
// Exact parity with grainy-detail/src/ai_denoise.rs.
// Params: p[0]=w, p[1]=h, p[2]=inv_h_luma (-1/h_luma^2), p[3]=inv_h_chroma (-1/h_chroma^2), p[4]=blend.

fn ai_to_ycc(rgb: vec3<f32>) -> vec3<f32> {
    let y = (rgb.r * 0.2126 + rgb.g * 0.7152) + rgb.b * 0.0722;
    return vec3(y, rgb.b - y, rgb.r - y);
}

fn ai_from_ycc(ycc: vec3<f32>) -> vec3<f32> {
    let y = ycc.x;
    let cb = ycc.y;
    let cr = ycc.z;
    let r = y + cr;
    let b = y + cb;
    let g = (y - 0.2126 * r - 0.0722 * b) * (1.0 / 0.7152);
    return vec3(r, g, b);
}

fn ai_load_ycc(x: i32, y: i32, w: i32, h: i32) -> vec3<f32> {
    let cx = u32(clamp(x, 0, w - 1));
    let cy = u32(clamp(y, 0, h - 1));
    let i = (cy * u32(w) + cx) * 3u;
    return ai_to_ycc(load_rgb(i));
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let wu = u32(p[0]);
    let hu = u32(p[1]);
    if id.x >= wu || id.y >= hu {
        return;
    }
    let w = i32(wu);
    let h = i32(hu);
    let xi = i32(id.x);
    let yi = i32(id.y);
    let inv_h_luma = p[2];
    let inv_h_chroma = p[3];
    let blend = p[4];

    let idx = (id.y * wu + id.x) * 3u;
    let src_rgb = load_rgb(idx);
    let center = ai_to_ycc(src_rgb);

    // Cache the center 3x3 patch once so we avoid redundant loads inside the 7x7 search loop.
    var cpatch: array<vec3<f32>, 9>;
    for (var py = -1; py <= 1; py++) {
        for (var px = -1; px <= 1; px++) {
            cpatch[u32((py + 1) * 3 + (px + 1))] = ai_load_ycc(xi + px, yi + py, w, h);
        }
    }

    var sum_y = center.x;
    var sum_cb = center.y;
    var sum_cr = center.z;
    var w_y = 1.0;
    var w_c = 1.0;

    for (var dy = -3; dy <= 3; dy++) {
        for (var dx = -3; dx <= 3; dx++) {
            if dx == 0 && dy == 0 {
                continue;
            }
            var dist_y = 0.0;
            var dist_c = 0.0;
            var nb_center = vec3<f32>(0.0);
            for (var py = -1; py <= 1; py++) {
                for (var px = -1; px <= 1; px++) {
                    let a = cpatch[u32((py + 1) * 3 + (px + 1))];
                    let b = ai_load_ycc(xi + dx + px, yi + dy + py, w, h);
                    if px == 0 && py == 0 {
                        nb_center = b;
                    }
                    let d = a - b;
                    dist_y += d.x * d.x;
                    dist_c += d.y * d.y + d.z * d.z;
                }
            }
            let d_luma = (dist_y + 0.25 * dist_c) * (1.0 / 9.0);
            let d_chroma = (0.35 * dist_y + dist_c) * (1.0 / 9.0);
            let wy = exp(d_luma * inv_h_luma);
            let wc = exp(d_chroma * inv_h_chroma);
            sum_y += wy * nb_center.x;
            w_y += wy;
            sum_cb += wc * nb_center.y;
            sum_cr += wc * nb_center.z;
            w_c += wc;
        }
    }
    let rgb = ai_from_ycc(vec3(sum_y / w_y, sum_cb / w_c, sum_cr / w_c));
    store_rgb(idx, src_rgb + (rgb - src_rgb) * blend);
}
