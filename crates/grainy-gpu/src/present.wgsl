// Zero-copy GPU display presentation: optional 33^3 tetrahedral monitor ICC 3D LUT,
// Rgba8Unorm storage texture output, exact 128x3 histogram, 6-bit channel clipping flags,
// packed 2-bit clipping mask, and on-demand premultiplied clipping overlay texture.

@group(0) @binding(0) var<storage, read> src: array<f32>;
@group(0) @binding(1) var<storage, read> lut: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> info: array<u32>;
@group(0) @binding(3) var out_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(4) var<storage, read_write> clip_bits: array<u32>;
@group(0) @binding(5) var<storage, read_write> stats: array<atomic<u32>>;

const LOW: f32 = 0.0019607844;   // 1.0 / 510.0 as f32
const HIGH: f32 = 0.9980392;     // 1.0 - 1.0 / 510.0 as f32
const LUT_N: u32 = 33u;

var<workgroup> wg_bins: array<atomic<u32>, 384>;
var<workgroup> wg_chan: atomic<u32>;
var<workgroup> wg_clip: array<u32, 256>;

fn to_byte(v: f32) -> u32 {
    if !(v == v) {
        return 0u;
    }
    return u32(clamp(v, 0.0, 1.0) * 255.0 + 0.5);
}

fn lut_at(base: u32, r: u32, g: u32, b: u32) -> vec3<f32> {
    return lut[base + (r * LUT_N + g) * LUT_N + b].xyz;
}

// Exact tetrahedral interpolation on a 33x33x33 3D LUT (preserves the neutral diagonal).
fn sample_lut33(rgb: vec3<f32>, base: u32) -> vec3<f32> {
    let clamped = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * 32.0;
    let x0 = min(u32(clamped.x), 31u);
    let y0 = min(u32(clamped.y), 31u);
    let z0 = min(u32(clamped.z), 31u);
    let x1 = x0 + 1u;
    let y1 = y0 + 1u;
    let z1 = z0 + 1u;
    let dx = clamped.x - f32(x0);
    let dy = clamped.y - f32(y0);
    let dz = clamped.z - f32(z0);

    let c000 = lut_at(base, x0, y0, z0);
    let c111 = lut_at(base, x1, y1, z1);
    if dx >= dy {
        if dy >= dz {
            let c100 = lut_at(base, x1, y0, z0);
            let c110 = lut_at(base, x1, y1, z0);
            return c000 + (c100 - c000) * dx + (c110 - c100) * dy + (c111 - c110) * dz;
        } else if dx >= dz {
            let c100 = lut_at(base, x1, y0, z0);
            let c101 = lut_at(base, x1, y0, z1);
            return c000 + (c100 - c000) * dx + (c111 - c101) * dy + (c101 - c100) * dz;
        } else {
            let c001 = lut_at(base, x0, y0, z1);
            let c101 = lut_at(base, x1, y0, z1);
            return c000 + (c101 - c001) * dx + (c111 - c101) * dy + (c001 - c000) * dz;
        }
    } else {
        if dz >= dy {
            let c001 = lut_at(base, x0, y0, z1);
            let c011 = lut_at(base, x0, y1, z1);
            return c000 + (c111 - c011) * dx + (c011 - c001) * dy + (c001 - c000) * dz;
        } else if dz >= dx {
            let c010 = lut_at(base, x0, y1, z0);
            let c011 = lut_at(base, x0, y1, z1);
            return c000 + (c111 - c011) * dx + (c010 - c000) * dy + (c011 - c010) * dz;
        } else {
            let c010 = lut_at(base, x0, y1, z0);
            let c110 = lut_at(base, x1, y1, z0);
            return c000 + (c110 - c010) * dx + (c010 - c000) * dy + (c111 - c110) * dz;
        }
    }
}

