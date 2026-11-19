// Shared native_gpu tone/color/effects primitives. Float32 RGB storage, no texture conversion.
@group(0) @binding(0) var<storage, read> source: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@group(0) @binding(2) var<storage, read> p: array<f32>;
@group(0) @binding(3) var<storage, read> table: array<vec2<f32>>;
@group(0) @binding(4) var<storage, read> aux: array<f32>;
@group(0) @binding(5) var<storage, read> extra0: array<f32>;
@group(0) @binding(6) var<storage, read> extra1: array<f32>;
@group(0) @binding(7) var<storage, read> extra2: array<f32>;

fn load_rgb(i: u32) -> vec3<f32> { return vec3(source[i],source[i+1],source[i+2]); }
fn store_rgb(i: u32,a: vec3<f32>) { output[i]=a.x; output[i+1]=a.y; output[i+2]=a.z; }
fn clip(a: vec3<f32>) -> vec3<f32> { return clamp(a,vec3(0.0),vec3(1.0)); }
fn luma(a: vec3<f32>) -> f32 { return (a.x*0.2126+a.y*0.7152)+a.z*0.0722; }
fn wrap(x: f32) -> f32 { return x-floor(x); }
fn srgb(x: f32) -> f32 {
    let a=max(x,0.0);
    if a<=0.0031308 {return a*12.92;}
    return 1.055*pow(a,1.0/2.4)-0.055;
}
// numpy.interp uses the last duplicate abscissa. Binary search also bounds arbitrary-size curves.
fn interp(x: f32,first: u32,count: u32) -> f32 {
    if x<table[first].x {return table[first].y;}
    var lo=0u; var hi=count;
    while lo<hi {
        let m=lo+(hi-lo)/2u;
        if x>=table[first+m].x {lo=m+1u;} else {hi=m;}
    }
    let i=first+lo-1u;
    if lo==count || x==table[i].x {return table[i].y;}
    let a=table[i];let b=table[i+1u];
    return a.y+(x-a.x)*((b.y-a.y)/(b.x-a.x));
}

// OpenCV's CPU HLS SIMD lanes and scalar tail, retained by the Rust CPU reference.
fn rgb_hls(raw: vec3<f32>,simd: bool) -> vec3<f32> {
    let c=clip(raw);let hi=max(max(c.r,c.g),c.b);let lo=min(min(c.r,c.g),c.b);
    let d=hi-lo;let sum=hi+lo;let l=sum*0.5;
    if d<=1.1920929e-7 {return vec3(0.0,l,0.0);}
    var divisor=2.0-hi-lo;
    if simd {divisor=2.0-sum;}
    if l<0.5 {divisor=sum;}
    let s=d/divisor;let scale=60.0/d;
    var h=0.0;
    if simd {
        if hi==c.r {h=fma(c.g-c.b,scale,select(0.0,360.0,c.g<c.b));}
        else if hi==c.g {h=fma(c.b-c.r,scale,120.0);}
        else {h=fma(c.r-c.g,scale,240.0);}
    } else {
        if hi==c.r {h=(c.g-c.b)*scale;}
        else if hi==c.g {h=(c.b-c.r)*scale+120.0;}
        else {h=(c.r-c.g)*scale+240.0;}
        if h<0.0 {h+=360.0;}
    }
    return vec3(h/360.0,l,s);
}
fn hls_rgb(hue: f32,light: f32,sat: f32,simd: bool) -> vec3<f32> {
    let h=wrap(hue)*360.0;let l=clamp(light,0.0,1.0);let s=clamp(sat,0.0,1.0);
    let raw=h*(6.0/360.0);
    var sector=0u;
    var v: array<f32,4>;
    if simd {
        let ls=l*s;let e=select(s-ls,ls,l<=0.5);let hs=raw-trunc(raw);
        sector=u32(trunc(raw)-6.0*trunc(raw*(1.0/6.0)))%6u;
        let e1=hs+hs;let t0=l+e;let t1=l-e;
        v=array<f32,4>(t0,t1,t0-e*e1,t1+e*e1);
    } else {
        if s==0.0 {return vec3(l);}
        let t2=select(l+s-l*s,l*(1.0+s),l<=0.5);let t1=2.0*l-t2;
        let wrapped=raw-6.0*floor(raw/6.0);let hs=wrapped-floor(wrapped);sector=u32(floor(wrapped))%6u;
        v=array<f32,4>(t2,t1,t1+(t2-t1)*(1.0-hs),t1+(t2-t1)*hs);
    }
    let orders=array<vec3<u32>,6>(vec3(0u,3u,1u),vec3(2u,0u,1u),vec3(1u,0u,3u),vec3(1u,2u,0u),vec3(3u,1u,0u),vec3(0u,1u,2u));
    let o=orders[sector];return vec3(v[o.x],v[o.y],v[o.z]);
}
