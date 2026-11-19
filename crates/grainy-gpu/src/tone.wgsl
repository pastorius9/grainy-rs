// Port of engine._develop_tone_pixels / native_gpu toneShader.
@compute @workgroup_size(16,16,1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let w=u32(p[0]);let h=u32(p[1]);if id.x>=w || id.y>=h {return;}
    let index=(id.y*w+id.x)*3u;let flags=u32(p[2]);let count=u32(p[3]);
    var a=load_rgb(index)*p[7];a*=vec3(p[4],p[5],p[6]);
    if (flags&8u)!=0u {
        let x=select(-1.0,-1.0+2.0*f32(id.x)/f32(max(w,2u)-1u),w>1u);
        let y=select(-1.0,-1.0+2.0*f32(id.y)/f32(max(h,2u)-1u),h>1u);
        a*=max(0.1,1.0+(x*x+y*y)*p[13]);
    }
    if (flags&32u)!=0u {
        for(var c=0u;c<3u;c++) {
            if a[c]>0.5 {let u=(a[c]-0.5)/(p[15]-0.5);a[c]=0.5+0.5*(p[16]*u/(1.0+(p[16]-1.0)*u));}
        }
    }
    a=vec3(srgb(a.r),srgb(a.g),srgb(a.b));
    if (flags&64u)!=0u {
        let mw=u32(p[17]);let mh=u32(p[18]);let maps=count+257u;let l=luma(a);
        let sx=clamp((f32(id.x)+0.5)*(f32(mw)/f32(w))-0.5,0.0,f32(mw-1u));
        let sy=clamp((f32(id.y)+0.5)*(f32(mh)/f32(h))-0.5,0.0,f32(mh-1u));
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