@compute @workgroup_size(16, 16, 1)
fn present_frame(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(local_invocation_index) lidx: u32,
    @builtin(workgroup_id) wid: vec3<u32>,
) {
    let src_w = info[0];
    let crop_x = info[2];
    let crop_y = info[3];
    let out_w = info[4];
    let out_h = info[5];
    let hist_step = info[6];
    let flags = info[7];

    // Clear workgroup histogram (384 bins across 256 threads).
    atomicStore(&wg_bins[lidx], 0u);
    if lidx < 128u {
        atomicStore(&wg_bins[256u + lidx], 0u);
    }
    if lidx == 0u {
        atomicStore(&wg_chan, 0u);
    }
    wg_clip[lidx] = 0u;
    workgroupBarrier();

    let ox = gid.x;
    let oy = gid.y;
    if ox < out_w && oy < out_h {
        let sx = ox + crop_x;
        let sy = oy + crop_y;
        let idx = (sy * src_w + sx) * 3u;
        let raw = vec3<f32>(src[idx], src[idx + 1u], src[idx + 2u]);

        // Working -> sRGB for histogram & clipping (identity unless flag bit 2 is set).
        var srgb = raw;
        if (flags & 4u) != 0u {
            srgb = sample_lut33(raw, 35937u);
        }
        let sr = to_byte(srgb.x);
        let sg = to_byte(srgb.y);
        let sb = to_byte(srgb.z);

        var chan = 0u;
        var bits = 0u;
        if srgb.x < LOW { chan |= 1u; bits |= 2u; }
        if srgb.y < LOW { chan |= 2u; bits |= 2u; }
        if srgb.z < LOW { chan |= 4u; bits |= 2u; }
        if srgb.x >= HIGH { chan |= 8u; bits |= 1u; }
        if srgb.y >= HIGH { chan |= 16u; bits |= 1u; }
        if srgb.z >= HIGH { chan |= 32u; bits |= 1u; }
        if chan != 0u {
            atomicOr(&wg_chan, chan);
        }
        wg_clip[lidx] = bits;

        if hist_step > 0u && (ox % hist_step) == 0u && (oy % hist_step) == 0u {
            atomicAdd(&wg_bins[sr >> 1u], 1u);
            atomicAdd(&wg_bins[128u + (sg >> 1u)], 1u);
            atomicAdd(&wg_bins[256u + (sb >> 1u)], 1u);
        }

        // Display colour: monitor 3D LUT when flag bit 0 is set, else direct sRGB bytes.
        var dr = sr;
        var dg = sg;
        var db = sb;
        if (flags & 1u) != 0u {
            let disp = sample_lut33(raw, 0u);
            dr = to_byte(disp.x);
            dg = to_byte(disp.y);
            db = to_byte(disp.z);
        }
        textureStore(
            out_tex,
            vec2<i32>(i32(ox), i32(oy)),
            vec4<f32>(f32(dr) / 255.0, f32(dg) / 255.0, f32(db) / 255.0, 1.0),
        );
    }

    workgroupBarrier();

    // Pack 16 horizontal pixels' 2-bit clip masks into one u32 per row (zero atomics).
    if (flags & 2u) != 0u && lid.x == 0u && oy < out_h {
        let row_base = lid.y * 16u;
        var packed = 0u;
        for (var k = 0u; k < 16u; k++) {
            packed |= (wg_clip[row_base + k] & 3u) << (k * 2u);
        }
        let stride_u32 = (out_w + 15u) / 16u;
        clip_bits[oy * stride_u32 + wid.x] = packed;
    }

    // Merge workgroup histogram & channel clip flags into global stats.
    if hist_step > 0u {
        let v0 = atomicLoad(&wg_bins[lidx]);
        if v0 != 0u {
            atomicAdd(&stats[lidx], v0);
        }
        if lidx < 128u {
            let v1 = atomicLoad(&wg_bins[256u + lidx]);
            if v1 != 0u {
                atomicAdd(&stats[256u + lidx], v1);
            }
        }
    }
    if lidx == 0u {
        let c = atomicLoad(&wg_chan);
        if c != 0u {
            atomicOr(&stats[384u], c);
        }
    }
}

// Renders packed 2-bit clipping mask into a premultiplied-alpha Rgba8Unorm overlay texture.
// info: [out_w, out_h, show_shadows, show_highlights]
@compute @workgroup_size(16, 16, 1)
fn clip_overlay(@builtin(global_invocation_id) gid: vec3<u32>) {
    let out_w = info[0];
    let out_h = info[1];
    if gid.x >= out_w || gid.y >= out_h {
        return;
    }
    let show_shadows = info[2] != 0u;
    let show_highlights = info[3] != 0u;
    let stride_u32 = (out_w + 15u) / 16u;
    let word = clip_bits[gid.y * stride_u32 + (gid.x >> 4u)];
    let bits = (word >> ((gid.x & 15u) * 2u)) & 3u;

    // Premultiplied alpha matching Color32::from_rgba_unmultiplied in egui:
    // highlight (255, 40, 30, 150) -> premul (150, 24, 18, 150) / 255
    // shadow    (40, 90, 255, 160) -> premul (25, 56, 160, 160) / 255
    var color = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    if show_highlights && (bits & 1u) != 0u {
        let a = 150.0 / 255.0;
        color = vec4<f32>(1.0 * a, (40.0 / 255.0) * a, (30.0 / 255.0) * a, a);
    }
    if show_shadows && (bits & 2u) != 0u {
        let a = 160.0 / 255.0;
        color = vec4<f32>((40.0 / 255.0) * a, (90.0 / 255.0) * a, 1.0 * a, a);
    }
    textureStore(out_tex, vec2<i32>(i32(gid.x), i32(gid.y)), color);
}
