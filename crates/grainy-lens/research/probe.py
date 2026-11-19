"""Black-box experiments only. Never inspect lensfun implementation source."""
import sys
sys.dont_write_bytecode = True
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[3] / 'tools' / 'golden'))
from common import np
import lensfunpy as lf
from luma import optics
import xml.etree.ElementTree as ET


def synthetic(calibration, crop=1, aspect='1.5', model='Test 10-100mm f/2'):
    xml = f'''<lensdatabase version="1"><mount><name>Test</name></mount><lens>
      <maker>Test</maker><model>{model}</model><mount>Test</mount><cropfactor>{crop}</cropfactor>
      <aspect-ratio>{aspect}</aspect-ratio><calibration>{calibration}</calibration>
      </lens></lensdatabase>'''
    db = lf.Database(xml=xml, load_common=False, load_bundled=False)
    return db, db.lenses[0]


def fields(o):
    if o is None:
        return None
    # Only public data attributes of returned black-box records, no implementation inspection.
    return {name: getattr(o, name) for name in ('model', 'focal', 'aperture', 'distance', 'terms') if hasattr(o, name)}


def inventory():
    print('version', lf.lensfun_version)
    root = Path(lf.__file__).parent / 'db_files'
    selected = {}
    for path in sorted(root.glob('*.xml')):
        for lens in ET.parse(path).getroot().findall('lens'):
            ds = {x.get('model') for x in lens.findall('./calibration/distortion')}
            ts = {x.get('model') for x in lens.findall('./calibration/tca')}
            vs = bool(lens.findall('./calibration/vignetting'))
            key = (tuple(sorted(ds)), tuple(sorted(ts)), vs)
            selected.setdefault(key, (path.name, lens.findtext('model'), lens.findtext('cropfactor')))
    for k, v in selected.items():
        print(k, v)


