// Port of engine._develop_tone_pixels / native_gpu toneShader + Process Version 4 + 1:1 tile UV support.
fn smoothstep_v4(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}
fn filmic_shoulder_v4(rgb: vec3<f32>) -> vec3<f32> {
    let c = max(rgb, vec3(0.0));
    let peak = max(c.r, max(c.g, c.b));
    let knee = 0.72;
    if peak <= knee {
        return c;
    }
    let headroom = 1.0 - knee;
    let pv = (peak - knee) / headroom;
    let p2 = pv * pv;
    let s_peak = knee + headroom * pv / sqrt(sqrt(1.0 + p2 * p2));
    let scaled = c * (s_peak / peak);
    let d = clamp((peak - 0.85) / 2.2, 0.0, 1.0);
    let desat = d * d * 0.65;
    return scaled + (vec3(s_peak) - scaled) * desat;
}
fn drt_tone(x_in: f32) -> f32 {
    var x = max(x_in, 0.0);
    let mid = 0.18;
    let shoulder = p[29];
    if shoulder < 0.0 {
        let stops = log2(max(x, 1e-12) / mid);
        if stops > 0.0 {
            x = mid * exp2(stops * (1.0 + 0.3 * shoulder));
        }
    }
    let m = p[26];
    var y = m * pow(x / (x + p[25]), p[24]);
    if m > 1.0 {
        y = min(y, 1.0);
    }
    let toe = p[27];
    if toe > 0.0 {
        y = (1.0 + toe) * y * y / (y + toe);
    }
    return y;
}
fn drt_flare(y: f32) -> f32 {
    let fl = max(-p[27], 0.0);
    if fl != 0.0 {
        return fl + (1.0 - fl) * y;
    }
    return y;
}
fn drt_pixel(rgb_in: vec3<f32>) -> vec3<f32> {
    let rgb = max(rgb_in, vec3(0.0));
    let peak = max(max(rgb.r, rgb.g), rgb.b);
    let level_raw = drt_tone(peak);
    let ratio = level_raw / max(peak, 1e-9);
    let level = drt_flare(level_raw);
    let start = p[28];
    let toward0 = clamp((level - start) / (1.0 - start), 0.0, 1.0);
    let toward = toward0 * toward0;
    let out_rgb = vec3(drt_flare(rgb.r * ratio), drt_flare(rgb.g * ratio), drt_flare(rgb.b * ratio));
    return out_rgb + (vec3(level) - out_rgb) * toward;
}
@compute @workgroup_size(16,16,1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let w=u32(p[0]);let h=u32(p[1]);if id.x>=w || id.y>=h {return;}
    let index=(id.y*w+id.x)*3u;let flags=u32(p[2]);let count=u32(p[3]);
    let has_tile=(flags&256u)!=0u;
    let skip_encode=(flags&1024u)!=0u;
    var a=load_rgb(index);
    if !skip_encode {
        a=a*p[7]*vec3(p[4],p[5],p[6]);
        if (flags&8u)!=0u {
            var x=select(-1.0,-1.0+2.0*f32(id.x)/f32(max(w,2u)-1u),w>1u);
            var y=select(-1.0,-1.0+2.0*f32(id.y)/f32(max(h,2u)-1u),h>1u);
            if has_tile {
                x=-1.0+2.0*(p[20]+((f32(id.x)+0.5)/f32(w))*p[22]);
                y=-1.0+2.0*(p[21]+((f32(id.y)+0.5)/f32(h))*p[23]);
            }
            a*=max(0.1,1.0+(x*x+y*y)*p[13]);
        }
    }
    if !skip_encode && (flags&128u)!=0u {
        // Process Version 4: unclipped 12-stop log-luminance local Highlights/Shadows + hue-preserving filmic shoulder.
        if (flags&64u)!=0u {
            let mw=u32(p[17]);let mh=u32(p[18]);let maps=count+257u;
            var sx=clamp((f32(id.x)+0.5)*(f32(mw)/f32(w))-0.5,0.0,f32(mw-1u));
            var sy=clamp((f32(id.y)+0.5)*(f32(mh)/f32(h))-0.5,0.0,f32(mh-1u));
            if has_tile {
                let fx=p[20]+((f32(id.x)+0.5)/f32(w))*p[22];
                let fy=p[21]+((f32(id.y)+0.5)/f32(h))*p[23];
                sx=clamp(fx*f32(mw)-0.5,0.0,f32(mw-1u));
                sy=clamp(fy*f32(mh)-0.5,0.0,f32(mh-1u));
            }
            let x0=u32(sx);let x1=min(x0+1u,mw-1u);let y0=u32(sy);let y1=min(y0+1u,mh-1u);
            let tx=sx-f32(x0);let ty=sy-f32(y0);
            let left=table[maps+y0*mw+x0]*(1.0-ty)+table[maps+y1*mw+x0]*ty;
            let right=table[maps+y0*mw+x1]*(1.0-ty)+table[maps+y1*mw+x1]*ty;
            let k=left*(1.0-tx)+right*tx;
            let pos_a=max(a,vec3(0.0));
            let y_lin=luma(pos_a);
            let u=clamp((log2(max(y_lin,0.0)+1e-4)-(-8.0))/12.0,0.0,1.0);
            let base_u=clamp(k.x*u+k.y,0.0,1.0);
            let w_hl=smoothstep_v4(0.42,0.72,base_u);
            let w_sh=(1.0-smoothstep_v4(0.18,0.50,base_u))*smoothstep_v4(0.0,0.08,base_u);
            let sh=clamp(p[8]/100.0,-1.0,1.0);
            let hl=clamp(p[9]/100.0,-1.0,1.0);
            let gain=exp2(sh*2.2*w_sh+hl*2.4*w_hl);
            a=pos_a*gain;
        }
        a=filmic_shoulder_v4(a);
        a=vec3(srgb(a.r),srgb(a.g),srgb(a.b));
    } else {
        if !skip_encode {
            if (flags&512u)!=0u {
                a=drt_pixel(a);
            } else if (flags&32u)!=0u {
                for(var c=0u;c<3u;c++) {
                    if a[c]>0.5 {let u=(a[c]-0.5)/(p[15]-0.5);a[c]=0.5+0.5*(p[16]*u/(1.0+(p[16]-1.0)*u));}
                }
            }
            a=vec3(srgb(a.r),srgb(a.g),srgb(a.b));
        }
        if (flags&64u)!=0u {
            let mw=u32(p[17]);let mh=u32(p[18]);let maps=count+257u;let l=luma(a);
            var sx=clamp((f32(id.x)+0.5)*(f32(mw)/f32(w))-0.5,0.0,f32(mw-1u));
            var sy=clamp((f32(id.y)+0.5)*(f32(mh)/f32(h))-0.5,0.0,f32(mh-1u));
            if has_tile {
                let fx=p[20]+((f32(id.x)+0.5)/f32(w))*p[22];
                let fy=p[21]+((f32(id.y)+0.5)/f32(h))*p[23];
                sx=clamp(fx*f32(mw)-0.5,0.0,f32(mw-1u));
                sy=clamp(fy*f32(mh)-0.5,0.0,f32(mh-1u));
            }
            let x0=u32(sx);let x1=min(x0+1u,mw-1u);let y0=u32(sy);let y1=min(y0+1u,mh-1u);
            let tx=sx-f32(x0);let ty=sy-f32(y0);
            let left=table[maps+y0*mw+x0]*(1.0-ty)+table[maps+y1*mw+x0]*ty;
            let right=table[maps+y0*mw+x1]*(1.0-ty)+table[maps+y1*mw+x1]*ty;
            let k=left*(1.0-tx)+right*tx;
            let pos=clamp(k.x*l+k.y,0.0,1.0)*256.0;let i=min(u32(pos),255u);let t=pos-f32(i);
            var change=table[count+i].x*(1.0-t)+table[count+i+1u].x*t;
            change=clamp(change,-max(l,0.0)*0.5,max(1.0-l,0.0)*0.5);
            let desired=max(l+change,0.0);
            a=(a*desired+p[19]*(a+desired-l))/(l+p[19]);
        } else if (flags&1u)!=0u {
            let l=luma(a);let low=clamp(1.0-l,0.0,1.0);let high=clamp(l,0.0,1.0);
            a+=p[8]*(low*low*low)*clamp(l*4.0,0.0,1.0)+p[9]*(high*high*high)*clamp((1.0-l)*4.0,0.0,1.0);
        }
    }
    if p[10]<0.0 {a=max(a+p[10],vec3(0.0))/(1.0+p[10]);}
    else if p[10]>0.0 {a=a*(1.0-p[10])+p[10];}
    a*=p[11];if (flags&2u)!=0u {a=(a-0.5)*p[12]+0.5;}
    if (flags&16u)!=0u {
        if (flags&4u)!=0u {
            let last=table[count-1u];
            for(var c=0u;c<3u;c++) {if a[c]>last.x {a[c]=last.y+(a[c]-last.x)*p[14];}else{a[c]=interp(a[c],0u,count);}}
        }
        a=max(a,vec3(0.0));
    } else {
        if (flags&4u)!=0u {a=vec3(interp(clamp(a.x,0.0,1.0),0u,count),interp(clamp(a.y,0.0,1.0),0u,count),interp(clamp(a.z,0.0,1.0),0u,count));}
        a=clip(a);
    }
    store_rgb(index,a);
}
