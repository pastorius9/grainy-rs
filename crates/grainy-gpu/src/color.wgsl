// Port of engine._develop_color and processing.color_tools, matching the CPU operation order.
const centers=array<f32,8>(0.0,1.0/12.0,1.0/6.0,1.0/3.0,0.5,2.0/3.0,0.75,5.0/6.0);
fn band(h: f32,center: f32,width: f32) -> f32 {return max(0.0,1.0-abs(wrap(h-center+0.5)-0.5)/width);}
fn mixer(raw: vec3<f32>,hsl: bool,simd: bool) -> vec3<f32> {
    let a=clip(raw);var h=0.0;var s=0.0;var l=0.0;
    if hsl {let hls=rgb_hls(a,simd);h=hls.x;l=hls.y;s=hls.z;}
    else {
        let hi=max(max(a.r,a.g),a.b);let lo=min(min(a.r,a.g),a.b);let d=hi-lo;let safe=max(d,1e-7);
        if hi==a.b {h=((a.r-a.g)/safe+4.0)/6.0;}
        if hi==a.g {h=((a.b-a.r)/safe+2.0)/6.0;}
        if hi==a.r {h=((a.g-a.b)/safe)/6.0;}
        h=wrap(h);s=select(0.0,d/max(hi,1e-7),hi>0.0);l=hi;
    }
    var dh=0.0;var ds=0.0;var dl=0.0;
    for(var g=0u;g<8u;g++) {
        let w0=band(h,centers[g],0.125);let weight=w0*w0;let j=14u+g*3u;
        dh+=weight*p[j]/600.0;ds+=weight*p[j+1u]/100.0;dl+=weight*p[j+2u]/200.0;
    }
    h=wrap(h+dh);s=clamp(s*(1.0+ds),0.0,1.0);l=clamp(l+dl,0.0,1.0);
    if hsl {return hls_rgb(h,l,s,simd);}
    let raw_i=floor(h*6.0);let f=h*6.0-raw_i;let v0=l*(1.0-s);let v1=l*(1.0-f*s);let v2=l*(1.0-(1.0-f)*s);
    let colors=array<vec3<f32>,6>(vec3(l,v2,v0),vec3(v1,l,v0),vec3(v0,l,v2),vec3(v0,v1,l),vec3(v2,v0,l),vec3(l,v0,v1));
    return colors[u32(raw_i)%6u];
}
@compute @workgroup_size(16,16,1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let w=u32(p[0]);let h=u32(p[1]);if id.x>=w || id.y>=h {return;}
    let index=(id.y*w+id.x)*3u;let flags=u32(p[2]);let simd=id.x<w-w%8u;
    var a=load_rgb(index);
    if (flags&1u)!=0u {
        let gray=luma(a);var factor=p[4];
        if (flags&2u)!=0u {factor+=p[5]*(1.0-(max(max(a.r,a.g),a.b)-min(min(a.r,a.g),a.b)));}
        a=gray+(a-gray)*factor;
    }
    if (flags&12u)!=0u {a=mixer(a,(flags&4u)!=0u,simd);}
    for(var c=0u;c<3u;c++) {if p[11u+c]>0.0 {a[c]=interp(a[c],u32(p[8u+c]),u32(p[11u+c]));}}
    if (flags&16u)!=0u {
        let l=clamp(luma(a),0.0,1.0);var delta=0.0;
        for(var i=0u;i<4u;i++) {let weight=max(0.0,1.0-abs(l-(0.125+f32(i)*0.25))/0.375);delta+=weight*weight*p[38u+i]/400.0;}
        a+=delta;
    }
    if (flags&32u)!=0u {
        let hls=rgb_hls(a,simd);var dh=0.0;var ds=0.0;
        for(var i=0u;i<3u;i++) {let weight=band(hls.x,f32(i)/3.0,0.25);dh+=weight*p[42u+i*2u]/600.0;ds+=weight*p[43u+i*2u]/100.0;}
        a=hls_rgb(hls.x+dh,hls.y,hls.z*(1.0+ds),simd);
    }
    for(var i=0u;i<u32(p[3]);i++) {
        let j=73u+i*16u;let hls=rgb_hls(a,simd);let hue=hls.x;let l=hls.y;let sat=hls.z;
        var weight=clamp(1.0-abs(wrap(hue-p[j]+0.5)-0.5)/p[j+4u],0.0,1.0);
        if p[j+12u]!=0.0 {
            if p[j+13u]!=0.0 {weight=1.0;}
            var sw=clamp(1.0-abs(sat-p[j+1u])/p[j+5u],0.0,1.0);if p[j+14u]!=0.0 {sw=1.0;}
            var lw=clamp(1.0-abs(l-p[j+2u])/p[j+6u],0.0,1.0);if p[j+15u]!=0.0 {lw=1.0;}
            weight=(weight*sw)*lw;
        } else {weight*=clamp(1.0-abs(sat-p[j+1u])/0.6,0.0,1.0);}
        if p[j+12u]==0.0 || weight>0.0 {a=hls_rgb(hue+weight*p[j+8u]/600.0,l+weight*p[j+10u]/200.0,sat*(1.0+weight*p[j+9u]/100.0),simd);}
    }
    if (flags&64u)!=0u {
        let l=clamp(luma(a)+p[6],0.0,1.0);let weights=array<f32,3>((1.0-l)*(1.0-l),4.0*l*(1.0-l),l*l);
        for(var i=0u;i<3u;i++) {a+=weights[i]*vec3(p[48u+i*3u],p[49u+i*3u],p[50u+i*3u]);}
    }
    if (flags&128u)!=0u {
        a=vec3((a.r*p[57]+a.g*p[58])+a.b*p[59],(a.r*p[60]+a.g*p[61])+a.b*p[62],(a.r*p[63]+a.g*p[64])+a.b*p[65]);
    }
    a=clip(a);
    if (flags&512u)!=0u {
        let lin=vec3(lin_srgb(a.r),lin_srgb(a.g),lin_srgb(a.b));
        let gain=vec3(p[66],p[67],p[68]);
        let wt=vec3(p[69],p[70],p[71]);
        var filt=lin*gain;
        if (flags&1024u)!=0u {
            if (flags&2048u)!=0u {
                let comp=filt/p[72];
                let gv=(comp.r*wt.r+comp.g*wt.g)+comp.b*wt.b;
                filt=vec3(gv);
            } else {
                let lum=(lin.r*wt.r+lin.g*wt.g)+lin.b*wt.b;
                let chg=(filt.r*wt.r+filt.g*wt.g)+filt.b*wt.b;
                let ratio=lum/max(chg,1e-12);
                filt*=ratio;
                let delta=filt-vec3(lum);
                let upper=(1.0-lum)/max(max(max(delta.r,delta.g),delta.b),1e-12);
                let lower=lum/max(-min(min(delta.r,delta.g),delta.b),1e-12);
                let amt=clamp(min(upper,lower),0.0,1.0);
                filt=vec3(lum)+delta*amt;
            }
        }
        a=vec3(srgb(max(filt.r,0.0)),srgb(max(filt.g,0.0)),srgb(max(filt.b,0.0)));
    }
    store_rgb(index,a);
}
