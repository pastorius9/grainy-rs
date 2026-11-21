// GPU-resident double-float reductions for wavelet luminance noise profiles.
// Computes the exact same 8-bin luminance noise profile as CPU noise_profile() without host readbacks.
@group(0) @binding(0) var<storage, read> energy: array<f32>;
@group(0) @binding(1) var<storage, read_write> scratch: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> profile_buf: array<vec2<f32>>;
@group(0) @binding(3) var<storage, read> info: array<u32>;

const WG_SIZE: u32 = 256u;
const LIMIT_SLOT: u32 = 2048u;

var<workgroup> sh_df: array<vec2<f32>, 256>;
var<workgroup> sh_cnt: array<u32, 256>;

fn df_add(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    let s = a.x + b.x;
    let bb = s - a.x;
    let err = (a.x - (s - bb)) + (b.x - bb);
    let lo = err + a.y + b.y;
    let hi = s + lo;
    return vec2<f32>(hi, lo - (hi - s));
}

fn df_mul_inv_count(sum: vec2<f32>) -> f32 {
    let inv_hi = bitcast<f32>(info[2]);
    let inv_lo = bitcast<f32>(info[3]);
    return fma(sum.x, inv_hi, sum.y * inv_hi + sum.x * inv_lo);
}

fn df_div_u32(sum: vec2<f32>, cnt: u32) -> f32 {
    let d_hi = f32(cnt & 0xffff0000u);
    let d_lo = f32(cnt & 0x0000ffffu);
    let d = d_hi + d_lo;
    let q = sum.x / d;
    let r = (fma(-q, d_hi, sum.x) - q * d_lo) + sum.y;
    return q + r / d;
}

@compute @workgroup_size(256, 1, 1)
fn mean_part(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) local: vec3<u32>) {
    let count = info[0];
    let groups = info[1];
    let lid = local.x;
    let stride = groups * WG_SIZE;
    var acc = vec2<f32>(0.0, 0.0);
    for (var i = wg.x * WG_SIZE + lid; i < count; i += stride) {
        acc = df_add(acc, vec2<f32>(abs(energy[i * 3u + 1u]), 0.0));
    }
    sh_df[lid] = acc;
    workgroupBarrier();
    for (var s = 128u; s > 0u; s >>= 1u) {
        if lid < s {
            sh_df[lid] = df_add(sh_df[lid], sh_df[lid + s]);
        }
        workgroupBarrier();
    }
    if lid == 0u {
        scratch[wg.x] = vec4<f32>(sh_df[0].x, sh_df[0].y, 0.0, 0.0);
    }
}

@compute @workgroup_size(256, 1, 1)
fn mean_final(@builtin(local_invocation_id) local: vec3<u32>) {
    let groups = info[1];
    let lid = local.x;
    sh_df[lid] = select(vec2<f32>(0.0, 0.0), scratch[lid].xy, lid < groups);
    workgroupBarrier();
    for (var s = 128u; s > 0u; s >>= 1u) {
        if lid < s {
            sh_df[lid] = df_add(sh_df[lid], sh_df[lid + s]);
        }
        workgroupBarrier();
    }
    if lid == 0u {
        let mean = df_mul_inv_count(sh_df[0]);
        scratch[LIMIT_SLOT] = vec4<f32>(2.5 * mean, 0.0, 0.0, 0.0);
    }
}

@compute @workgroup_size(256, 1, 1)
fn hist_part(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) local: vec3<u32>) {
    let count = info[0];
    let groups = info[1];
    let lid = local.x;
    let stride = groups * WG_SIZE;
    let limit = scratch[LIMIT_SLOT].x;
    var sums: array<vec2<f32>, 8>;
    var cnts: array<u32, 8>;
    for (var b = 0u; b < 8u; b += 1u) {
        sums[b] = vec2<f32>(0.0, 0.0);
        cnts[b] = 0u;
    }
    for (var i = wg.x * WG_SIZE + lid; i < count; i += stride) {
        let base = i * 3u;
        let val = min(abs(energy[base + 1u]), limit);
        let guide = energy[base + 2u];
        let bin = u32(clamp(i32(clamp(guide * 0.08, 0.0, 7.0)), 0, 7));
        sums[bin] = df_add(sums[bin], vec2<f32>(val, 0.0));
        cnts[bin] += 1u;
    }
    for (var b = 0u; b < 8u; b += 1u) {
        sh_df[lid] = sums[b];
        sh_cnt[lid] = cnts[b];
        workgroupBarrier();
        for (var s = 128u; s > 0u; s >>= 1u) {
            if lid < s {
                sh_df[lid] = df_add(sh_df[lid], sh_df[lid + s]);
                sh_cnt[lid] += sh_cnt[lid + s];
            }
            workgroupBarrier();
        }
        if lid == 0u {
            scratch[wg.x * 8u + b] = vec4<f32>(sh_df[0].x, sh_df[0].y, bitcast<f32>(sh_cnt[0]), 0.0);
        }
        workgroupBarrier();
    }
}

@compute @workgroup_size(256, 1, 1)
fn hist_final(@builtin(local_invocation_id) local: vec3<u32>) {
    let count = info[0];
    let groups = info[1];
    let lid = local.x;
    var bin_sum: array<vec2<f32>, 8>;
    var bin_cnt: array<u32, 8>;
    for (var b = 0u; b < 8u; b += 1u) {
        if lid < groups {
            let v = scratch[lid * 8u + b];
            sh_df[lid] = v.xy;
            sh_cnt[lid] = bitcast<u32>(v.z);
        } else {
            sh_df[lid] = vec2<f32>(0.0, 0.0);
            sh_cnt[lid] = 0u;
        }
        workgroupBarrier();
        for (var s = 128u; s > 0u; s >>= 1u) {
            if lid < s {
                sh_df[lid] = df_add(sh_df[lid], sh_df[lid + s]);
                sh_cnt[lid] += sh_cnt[lid + s];
            }
            workgroupBarrier();
        }
        if lid == 0u {
            bin_sum[b] = sh_df[0];
            bin_cnt[b] = sh_cnt[0];
        }
        workgroupBarrier();
    }
    if lid == 0u {
        var total = vec2<f32>(0.0, 0.0);
        for (var b = 0u; b < 8u; b += 1u) {
            total = df_add(total, bin_sum[b]);
        }
        let overall = df_mul_inv_count(total);
        let minimum = max(256u, count / 2000u);
        for (var b = 0u; b < 8u; b += 1u) {
            var pval = overall;
            if bin_cnt[b] > minimum {
                pval = df_div_u32(bin_sum[b], bin_cnt[b]);
            }
            profile_buf[b] = vec2<f32>(pval, 0.0);
        }
    }
}

@compute @workgroup_size(8, 1, 1)
fn halve_profile(@builtin(local_invocation_id) local: vec3<u32>) {
    let b = local.x;
    if b < 8u {
        profile_buf[b] = vec2<f32>(profile_buf[b].x * 0.5, 0.0);
    }
}