def probe():
    for model, attrs in [('poly3', 'k1="0.1"'), ('poly5', 'k1="0.1" k2="0.03"'), ('ptlens', 'a="0.01" b="0.02" c="0.03"')]:
        db, lens = synthetic(f'<distortion model="{model}" focal="50" {attrs}/>')
        print(model, fields(lens.interpolate_distortion(50)))
        for w,h,crop in [(96,64,1),(301,200,1),(96,64,1.5),(100,100,1),(64,96,1)]:
            for scale in [1,0]:
                m = lf.Modifier(lens,crop,w,h)
                flags = m.initialize(50, 4, 1000, scale=scale, pixel_format=np.float32, flags=int(lf.ModifyFlags.DISTORTION|lf.ModifyFlags.SCALE))
                a = m.apply_geometry_distortion()
                print((w,h,crop,scale,flags), 'samples',a[0,0],a[h//2,0],a[0,w//2],a[h//2,w//2])
    for model, attrs in [('linear','kr="1.01" kb="0.99"'),('poly3','vr="1.01" vb="0.99" cr="0.02" cb="-0.01" br="0.03" bb="-0.02"')]:
        db,lens=synthetic(f'<tca model="{model}" focal="50" {attrs}/>')
        print('tca',model,fields(lens.interpolate_tca(50)))
        m=lf.Modifier(lens,1,96,64)
        print('flags',m.initialize(50,4,1000,scale=1,pixel_format=np.float32,flags=int(lf.ModifyFlags.TCA)))
        print('maps',m.apply_geometry_distortion(),m.apply_subpixel_geometry_distortion()[0,:2])
    db,lens=synthetic('<vignetting model="pa" focal="50" aperture="4" distance="1000" k1="-0.1" k2="0.03" k3="-0.02"/>')
    for w,h,crop in [(96,64,1),(301,200,1),(96,64,1.5),(100,100,1),(64,96,1)]:
        m=lf.Modifier(lens,crop,w,h)
        m.initialize(50,4,1000,pixel_format=np.float32,flags=int(lf.ModifyFlags.VIGNETTING))
        a=np.ones((h,w,3),np.float32); m.apply_color_modification(a)
        print('vig',w,h,crop, a[0,0,0],a[h//2,0,0],a[0,w//2,0],a[h//2,w//2,0])


def interpolation():
    for model,attribute in [('poly3','k1'),('poly5','k1'),('ptlens','a')]:
        for focals,terms in [([20,40],[.1,.2]),([10,20,40,80],[.01,.02,.08,.03])]:
            db,lens=synthetic(''.join(f'<distortion model="{model}" focal="{f}" {attribute}="{v}"/>' for f,v in zip(focals,terms)))
            for f in [5,10,15,20,25,30,35,40,50,60,80,100]:
                print('interp',model,focals,f, fields(lens.interpolate_distortion(f)))
    for axis,values in [('focal',[20,40]),('aperture',[2,8]),('distance',[1,1000])]:
        c=[]
        for value,k in zip(values,[-.1,-.5]):
            d=dict(focal=30,aperture=4,distance=10);d[axis]=value
            c.append(f'<vignetting model="pa" focal="{d["focal"]}" aperture="{d["aperture"]}" distance="{d["distance"]}" k1="{k}"/>')
        db,lens=synthetic(''.join(c))
        for v in sorted(set([values[0]*.1,values[0],sum(values)/2,values[1],values[1]*2,3,4,6,10,30,100])):
            d=dict(focal=30,aperture=4,distance=10);d[axis]=v
            print('viginterp',axis,v,fields(lens.interpolate_vignetting(**d)))


def normalization():
    for model,attrs in [('poly3','k1="0.1"'),('poly5','k1="0.1" k2="0.03"'),('ptlens','a="0.01" b="0.02" c="0.03"')]:
        for aspect in ['1.5','1','2','4:3']:
            db,lens=synthetic(f'<distortion model="{model}" focal="50" {attrs}/>',aspect=aspect)
            w,h=96,64
            m=lf.Modifier(lens,1,w,h);m.initialize(50,4,scale=1,pixel_format=np.float32,flags=int(lf.ModifyFlags.DISTORTION))
            a=m.apply_geometry_distortion()
            print('norm',model,aspect,'corners',a[0,0],a[0,-1],a[-1,0],a[-1,-1])
            # Fit channel mapping radial factor in pixel radius and expose asymmetries.
            for x,y in [(0,0),(1,0),(2,0),(3,0),(4,0),(0,31),(47,0),(47,31),(48,32),(95,63)]:
                dx,dy=x-47.5,y-31.5
                print(x,y,'fac',(a[y,x]-[47.5,31.5])/[dx,dy])


def weights():
    for axis in ['focal','aperture','distance']:
        c=[]
        for value,k in zip([2,8],[-.1,-.5]):
            d=dict(focal=4,aperture=4,distance=4);d[axis]=value
            c.append(f'<vignetting model="pa" focal="{d["focal"]}" aperture="{d["aperture"]}" distance="{d["distance"]}" k1="{k}"/>')
        db,lens=synthetic(''.join(c))
        for v in [0.5,1,1.5,2,2.5,3,3.2,4,5,6,8,10,16,100]:
            d=dict(focal=4,aperture=4,distance=4);d[axis]=v
            r=lens.interpolate_vignetting(**d)
            t=(r.terms[0]+.1)/-.4 if r else None
            print(axis,v,t)
    # A basis in coefficient*focal confirms the polynomial interpolation independently.
    for i in range(4):
        fs=[10,20,40,80]
        db,lens=synthetic(''.join(f'<distortion model="poly3" focal="{f}" k1="{1/f if i==j else 0}"/>' for j,f in enumerate(fs)))
        print('basis',i,[(f,lens.interpolate_distortion(f).terms[0]*f) for f in [15,25,30,35,50,60]])


def combined():
    db,lens=synthetic('<distortion model="poly3" focal="50" k1="0.1"/><tca model="linear" focal="50" kr="1.01" kb="0.99"/>')
    for w in [1,2,3,4,5,6,7,8,9,96,97]:
        m=lf.Modifier(lens,1,w,64); m.initialize(50,4,scale=1,pixel_format=np.float32,flags=int(lf.ModifyFlags.DISTORTION|lf.ModifyFlags.TCA))
        g=m.apply_geometry_distortion(); s=m.apply_subpixel_geometry_distortion()
        print('combined',w, 'green-geo',np.max(np.abs(g-s[:,:,1,:])), 'top',g[0,:min(9,w)].tolist())
    for model,attrs in [('linear','kr'),('poly3','vr'),('poly3','br'),('poly3','cr')]:
        db,lens=synthetic(''.join(f'<tca model="{model}" focal="{f}" {attrs}="{k}"/>' for f,k in [(10,.1),(20,.2),(40,.8),(80,.3)]))
        print('tcainterp',model,attrs, [(f,fields(lens.interpolate_tca(f))) for f in [15,30,60]])


def vigmetric():
    cases=[[(20,4,10),(40,8,10)],[(20,4,10),(40,4,1000)],[(20,2,1),(80,8,1000)]]
    for nodes in cases:
        db,lens=synthetic(''.join(f'<vignetting model="pa" focal="{f}" aperture="{a}" distance="{d}" k1="{k}"/>' for (f,a,d),k in zip(nodes,[-.1,-.5])))
        for f,a,d in [(30,4,10),(20,8,10),(40,4,10),(20,4,1000),(40,4,1000),(50,4,10),(30,2,1)]:
            r=lens.interpolate_vignetting(f,a,d)
            print(nodes,(f,a,d),fields(r))


def autoscale():
    for model,attrs in [('poly3','k1="0.1"'),('poly3','k1="-0.1"'),('poly5','k1="0.1" k2="0.03"'),('poly5','k1="-0.1"'),('ptlens','a="0.01" b="0.02" c="0.03"'),('ptlens','a="-0.01" b="-0.02" c="-0.03"'),('none','')]:
        for tca in ['', '<tca model="linear" focal="50" kr="1.01" kb="0.99"/>']:
            cal=f'<distortion model="{model}" focal="50" {attrs}/>' if model!='none' else ''
            db,lens=synthetic(cal+tca)
            for w,h in [(96,64),(301,200)]:
                m=lf.Modifier(lens,1,w,h);m.initialize(50,4,scale=0,pixel_format=np.float32,flags=int(lf.ModifyFlags.DISTORTION|lf.ModifyFlags.TCA|lf.ModifyFlags.SCALE))
                # A tiny offset isolates scaling from cubic radial changes.
                a=m.apply_subpixel_geometry_distortion(xu=(w-1)/2+.1,yu=(h-1)/2,width=1,height=1)
                g=m.apply_geometry_distortion(xu=0,yu=0,width=1,height=1)
                print('auto',model,attrs,bool(tca),w,h,None if a is None else a[0,0].tolist(), None if g is None else g[0,0].tolist())


def records():
    models=['Nikon AF-S DX Nikkor 35mm f/1.8G','Canon EF 70-200mm f/2.8L IS USM','Canon PowerShot G12 & compatibles (Standard)','Canon EF-M 22mm f/2 STM','Canon PowerShot G3 & compatibles (Standard)','E 24mm f/1.8 ZA','Schneider 80mm Xenotar f/2.8']
    for record in optics.lens_records():
        if record['model'] in models:
            lens=optics.profile_lens(record)
            print({k:v for k,v in record.items() if k!='xml'})
            print('bounds',lens.min_focal,lens.max_focal,lens.min_aperture,lens.max_aperture)
            print(record['xml'][:1200])
    for camera in optics.cameras():
        if camera.model in ['D7000','EOS 5D Mark II','EOS M','NEX-5','PowerShot G12','PowerShot G3']:
            print('camera',camera.maker,camera.model,camera.variant,camera.mount,camera.crop_factor)


def compatibility():
    for crop in [1.04,1.041,1.04166,1.04167,1.045,1.049,1.05]:
        xml=f'''<lensdatabase version="1"><mount><name>A</name><compat>B</compat></mount><mount><name>B</name><compat>C</compat></mount><mount><name>C</name></mount>
        <camera><maker>Test</maker><model>Camera</model><mount>A</mount><cropfactor>1</cropfactor></camera>'''
        for mount in ['A','B','C']:
            xml+=f'<lens><maker>Test</maker><model>Lens {mount} 50mm f/2</model><mount>{mount}</mount><cropfactor>{crop}</cropfactor></lens>'
        xml+='</lensdatabase>'
        db=lf.Database(xml=xml,load_common=False,load_bundled=False)
        print(crop,[l.model for l in db.find_lenses(db.cameras[0])])


def strips():
    db,lens=synthetic('<distortion model="poly3" focal="50" k1="0.1"/>')
    for w in [96,97,301]:
        m=lf.Modifier(lens,1,w,64);m.initialize(50,4,scale=1,pixel_format=np.float32,flags=int(lf.ModifyFlags.DISTORTION))
        full=m.apply_geometry_distortion();one=m.apply_geometry_distortion(width=w,height=1)
        print('strips',w,'diff',np.max(abs(full[0]-one[0])), 'fulltail',full[0,-4:], 'onetail',one[0,-4:])
        two=m.apply_geometry_distortion(yu=1,width=w,height=1)
        print('nextrow',np.max(abs(full[1]-two[0])),full[1,:4],two[0,:4])


def scales():
    def factor(cal,r):
        t=cal.terms
        return (1-t[0]+t[0]*r*r if cal.model==lf.DistortionModel.POLY3 else
                1+t[0]*r*r+t[1]*r**4 if cal.model==lf.DistortionModel.POLY5 else
                ((t[0]*r+t[1])*r+t[2])*r+1-t[0]-t[1]-t[2])
    def root(cal,r,stop=0):
        x=r
        for _ in range(20):
            e=1e-5;f=x*factor(cal,x)-r;d=((x+e)*factor(cal,x+e)-(x-e)*factor(cal,x-e))/(2*e)
            if abs(f)<stop:break
            x-=f/d
        return x
    rs=optics.lens_records()
    for model in ['Canon EF 70-200mm f/2.8L IS USM','Canon PowerShot G12 & compatibles (Standard)','E 24mm f/1.8 ZA']:
        rec=next(r for r in rs if r['model']==model);lens=optics.profile_lens(rec)
        aspect=float(ET.fromstring(rec['xml']).findtext('./lens/aspect-ratio','1.5').replace(':','/').split('/')[0])
        ar=ET.fromstring(rec['xml']).findtext('./lens/aspect-ratio','1.5');aspect=float(ar) if ':' not in ar else float(ar.split(':')[0])/float(ar.split(':')[1])
        for w,h,cr in [(96,64,1),(301,200,1.1),(1450,1000,1),(96,64,.8),(120,60,1.5),(100,100,1)]:
            crop=rec['crop']*cr;focal=lens.max_focal if model.startswith('Canon EF 70') else lens.min_focal;cal=lens.interpolate_distortion(focal)
            m=lf.Modifier(lens,crop,w,h);m.initialize(focal,4,scale=0,pixel_format=np.float32,flags=int(lf.ModifyFlags.DISTORTION|lf.ModifyFlags.SCALE))
            offset=w*.2
            a=m.apply_geometry_distortion(xu=(w-1)/2+offset,yu=(h-1)/2,width=1,height=1)
            norm=2*np.sqrt(aspect*aspect+1)*(lens.crop_factor/crop)/np.hypot(w-1,h-1)
            rd=(float(a[0,0,0])-(w-1)/2)*norm
            measured=offset*norm/root(cal,rd)
            candidates=[]
            for ar in [(w-1)/(h-1)]:
                short=np.sqrt(aspect*aspect+1)*(lens.crop_factor/crop)/np.sqrt(ar*ar+1)
                candidates.append(max(r/root(cal,r) for r in [short,short*ar,short*np.sqrt(ar*ar+1)])*1.001)
                candidates.append(max(r/root(cal,r,0.001) for r in [short,short*ar,short*np.sqrt(ar*ar+1)])*1.001)
            print('scale',model[:20],w,h,cr,'oracle',measured,'exact / Newton .001',candidates)


def sqrt_probe():
    # x=1, y variable and a unit coefficient isolates the callback's radius estimate.
    # A square 3x3 sensor with aspect 1 makes the normalization exactly 1.
    db,lens=synthetic('<distortion model="ptlens" focal="50" c="1"/>',aspect='1')
    m=lf.Modifier(lens,1,3,3);m.initialize(50,4,scale=1,pixel_format=np.float32,flags=int(lf.ModifyFlags.DISTORTION))
    for r2 in [1,1.001,1.1,1.5,2,2.1,3,3.25,4]:
        y=np.sqrt(r2-1)
        g=m.apply_geometry_distortion(xu=2,yu=1+y,width=4,height=1)
        got=g[0,0,0]-1
        f=np.float32(np.sqrt(np.float32(r2)))
        print('sqrt',r2,'measured',got,'sqrt',f,'bits',hex(got.view(np.uint32)),hex(f.view(np.uint32)))


def sqrt_table():
    db,lens=synthetic('<distortion model="ptlens" focal="50" c="1"/>',aspect='1')
    m=lf.Modifier(lens,1,3,3);m.initialize(50,4,scale=1,pixel_format=np.float32,flags=int(lf.ModifyFlags.DISTORTION))
    def query(r2):
        g=m.apply_geometry_distortion(xu=2,yu=1+np.sqrt(r2-1),width=4,height=1)
        return np.float32(g[0,0,0]-1)
    table=[]
    for i in range(4096):
        bits=np.uint32(0x3f800000+(i<<12)+2048)
        table.append(query(bits.view(np.float32)))
    errors=[]
    rng=np.random.default_rng(3)
    for _ in range(10000):
        r2=np.float32(rng.uniform(1,4));idx=(int(r2.view(np.uint32))-0x3f800000)>>12
        actual=query(r2)
        if actual!=table[idx]:errors.append((float(r2),float(actual),float(table[idx])))
    print('table errors',len(errors),errors[:20])
    path=Path(__file__).parents[1]/'src'/'sqrt_table.rs'
    path.write_text('// Black-box measurements of the oracle ptlens radius estimator; generated by research/probe.py sqrt_table.\n'
                    +'pub(crate) const SQRT_BITS: [u32; 4096] = [\n'+''.join('    '+','.join(f'0x{v.view(np.uint32):08x}' for v in table[i:i+16])+',\n' for i in range(0,len(table),16))+'];\n',encoding='utf-8')


def crop_precision():
    for name,crop in [('Nikon AF-S DX Nikkor 35mm f/1.8G',1.528*1.1),('E 24mm f/1.8 ZA',1.534*.8)]:
        rec=next(r for r in optics.lens_records() if r['model']==name and r['crop']>1);lens=optics.profile_lens(rec)
        print(name)
        maps=[]
        for value in [crop,float(np.float32(crop))]:
            for scale in [1,0]:
                m=lf.Modifier(lens,value,301,200);m.initialize(lens.min_focal,4,scale=scale,pixel_format=np.float32,flags=int(lf.ModifyFlags.DISTORTION|lf.ModifyFlags.TCA|lf.ModifyFlags.SCALE))
                a=m.apply_geometry_distortion();maps.append(a)
                print('crop',repr(value),'scale',scale,'corners',a[0,0],a[0,-1])
        print('diff unscaled/scaled',np.max(abs(maps[0]-maps[2])),np.max(abs(maps[1]-maps[3])))


def scale_composition():
    for name,crop,w,h in [('Nikon AF-S DX Nikkor 35mm f/1.8G',1.528*1.1,301,200),('E 24mm f/1.8 ZA',1.534*.8,96,64),('E 24mm f/1.8 ZA',1.534,96,64)]:
        rec=next(r for r in optics.lens_records() if r['model']==name and r['crop']>1);lens=optics.profile_lens(rec)
        scales=[]
        for flags in [lf.ModifyFlags.DISTORTION,lf.ModifyFlags.TCA,lf.ModifyFlags.DISTORTION|lf.ModifyFlags.TCA]:
            flags=int(flags|lf.ModifyFlags.SCALE)
            def at(scale):
                m=lf.Modifier(lens,crop,w,h);m.initialize(lens.min_focal,4,scale=scale,pixel_format=np.float32,flags=flags)
                a=m.apply_geometry_distortion(xu=(w-1)/2+w*.3,yu=(h-1)/2,width=1,height=1)
                return float(a[0,0,0]) if a is not None else (w-1)/2+w*.3
            target=at(0);lo=.8;hi=1.2
            for _ in range(30):
                mid=(lo+hi)/2
                if at(mid)>target:lo=mid
                else:hi=mid
            scales.append((lo+hi)/2)
        print(name,w,h,crop,scales,'ratio',scales[2]/scales[0],'add',scales[2]-scales[0])


def norm_identity():
    for w,h,crop,cal,aspect in [(96,64,1,1,1.5),(301,200,1.6808,1.528,1.5),(96,64,1.2272,1.534,1.5),(96,64,4.63,4.63,4/3)]:
        db,lens=synthetic('<tca model="linear" focal="50" kr="1" kb="1"/>',crop=cal,aspect=str(aspect));m=lf.Modifier(lens,crop,w,h);m.initialize(50,4,scale=1,pixel_format=np.float32,flags=int(lf.ModifyFlags.TCA))
        actual=m.apply_subpixel_geometry_distortion()[:,:,1]
        aspect=float(np.float32(aspect));crop=float(np.float32(crop));cal=float(np.float32(cal));dia=np.hypot(w-1,h-1)
        q=2*np.sqrt(aspect**2+1)*cal/crop/dia
        ar=float(np.float32((w-1)/(h-1)))
        norms=[q,2*np.sqrt((aspect**2+1)/(ar*ar+1))*cal/crop/(h-1),float(np.float32(q))]
        for i,q in enumerate(norms):
            cx=(w-1)/2*q;cy=(h-1)/2*q;x=np.float32(-cx);y=np.float32(-cy)
            xx=[];yy=[]
            for _ in range(w):xx.append(np.float32((float(x)+cx)/q));x+=np.float32(q)
            for _ in range(h):yy.append(np.float32((float(y)+cy)/q));y+=np.float32(q)
            print('identity',w,h,crop,cal,i,q,max(np.max(abs(np.array(xx)-actual[0,:,0])),np.max(abs(np.array(yy)-actual[:,0,1]))),xx[0],actual[0,0,0])


if __name__ == '__main__':
    globals()[sys.argv[1] if len(sys.argv)>1 else 'inventory']()
