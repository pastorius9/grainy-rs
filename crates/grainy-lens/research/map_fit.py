"""Compare independent normalization/evaluation candidates with native maps."""
from probe import *
import json,re,itertools
table=np.array([int(x,16) for x in re.findall('0x[0-9a-f]+',(Path(__file__).parents[1]/'src'/'sqrt_table.rs').read_text())],np.uint32).view(np.float32)
def root(x):
    bits=np.float32(x).view(np.uint32);exp=int(bits>>23)-127
    idx=(int(bits&0x7fffff)|((exp%2)<<23))>>12
    return np.float32(float(table[idx])*2**(exp//2))
for name in ['m0_focal1','m5_sensor0']:
    j=json.loads((Path(__file__).parents[3]/'golden'/'lens'/name/'case.json').read_text(encoding='utf-8'));p=j['settings']['lensfun'];_,lens=optics._saved_lens(p['xml']);w=j['width'];h=j['height']
    cal=lens.interpolate_distortion(p['focal']);a,b,c=np.array(cal.terms,np.float32);d=np.float32(1-(a+b+c))
    modifier=optics._modifier(j['settings'],(h,w,3));actual=modifier.apply_geometry_distortion()
    lo=.8;hi=1.2;xu=(w-1)/2+w*.3;yu=(h-1)/2
    target=float(modifier.apply_geometry_distortion(xu=xu,yu=yu,width=1,height=1)[0,0,0])
    def explicit(s):
        m=lf.Modifier(lens,p['crop'],w,h);m.initialize(p['focal'],p['aperture'],p['distance'],scale=s,pixel_format=np.float32,flags=41)
        return m
    for _ in range(40):
        mid=(hi+lo)/2;v=float(explicit(mid).apply_geometry_distortion(xu=xu,yu=yu,width=1,height=1)[0,0,0]);
        if v>target:lo=mid
        else:hi=mid
    base=np.float32((hi+lo)/2).view(np.uint32)
    for sbits in range(int(base)-3,int(base)+5):
        s=np.uint32(sbits).view(np.float32);err=np.max(abs(explicit(float(s)).apply_geometry_distortion()-actual))
        if err==0:break
    print(name,'SCALE',s,'exact scale diff',err)
    best=[]
    for crop32,xf32,yf32,recip in itertools.product([1,2,3,4,5],[True],[True],[1]):
        crop=float(np.float32(p['crop'])) if crop32 else p['crop'];aspect=float(np.float32(1.5))
        ratio=float(np.float32(lens.crop_factor/crop)) if crop32==2 else lens.crop_factor/crop
        halfdiag=float(np.sqrt(np.float32(aspect*aspect+1))) if crop32>=3 else np.sqrt(aspect*aspect+1)
        if crop32==4:ratio=float(np.float32(lens.crop_factor/crop))
        if crop32==5:ratio=float(np.float32(1/crop))*lens.crop_factor
        norm=2*halfdiag*ratio/np.hypot(w-1,h-1);cx=(w-1)*norm/2;cy=(h-1)*norm/2
        xx=np.float32(-cx);yy=np.float32(-cy);xs=[];ys=[]
        for x in range(w):
            xs.append(np.float32(xx));xx=np.float32(xx+np.float32(norm)) if xf32 else float(xx)+norm
        for y in range(h):
            ys.append(np.float32(yy));yy=np.float32(yy+np.float32(norm)) if yf32 else float(yy)+norm
        out=np.zeros_like(actual)
        for y in range(h):
            pts=np.array([[x,ys[y]] for x in xs],np.float32)
            pts=(pts.astype(np.float64)*(1/float(s))).astype(np.float32) if recip==2 else ((pts.astype(np.float64)/float(s)).astype(np.float32) if recip==3 else pts*np.float32(1/s) if recip else pts/s)
            for x in range(w):
                if (y*w)%2==0 and x<w//4*4:
                    px=pts[x//4*4+(x%2)*2];py=pts[x//4*4+(x%2)*2+1]
                    r2x=np.float32(px[0]*px[0]+px[1]*px[1]);r2y=np.float32(py[0]*py[0]+py[1]*py[1]);rx=root(r2x);ry=root(r2y)
                    fx=np.float32(a*r2x*rx+b*r2x+c*rx+d);fy=np.float32(a*r2y*ry+b*r2y+c*ry+d)
                else:
                    r2=np.float32(pts[x,0]**2+pts[x,1]**2);r=np.sqrt(r2);fx=fy=np.float32(a*r2*r+b*r2+c*r+d)
                out[y,x,0]=(float(np.float32(pts[x,0]*fx))+cx)/norm;out[y,x,1]=(float(np.float32(pts[x,1]*fy))+cy)/norm
        if crop32 and xf32 and yf32 and recip:
            y,x,k=np.unravel_index(np.argmax(abs(out-actual)),out.shape)
            pindex=x//4*4+(x%2)*2+k
            px=np.float32(xs[pindex]*np.float32(1/s));py=np.float32(ys[y]*np.float32(1/s));rr=np.float32(px*px+py*py)
            db2,ll=synthetic('<distortion model="ptlens" focal="50" c="1"/>',crop=lens.crop_factor)
            mm=lf.Modifier(ll,p['crop'],w,h);mm.initialize(50,4,scale=float(s),pixel_format=np.float32,flags=40)
            gg=mm.apply_geometry_distortion()
            orig=np.float32(xs[x]*np.float32(1/s)) if k==0 else np.float32(ys[y]*np.float32(1/s))
            est=((float(gg[y,x,k])-(w-1)/2 if k==0 else float(gg[y,x,k])-(h-1)/2)*norm)/float(orig)
            print('worst details',x,y,k,'r2',rr,'root LUT',root(rr),'measured sqrt actual',est,'expected pixel',actual[y,x,k],'computed',out[y,x,k])
        best.append((float(np.max(abs(out-actual))),(crop32,xf32,yf32,recip)))
    print(sorted(best))
