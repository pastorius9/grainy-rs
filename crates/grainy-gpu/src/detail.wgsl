// Port of processing.detail_tools, sharpen and engine.post_crop_vignette.
override detail_stage:u32;
fn reflected(i: i32,n: i32)->u32 {if n<=1{return 0u;}let period=2*n-2;let j=((i%period)+period)%period;return u32(select(period-j,j,j<n));}
fn at(x: i32,y: i32)->vec3<f32>{let i=(reflected(y,i32(p[1]))*u32(p[0])+reflected(x,i32(p[0])))*3u;return load_rgb(i);}
fn from_aux(i:u32)->vec3<f32>{return vec3(aux[i],aux[i+1u],aux[i+2u]);}
fn from_e0(i:u32)->vec3<f32>{return vec3(extra0[i],extra0[i+1u],extra0[i+2u]);}
fn from_e1(i:u32)->vec3<f32>{return vec3(extra1[i],extra1[i+1u],extra1[i+2u]);}
fn from_e2(i:u32)->vec3<f32>{return vec3(extra2[i],extra2[i+1u],extra2[i+2u]);}
fn gray(x:i32,y:i32)->f32{return luma(at(x,y));}
fn channel_max(x:i32,y:i32)->f32{let a=at(x,y);return max(max(a.r,a.g),a.b);}
fn linear(x:f32)->f32{if x<=0.04045{return x/12.92;}return pow((x+0.055)/1.055,2.4);}
fn lab_f(x:f32)->f32{if x>0.008856{return pow(x,1.0/3.0);}return 7.787*x+16.0/116.0;}
fn lab_inv(x:f32)->f32{if x<=0.20689303{return (x-16.0/116.0)/7.787;}return x*x*x;}
fn rgb_lab(rgb:vec3<f32>)->vec3<f32>{
    let c=clip(rgb);let r=linear(c.r);let g=linear(c.g);let b=linear(c.b);
    let x=(0.412453*r+0.357580*g+0.180423*b)/0.950456;let y=0.212671*r+0.715160*g+0.072169*b;
    let z=(0.019334*r+0.119193*g+0.950227*b)/1.088754;let fx=lab_f(x);let fy=lab_f(y);let fz=lab_f(z);
    return vec3(select(903.3*y,116.0*fy-16.0,y>0.008856),500.0*(fx-fy),200.0*(fy-fz));
}
fn lab_rgb(a:vec3<f32>)->vec3<f32>{
    let low=a.x<=7.9996248;let fy=select((a.x+16.0)/116.0,7.787*(a.x/903.3)+16.0/116.0,low);
    let y=select(fy*fy*fy,a.x/903.3,low);let x=lab_inv(fy+a.y/500.0);let z=lab_inv(fy-a.z/200.0);
    let c=clip(vec3(3.07993494*x-1.53715152*y-0.542783419*z,-0.921234183*x+1.87599*y+0.0452441813*z,0.052889682*x-0.204041338*y+1.15115166*z));
    return vec3(srgb(c.r),srgb(c.g),srgb(c.b));
}
fn sort4(v: vec4<f32>) -> vec4<f32> {
    var a=v.x;var b=v.y;var c=v.z;var d=v.w;
    var t=min(a,b);b=max(a,b);a=t; t=min(c,d);d=max(c,d);c=t;
    t=min(a,c);c=max(a,c);a=t; t=min(b,d);d=max(b,d);b=t;
    t=min(b,c);c=max(b,c);b=t;
    return vec4(a,b,c,d);
}
fn sort5(a0:f32,b0:f32,c0:f32,d0:f32,e0:f32) -> array<f32,5> {
    var a=a0;var b=b0;var c=c0;var d=d0;var e=e0;
    var t=min(a,b);b=max(a,b);a=t; t=min(d,e);e=max(d,e);d=t;
    t=min(c,e);e=max(c,e);c=t; t=min(c,d);d=max(c,d);c=t;
    t=min(a,d);d=max(a,d);a=t; t=min(a,c);c=max(a,c);a=t;
    t=min(b,e);e=max(b,e);b=t; t=min(b,d);d=max(b,d);b=t;
    t=min(b,c);c=max(b,c);b=t;
    return array<f32,5>(a,b,c,d,e);
}
// Exact 25-element register-only median network (zero dynamic array indexing / local memory spills).
fn median5(x:i32,y:i32,c:u32)->f32{
    let w=i32(p[0]);let h=i32(p[1]);
    let x0=u32(clamp(x-2,0,w-1));let x1=u32(clamp(x-1,0,w-1));let x2=u32(x);let x3=u32(clamp(x+1,0,w-1));let x4=u32(clamp(x+2,0,w-1));
    let y0=u32(clamp(y-2,0,h-1))*u32(w);let y1=u32(clamp(y-1,0,h-1))*u32(w);let y2=u32(y)*u32(w);let y3=u32(clamp(y+1,0,h-1))*u32(w);let y4=u32(clamp(y+2,0,h-1))*u32(w);
    let r0=sort5(source[(y0+x0)*3u+c],source[(y0+x1)*3u+c],source[(y0+x2)*3u+c],source[(y0+x3)*3u+c],source[(y0+x4)*3u+c]);
    let r1=sort5(source[(y1+x0)*3u+c],source[(y1+x1)*3u+c],source[(y1+x2)*3u+c],source[(y1+x3)*3u+c],source[(y1+x4)*3u+c]);
    let r2=sort5(source[(y2+x0)*3u+c],source[(y2+x1)*3u+c],source[(y2+x2)*3u+c],source[(y2+x3)*3u+c],source[(y2+x4)*3u+c]);
    let r3=sort5(source[(y3+x0)*3u+c],source[(y3+x1)*3u+c],source[(y3+x2)*3u+c],source[(y3+x3)*3u+c],source[(y3+x4)*3u+c]);
    let r4=sort5(source[(y4+x0)*3u+c],source[(y4+x1)*3u+c],source[(y4+x2)*3u+c],source[(y4+x3)*3u+c],source[(y4+x4)*3u+c]);
    let c0=sort5(r0[0],r1[0],r2[0],r3[0],r4[0]);
    let c1=sort5(r0[1],r1[1],r2[1],r3[1],r4[1]);
    let c2=sort5(r0[2],r1[2],r2[2],r3[2],r4[2]);
    let c3=sort5(r0[3],r1[3],r2[3],r3[3],r4[3]);
    let c4=sort5(r0[4],r1[4],r2[4],r3[4],r4[4]);
    let m=sort5(c4[0],c3[1],c2[2],c1[3],c0[4]);
    let u=sort4(vec4(c3[0],c2[1],c1[2],c0[3]));
    let l=sort4(vec4(c4[1],c3[2],c2[3],c1[4]));
    var a0=min(u.x,l.w);var b0=max(u.x,l.w);var a1=min(u.y,l.z);var b1=max(u.y,l.z);
    var a2=min(u.z,l.y);var b2=max(u.z,l.y);var a3=min(u.w,l.x);var b3=max(u.w,l.x);
    var t=min(a0,a2);a2=max(a0,a2);a0=t; t=min(a1,a3);a3=max(a1,a3);a1=t;
    t=min(a0,a1);a1=max(a0,a1);a0=t; t=min(a2,a3);a3=max(a2,a3);a2=t;
    t=min(b0,b2);b2=max(b0,b2);b0=t; t=min(b1,b3);b3=max(b1,b3);b1=t;
    t=min(b0,b1);b1=max(b0,b1);b0=t; t=min(b2,b3);b3=max(b2,b3);b2=t;
    return max(max(max(a1,min(a2,m[4])),max(min(a3,m[3]),min(b0,m[2]))),max(min(b1,m[1]),min(b2,m[0])));
}
@compute @workgroup_size(16,16,1)
fn main(@builtin(global_invocation_id) id:vec3<u32>){
    let w=u32(p[0]);let h=u32(p[1]);if id.x>=w||id.y>=h{return;}
    let x=i32(id.x);let y=i32(id.y);let index=(id.y*w+id.x)*3u;let stage=detail_stage;let flags=u32(p[3]);
    var a=load_rgb(index);
    switch stage {
        case 0u:{
            let r=i32(p[11]);let stride=max(i32(p[14]),1);let margin=r*stride;let mode=u32(p[31]);
            let row_base=id.y*w;
            if mode==1u{
                var sum=0.0;
                if x>=margin&&x+margin<i32(w){for(var k=-r;k<=r;k++){sum+=source[(row_base+u32(x+k*stride))*3u]*table[u32(k+r)].x;}}
                else{for(var k=-r;k<=r;k++){sum+=source[(row_base+reflected(x+k*stride,i32(w)))*3u]*table[u32(k+r)].x;}}
                output[index]=sum;return;
            }
            if mode==3u{
                let wt=table[0].x;var sum=0.0;
                if x>=margin&&x+margin<i32(w){for(var k=-r;k<=r;k++){sum+=source[(row_base+u32(x+k*stride))*3u]*wt;}}
                else{for(var k=-r;k<=r;k++){sum+=source[(row_base+reflected(x+k*stride,i32(w)))*3u]*wt;}}
                output[index]=sum;return;
            }
            var sum=vec3(0.0);
            if mode==2u{
                let wt=table[0].x;
                if x>=margin&&x+margin<i32(w){for(var k=-r;k<=r;k++){sum+=load_rgb((row_base+u32(x+k*stride))*3u)*wt;}}
                else{for(var k=-r;k<=r;k++){sum+=load_rgb((row_base+reflected(x+k*stride,i32(w)))*3u)*wt;}}
            }else if x>=margin&&x+margin<i32(w){
                for(var k=-r;k<=r;k++){sum+=load_rgb((row_base+u32(x+k*stride))*3u)*table[u32(k+r)].x;}
            }else{
                for(var k=-r;k<=r;k++){sum+=load_rgb((row_base+reflected(x+k*stride,i32(w)))*3u)*table[u32(k+r)].x;}
            }
            a=sum;
        }
        case 1u:{
            let r=i32(p[11]);let stride=max(i32(p[14]),1);let margin=r*stride;let mode=u32(p[31]);
            if mode==1u{
                var sum=0.0;
                if y>=margin&&y+margin<i32(h){for(var k=-r;k<=r;k++){sum+=source[(u32(y+k*stride)*w+id.x)*3u]*table[u32(k+r)].x;}}
                else{for(var k=-r;k<=r;k++){sum+=source[(reflected(y+k*stride,i32(h))*w+id.x)*3u]*table[u32(k+r)].x;}}
                output[index]=sum;return;
            }
            if mode==3u{
                let wt=table[0].x;var sum=0.0;
                if y>=margin&&y+margin<i32(h){for(var k=-r;k<=r;k++){sum+=source[(u32(y+k*stride)*w+id.x)*3u]*wt;}}
                else{for(var k=-r;k<=r;k++){sum+=source[(reflected(y+k*stride,i32(h))*w+id.x)*3u]*wt;}}
                output[index]=sum;return;
            }
            var sum=vec3(0.0);
            if mode==2u{
                let wt=table[0].x;
                if y>=margin&&y+margin<i32(h){for(var k=-r;k<=r;k++){sum+=load_rgb((u32(y+k*stride)*w+id.x)*3u)*wt;}}
                else{for(var k=-r;k<=r;k++){sum+=load_rgb((reflected(y+k*stride,i32(h))*w+id.x)*3u)*wt;}}
            }else if y>=margin&&y+margin<i32(h){
                for(var k=-r;k<=r;k++){sum+=load_rgb((u32(y+k*stride)*w+id.x)*3u)*table[u32(k+r)].x;}
            }else{
                for(var k=-r;k<=r;k++){sum+=load_rgb((reflected(y+k*stride,i32(h))*w+id.x)*3u)*table[u32(k+r)].x;}
            }
            a=sum;
        }
        case 2u:{
            if (flags&1u)!=0u{a+=(a-from_aux(index))*p[4]/100.0;}
            if (flags&16u)==0u{a=clip(a);if (flags&2u)!=0u{a=vec3(luma(a));}}
        }
        case 3u:{a+=(a-from_aux(index))*p[5]/140.0;}
        case 4u:{
            var residual=a-from_aux(index);if (flags&64u)!=0u{residual=vec3(luma(residual));}
            if (flags&4u)!=0u{residual*=clamp(abs(residual)/p[7],vec3(0.0),vec3(1.0));}
            if (flags&8u)!=0u{
                let dx=(gray(x+1,y-1)-gray(x-1,y-1))+2.0*(gray(x+1,y)-gray(x-1,y))+(gray(x+1,y+1)-gray(x-1,y+1));
                let dy=(gray(x-1,y+1)-gray(x-1,y-1))+2.0*(gray(x,y+1)-gray(x,y-1))+(gray(x+1,y+1)-gray(x+1,y-1));
                residual*=clamp(sqrt(dx*dx+dy*dy)*p[9]/p[8],0.0,1.0);
            }
            a+=residual*p[6]/65.0;
        }
        case 5u:{
            var u=select(-1.0,-1.0+2.0*f32(x)/f32(max(w,2u)-1u),w>1u);var v=select(-1.0,-1.0+2.0*f32(y)/f32(max(h,2u)-1u),h>1u);
            if p[19]!=0.0{u*=1.0+p[19]*(p[24]-1.0);v*=1.0+p[19]*(p[25]-1.0);}
            var d2=u*u+v*v;if p[20]!=2.0{d2=pow(pow(abs(u),p[20])+pow(abs(v),p[20]),2.0/p[20]);}
            d2=clamp(d2*p[21],0.0,2.0);if p[22]!=1.0{d2=2.0*pow(d2/2.0,p[22]);}
            var darken=d2*p[10];if p[23]!=0.0{let l=clamp((luma(a)-0.5)/0.5,0.0,1.0);darken*=1.0-p[23]*l*l;}
            a*=1.0-darken;
        }
        case 6u:{a=rgb_lab(a);}
        case 7u:{
            let center=a.x;var sum=0.0;var weights=0.0;
            for(var k=0u;k<u32(p[30]);k++){
                let o=table[u32(p[29])+2u*k];let space=table[u32(p[29])+2u*k+1u].x;
                let value=at(x+i32(o.y),y+i32(o.x)).x;let alpha=abs(value-center)*p[28];let idx=u32(floor(alpha));let f=alpha-f32(idx);
                let weight=space*(table[idx].x+f*(table[idx+1u].x-table[idx].x));sum+=value*weight;weights+=weight;
            }
            a.x=center*(1.0-p[13])+(sum/weights)*p[13];
        }
        case 8u:{a=lab_rgb(a);}
        case 10u:{a=vec3(min(min(a.r,a.g),a.b));}
        case 11u,12u:{
            var m=a.x;let r=i32(p[18]);
            for(var o=-r;o<=r;o++){let sx=select(x,x+o,stage==11u);let sy=select(y,y+o,stage==12u);if sx>=0&&sy>=0&&sx<i32(w)&&sy<i32(h){m=min(m,source[u32((sy*i32(w)+sx)*3)]);}}
            a=vec3(m);
        }
        case 13u:{let transmission=clamp(1.0-p[15]*aux[index],0.2,1.8);a=(a-1.0)/transmission+1.0;}
        case 14u:{
            let purple=(a.r+a.b)/2.0-a.g;
            let lap=(channel_max(x-1,y)+channel_max(x+1,y)+channel_max(x,y-1)+channel_max(x,y+1))-4.0*channel_max(x,y);
            let edge=abs(lap)*p[17];let weight=clamp(purple*6.0,0.0,1.0)*clamp(edge*3.0,0.0,1.0)*p[16]/100.0;
            let mean=(a.r+a.g+a.b)/3.0;a=clip(a*(1.0-weight)+mean*weight);if (flags&2u)!=0u{a=vec3(luma(a));}
        }
        case 17u:{a=vec3(a.x/100.0,median5(x,y,1u)/100.0,median5(x,y,2u)/100.0);}
        case 18u:{a=a.x*a;}
        case 19u:{a=vec3(a.y*a.y,a.y*a.z,a.z*a.z);}
        case 20u:{
            let m=a;let b=from_aux(index);let c=from_e0(index);
            let a11=b.x-m.x*m.x+p[7];let a12=b.y-m.x*m.y;let a13=b.z-m.x*m.z;
            let a22=c.x-m.y*m.y+p[8];let a23=c.y-m.y*m.z;let a33=c.z-m.z*m.z+p[8];
            let i11=a22*a33-a23*a23;let i12=a13*a23-a12*a33;let i13=a12*a23-a13*a22;
            let i22=a11*a33-a13*a13;let i23=a13*a12-a11*a23;let i33=a11*a22-a12*a12;let det=a11*i11+a12*i12+a13*i13;
            a=select(vec3(i22,i23,i33),vec3(i11,i12,i13),p[12]==0.0)/det;
        }
        case 21u:{let g=from_aux(index);let value=a[u32(p[12])];a=vec3(value,g.x*value,g.y*value);}
        case 22u:{a=vec3(from_aux(index).z*a[u32(p[12])],0.0,0.0);}
        case 23u:{
            let m=a;let d=from_aux(index);let i1=from_e1(index);let i2=from_e2(index);
            let c0=d.y-m.x*d.x;let c1=d.z-m.y*d.x;let c2=extra0[index]-m.z*d.x;
            a=vec3(i1.x*c0+i1.y*c1+i1.z*c2,i1.y*c0+i2.x*c1+i2.y*c2,i1.z*c0+i2.y*c1+i2.z*c2);
        }
        case 24u:{let k=from_aux(index);a=vec3(extra0[index]-(k.x*a.x+k.y*a.y+k.z*a.z),0.0,0.0);}
        case 25u:{let g=from_aux(index);let k=from_e0(index);let value=a[u32(p[12])];let q=k.x*g.x+k.y*g.y+k.z*g.z+extra1[index];let blended=value+(q-value)*p[13];if p[12]==1.0{a.y=blended;}else{a.z=blended;}}
        case 26u:{a=vec3(a.x,0.0,0.0);}
        case 29u:{let sx=aux[index];let d=a.x-sx;let guide=select(a.z,sx,p[27]==0.0);a=vec3(d*d,d,guide);}
        case 30u:{
            let sx=aux[index];let d=a.x-sx;let energy=extra0[index];let guide=select(a.z,sx,p[27]==0.0);
            let t=guide/12.5-0.5;var sigma=table[0].x;
            if t>=7.0{sigma=table[7].x;}else if t>0.0{let j=u32(t);let f=t-f32(j);sigma=table[j].x+(table[j+1u].x-table[j].x)*f;}
            sigma*=1.25;let gain=energy/(energy+p[26]*sigma*sigma+1e-12);a=vec3(sx,a.y+d*gain,guide);
        }
        case 31u:{let b=from_aux(index);a.x=b.x+b.y;}
        default:{}
    }
    store_rgb(index,a);
}
