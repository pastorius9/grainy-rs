"""Fit numerical auto-scale rules using public Modifier output only."""
from probe import *
import itertools

def fac(c,x,y,order=0):
    t=np.array(c.terms,np.float32); r2=np.float32(x*x+y*y)
    if c.model==lf.DistortionModel.POLY3:
        return np.float32(np.float32(1-t[0])+t[0]*r2)
    if c.model==lf.DistortionModel.POLY5:
        return np.float32(1+float(t[0]*r2)+float(t[1]*r2*r2))
    r=np.sqrt(r2)
    if order==0:return np.float32(t[0]*r2*r+t[1]*r2+t[2]*r+np.float32(1-(t[0]+t[1]+t[2])))
    return np.float32(np.float32(1-(t[0]+t[1]+t[2]))+t[0]*r2*r+t[1]*r2+t[2]*r)

data=[]
for model in ['Canon EF 70-200mm f/2.8L IS USM','Canon PowerShot G12 & compatibles (Standard)','E 24mm f/1.8 ZA','Nikon AF-S DX Nikkor 35mm f/1.8G']:
    rec=next(r for r in optics.lens_records() if r['model']==model and (not model.startswith('Nikon') or r['crop']>1));lens=optics.profile_lens(rec)
    ar=ET.fromstring(rec['xml']).findtext('./lens/aspect-ratio','1.5');aspect=float(np.float32(float(ar) if ':' not in ar else float(ar.split(':')[0])/float(ar.split(':')[1])))
    for w,h,cr in [(96,64,1),(301,200,1.1),(1450,1000,1),(96,64,.8),(120,60,1.5),(100,100,1)]:
        crop=float(np.float32(rec['crop']*cr));focal=lens.max_focal if model.startswith('Canon EF 70') else lens.min_focal;cal=lens.interpolate_distortion(focal)
        norm=2*np.sqrt(aspect*aspect+1)*(lens.crop_factor/crop)/np.hypot(w-1,h-1)
        cx=(w-1)/2*norm;cy=(h-1)/2*norm
        m=lf.Modifier(lens,crop,w,h);m.initialize(focal,4,scale=0,pixel_format=np.float32,flags=int(lf.ModifyFlags.DISTORTION|lf.ModifyFlags.SCALE))
        # f32 pixel positions supplied to lensfunpy.
        xu=np.float32((w-1)/2+w*.2);offset=(float(xu)-(w-1)/2)*norm
        target=float(m.apply_geometry_distortion(xu=xu,yu=(h-1)/2,width=1,height=1)[0,0,0])
        data.append((model[:10],w,h,cr,cal,cx,cy,norm,offset,target))

def solve(c,cx,cy,epsilon,mode,ftype,rtype,order,init,angle):
    scale=0.
    for xx,yy in [(cx,0),(0,cy),(cx,cy)]:
        if init:xx=float(np.float32(xx));yy=float(np.float32(yy))
        radius=np.hypot(xx,yy)
        if init==2:radius=float(np.sqrt(np.float32(np.float32(xx)*np.float32(xx)+np.float32(yy)*np.float32(yy))))
        if init==3:radius=float(np.sqrt(float(np.float32(np.float32(xx)*np.float32(xx)+np.float32(yy)*np.float32(yy)))))
        ux=xx/radius;uy=yy/radius
        if ftype==5:
            radius=max(abs(xx),abs(yy));ux=xx/radius;uy=yy/radius
        if angle==1:ux=float(np.float32(ux));uy=float(np.float32(uy))
        if angle==2:
            theta=np.float32(np.arctan2(yy,xx));ux=float(np.cos(theta));uy=float(np.sin(theta))
        if angle==3:
            theta=np.arctan2(yy,xx);ux=np.cos(theta);uy=np.sin(theta)
        if angle==4:
            theta=np.arctan2(yy,xx);ux=float(np.float32(np.cos(theta)));uy=float(np.float32(np.sin(theta)))
        if angle==5:
            theta=float(np.float32(np.arctan2(yy,xx)));ux=float(np.float32(np.cos(theta)));uy=float(np.float32(np.sin(theta)))
        r=radius
        def f(r):
            x=np.float32(ux*r);y=np.float32(uy*r);k=fac(c,x,y,order)
            if ftype==0:return float(k)*r
            if ftype==1:return float(np.float32(k*np.float32(r)))
            x=np.float32(x*k);y=np.float32(y*k)
            if ftype==2:return np.hypot(float(x),float(y))
            if ftype==3:return float(np.sqrt(np.float32(x*x+y*y)))
            if ftype==6:return float(y)/uy if uy!=0 and (ux==0 or abs(uy)<abs(ux)) else float(x)/ux
            if ftype==7:return max(float(x)/ux if ux else 0,float(y)/uy if uy else 0)
            if ftype==8:return min(float(x)/ux if ux else float('inf'),float(y)/uy if uy else float('inf'))
            return float(x)/ux if abs(ux)>abs(uy) else float(y)/uy
        for _ in range(10):
            fr=f(r);res=fr-radius
            if abs(res)<.001:break
            e=epsilon
            if mode==0:d=(f(r+e)-f(r-e))/(2*e)
            elif mode==1:d=(f(r+e)-fr)/e
            else:d=(fr-f(r-e))/e
            if d==0:break
            r-=res/d
            if rtype:r=float(np.float32(r))
        scale=max(scale,radius/r)
    return np.float32(scale*1.001)

best=[]
for args in itertools.product([.0001],[0,1,2],[0,1,2,3,4],[0,1],[0],[0,1,2,3],[0,1,2,3,4,5]):
    errors=[]
    for _,w,h,cr,c,cx,cy,norm,offset,target in data:
        scale=solve(c,cx,cy,*args)
        x=np.float32(np.float32(offset)*np.float32(1/scale));f=fac(c,x,np.float32(0))
        pred=(float(np.float32(x*f))+cx)/norm
        errors.append(abs(pred-target))
    best.append((max(errors),sum(errors),args,errors))
for worst,total,args,errors in sorted(best)[:6]:print(worst,total,args,errors)
for d in data:
    model,w,h,cr,c,cx,cy,norm,offset,target=d
    scale=solve(c,cx,cy,.0001,1,4,0,0,1,2)
    x=np.float32(np.float32(offset)*np.float32(1/scale));f=fac(c,x,np.float32(0));pred=(float(np.float32(x*f))+cx)/norm
    print('details',model,w,h,cr,'scale',scale,'diff',pred-target)
