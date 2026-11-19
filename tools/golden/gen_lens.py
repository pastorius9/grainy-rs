"""Lensfun 0.3.4 black-box goldens. Reads XML DATA and public numeric results only.

No library/binding implementation source, user library, settings, or oracle data/ is read.
Run with the oracle interpreter from the Rust worktree. Outputs stay below 9 MB.
"""
import sys
sys.dont_write_bytecode = True
from common import Case, ROOT, np, inputs, settings, write_json
import itertools
import hashlib
import json
from pathlib import Path
import xml.etree.ElementTree as ET
import lensfunpy as lf
from luma import optics, engine

MODELS = ['Nikon AF-S DX Nikkor 35mm f/1.8G', 'Canon EF 70-200mm f/2.8L IS USM',
          'Canon PowerShot G12 & compatibles (Standard)', 'Canon EF-M 22mm f/2 STM',
          'Canon PowerShot G3 & compatibles (Standard)', 'E 24mm f/1.8 ZA',
          'Schneider 80mm Xenotar f/2.8']
DBROOT = Path(lf.__file__).parent / 'db_files'
EXTRACT = ROOT / 'crates' / 'grainy-lens' / 'tests' / 'data'


def read_tree(path):
    return ET.parse(path, parser=ET.XMLParser(target=ET.TreeBuilder(insert_comments=True))).getroot()


def extract():
    EXTRACT.mkdir(parents=True, exist_ok=True)
    selected = {}
    mounts = set()
    for path in sorted(DBROOT.glob('*.xml')):
        root = read_tree(path)
        nodes = [n for n in root.findall('lens') if n.findtext('model') in MODELS]
        cameras = [n for n in root.findall('camera') if any(s in n.findtext('model','') for s in
                   ['EOS 5D Mark II', 'EOS 700D', 'EOS M', 'Nikon D7000', 'NEX-5', 'PowerShot G12', 'PowerShot G3'])]
        # A handful of exact cameras, not every EOS M variant or NEX-5 revision.
        cameras = [n for n in cameras if n.findtext('model') in ['Canon EOS 5D Mark II','Canon EOS 700D','Canon EOS M','Nikon D7000','NEX-5','Canon PowerShot G12','Canon PowerShot G3']]
        if nodes or cameras:
            selected[path.name] = cameras + nodes
            for n in cameras + nodes:
                mounts.update(m.text for m in n.findall('mount'))
    mount_nodes = []
    for path in sorted(DBROOT.glob('*.xml')):
        mount_nodes.extend(read_tree(path).findall('mount'))
    # Keep the transitive definition closure, but matching itself is direct, directional compatibility.
    while True:
        old = set(mounts)
        for n in mount_nodes:
            if n.findtext('name') in mounts:
                mounts.update(c.text for c in n.findall('compat'))
        if old == mounts: break
    selected['mounts.xml'] = [n for n in mount_nodes if n.findtext('name') in mounts]
    for filename,nodes in selected.items():
        xml='<lensdatabase version="1">\n'+''.join(ET.tostring(n, encoding='unicode') for n in nodes)+'</lensdatabase>\n'
        (EXTRACT/filename).write_text(xml,encoding='utf-8',newline='\n')
    return [r for r in optics.lens_records() if r['model'] in MODELS]


def plain_cal(c):
    if c is None:return None
    return {k: (getattr(c,k).name.lower() if k=='model' else getattr(c,k)) for k in ['model','focal','aperture','distance','terms'] if hasattr(c,k)}


def profile(record,focal,aperture=4,distance=1000,orientation=1,crop=None):
    # Out-of-range modifier probes intentionally bypass make_profile's UI range validation.
    return dict(xml=record['xml'],name=record['model'],maker=record['maker'],camera='',crop=record['crop'] if crop is None else crop,
                focal=focal,aperture=aperture,distance=distance,orientation=orientation)


def image(w,h):
    y,x=np.mgrid[:h,:w].astype(np.float32)
    return np.stack([x/max(w-1,1),y/max(h-1,1),.2+.4*x/max(w-1,1)+.2*y/max(h-1,1)],-1).astype(np.float32)


def selection(w,h):
    xs=sorted(set([0,1,2,3,4,5,w//4,w//2,3*w//4,w-5,w-4,w-3,w-2,w-1]))
    ys=sorted(set([0,1,2,h//4,h//2,3*h//4,h-3,h-2,h-1]))
    return xs,ys


def map_case(name,record,focal,w,h,distortion=True,tca=True,scale=True,crop=None,aperture=4,distance=1000,pixels=False,orientation=1,input_name=None):
    data=profile(record,focal,aperture,distance,orientation,crop)
    changes=dict(lensfun=data,lensfun_enabled=True,lensfun_distortion=distortion,lensfun_tca=tca,lensfun_scale=scale,lensfun_vignette=True)
    s=settings(**changes)
    _,lens=optics._saved_lens(record['xml'])
    m=optics._modifier(s,(h,w,3))
    xs,ys=selection(w,h)
    with Case('lens',name) as case:
        g=a=None
        if m is not None:
            if w*h*24 <= optics.MAP_BUDGET:
                g=m.apply_geometry_distortion(); a=m.apply_subpixel_geometry_distortion()
                if g is not None:g=g[np.ix_(ys,xs)]
                if a is not None:a=a[np.ix_(ys,xs)]
            else:
                gs=[];ss=[]
                for y in ys:
                    start=(y//128)*128;count=min(128,h-start)
                    gg=m.apply_geometry_distortion(yu=start,width=w,height=count)
                    aa=m.apply_subpixel_geometry_distortion(yu=start,width=w,height=count)
                    if gg is not None:gs.append(gg[y-start,xs])
                    if aa is not None:ss.append(aa[y-start,xs])
                g=np.asarray(gs) if gs else None;a=np.asarray(ss) if ss else None
        if g is not None:case.array('geometry',g)
        if a is not None:case.array('subpixel',a)
        case.json(kind='maps',settings=changes,width=w,height=h,xs=xs,ys=ys,model=plain_cal(lens.interpolate_distortion(focal)),tca=plain_cal(lens.interpolate_tca(focal)),vignetting=plain_cal(lens.interpolate_vignetting(focal,aperture,distance)))
        color=optics._modifier(s,(h,w,3),color=True)
        constant=np.ones((h,w,3),np.float32)
        changed=color.apply_color_modification(constant) if color else False
        case.array('gain',constant[np.ix_(ys,xs)])
        case.json(color_changed=bool(changed))
        if pixels:
            a=inputs()[input_name] if input_name else image(w,h)
            if orientation!=1:a=optics.orient_pixels(a,orientation)
            shade=optics.lens_shading(a,s);geo=optics.lens_geometry(a,s)
            # Store full small images, subsamples of the large strip path.
            if w*h*24>optics.MAP_BUDGET:
                case.array('shading',shade[np.ix_(ys,xs)]);case.array('remap',geo[np.ix_(ys,xs)])
                case.array('develop',engine.develop(a,s)[np.ix_(ys,xs)])
            else:
                case.array('shading',shade);case.array('remap',geo)
                case.array('develop',engine.develop(a,s))
            case.json(pixels=True,orientation=orientation,input_name=input_name)


def database_cases(records):
    xml='<lensdatabase version="1">'+''.join(''.join(ET.tostring(n,encoding='unicode') for n in read_tree(p)) for p in sorted(EXTRACT.glob('*.xml')))+'</lensdatabase>'
    db=lf.Database(xml=xml,load_common=False,load_bundled=False)
    cams=tuple(sorted(db.cameras,key=lambda c:(c.maker,c.model,c.variant or '')))
    # Scope only the oracle's database provider functions to the committed extract.
    optics.database=lambda:db;optics.cameras=lambda:cams;optics.lens_records=lambda:tuple(records)
    cases=[]
    for c in cams:
        for record in optics.compatible_lenses(c):
            cases.append(dict(make=c.maker,camera=c.model,lens=record['model'],focal_length=optics.profile_lens(record).min_focal,aperture=4))
    cases += [{},{'make':'Canon','camera':'not a camera'},
              {'make':'NIKON CORPORATION','camera':'NIKON D7000','lens':'Nikkor AF-S DX 35mm f/1.8G'},
              {'make':'Canon Inc.','camera':'EOS M','lens':'EF-M 22.0mm f/2.0 STM','focal_length':22,'aperture':2,'source_orientation':6},
              {'make':'Sony','camera':'NEX-5','lens':'E 24mm f/1.8 ZA','focal_length':'bad'}]
    for c in cams:
        cases.append(dict(make=c.maker,camera=c.model))
    matched=[]
    for metadata in cases:
        c,rs,reason=optics.match_metadata(metadata)
        p,why=optics.automatic_profile(metadata)
        matched.append(dict(metadata=metadata,camera=None if c is None else c.model,candidates=[r['id'] for r in rs],reason=reason,profile=p,profile_reason=why,description=optics.description(p)))
    lenses=[]
    for r in records:
        l=optics.profile_lens(r)
        attrs={k:getattr(l,k) for k in ['maker','model','mounts','crop_factor','min_focal','max_focal','min_aperture','max_aperture']}
        attrs['distortion']=[plain_cal(c) for c in l.calib_distortion]
        attrs['tca']=[plain_cal(c) for c in l.calib_tca]
        attrs['vignetting']=[plain_cal(c) for c in l.calib_vignetting]
        lenses.append(dict(record=r,lens=attrs))
    retarget=[]
    successful=[r['profile'] for r in matched if r['profile']]
    for p in successful[:5]:
        for metadata in [{},{'camera':'unknown'}, {'focal_length':9999}, {'source_orientation':9}, *cases[:5]]:
            s=dict(lensfun=p,lensfun_enabled=True,exposure=.5)
            out=optics.retarget(s,metadata)
            retarget.append(dict(settings=s,metadata=metadata,result=out,description=optics.description(out['lensfun'])))
    keys=['NIKON CORPORATION','Canon Inc.','24.000mm F/2.0','24.0001mm','incorporated','Co_Inc','ß ﬀ ﬁ K','A 1.00.00 B','ﬅ ﬆ ǰ ẖ ẗ ẘ ẙ ẚ']
    descriptions=[]
    for focal,aperture,crop in [(1e-5,0,1.9999),(999999.9,2.800001,999.9),(1.234567,8,9.9995),(50,0,1.6)]:
        p=dict(name='Test',focal=focal,aperture=aperture,crop=crop,capabilities=['distortion','unknown','vignette'],note='Test note')
        descriptions.append(dict(profile=p,description=optics.description(p)))
    with Case('lens','database') as case:
        case.json(kind='database',cameras=[{k:(getattr(c,k) or '' if k=='variant' else getattr(c,k)) for k in ['maker','model','variant','mount','crop_factor']} for c in cams],
                  lenses=lenses,matches=matched,retarget=retarget,keys=[dict(text=k,key=optics._key(k),model_key=optics._model_key(k,'Canon')) for k in keys],
                  descriptions=descriptions,compatible_none=[r['id'] for r in optics.compatible_lenses(None)],
                  compatible=[dict(camera=c.model,ids=[r['id'] for r in optics.compatible_lenses(c)]) for c in cams])


def radius_estimator():
    """Measure numeric outputs, never inspect or derive from implementation source."""
    xml='<lensdatabase><lens><maker>Test</maker><model>Test 50mm f/2</model><mount>Test</mount><cropfactor>1</cropfactor><aspect-ratio>1</aspect-ratio><calibration><distortion model="ptlens" focal="50" c="1"/></calibration></lens></lensdatabase>'
    _,lens=optics._saved_lens(xml)
    m=lf.Modifier(lens,1,3,3)
    m.initialize(50,4,scale=1,pixel_format=np.float32,flags=int(lf.ModifyFlags.DISTORTION))
    values=[]
    for i in range(4096):
        r2=np.uint32(0x3f800000+(i<<12)+2048).view(np.float32)
        out=m.apply_geometry_distortion(xu=2,yu=1+np.sqrt(r2-1),width=4,height=1)
        values.append(np.float32(out[0,0,0]-1))
    path=ROOT/'crates'/'grainy-lens'/'src'/'sqrt_table.rs'
    path.write_text('// Measured numeric outputs of the oracle ptlens radius estimator.\n'
                    '// Generated by tools/golden/gen_lens.py; no implementation source consulted.\n'
                    '#[rustfmt::skip]\npub(crate) const SQRT_BITS: [u32; 4096] = [\n'
                    +''.join('    '+', '.join(f'0x{v.view(np.uint32):08x}' for v in values[i:i+8])+',\n' for i in range(0,4096,8))+'];\n',encoding='utf-8',newline='\n')
    # Independent validation points, powers of two, and scalar tails.
    starts=[(1,1),(2,1),(2,1.5),(2,2),(2.1,2.7),(1.01,1.02),(.5,.7),(5.3,7.1),(-20.2,11.3)]
    # Sweep the exact discontinuity implicated by m0_focal1, on both sides.
    for dx in [-2,-1,0,1,2]:
        for dy in [-2,-1,0,1,2]:
            x=np.uint32(int(np.float32(1.3423657).view(np.uint32))+dx).view(np.float32)
            y=np.uint32(int(np.float32(-.8843294).view(np.uint32))+dy).view(np.float32)
            starts.append((float(np.float32(x+1)),float(np.float32(y+1))))
    cases=[];scalar=[]
    for xu,yu in starts:
        cases.append(m.apply_geometry_distortion(xu=xu,yu=yu,width=7,height=1)[0])
        scalar.append(m.apply_geometry_distortion(xu=xu,yu=yu,width=1,height=1)[0,0])
    with Case('lens','radius') as case:
        case.array('coordinates',cases)
        case.array('scalar',np.array(scalar,np.float32))
        case.json(xml=xml,starts=starts)


def measured_scale(lens, crop, w, h, focal, aperture, distance, flags):
    """Identify float32 scales by matching public maps, never native state/source.

    The nonlinear distortion means an output-distance ratio alone is insufficient.
    Match three independent off-centre rays against explicit-scale modifiers, then
    retain every adjacent float32 whose maps are indistinguishable (the reciprocal
    is rounded too). No fitted Rust solver participates in this measurement.
    """
    def modifier(scale):
        m=lf.Modifier(lens,crop,w,h)
        m.initialize(focal,aperture,distance,scale=scale,pixel_format=np.float32,flags=flags)
        return m
    positions=[(w*.21,h*.37),(w*.87,h*.19),(w*.63,h*.91)]
    def samples(m):
        values=[]
        for x,y in positions:
            a=m.apply_geometry_distortion(xu=x,yu=y,width=1,height=1)
            values.append(np.array([x,y],np.float32) if a is None else a[0,0])
        return np.array(values)
    auto=modifier(0)
    if auto.apply_geometry_distortion(width=1,height=1) is None:return [1.0],auto
    if not ((flags&8 and lens.interpolate_distortion(focal) is not None) or
            (flags&1 and lens.interpolate_tca(focal) is not None)):
        return [1.0],auto
    expected=samples(auto)
    lo,hi=.01,10.
    for _ in range(35):
        mid=(lo+hi)/2
        actual=samples(modifier(mid))
        if actual[1,0]>expected[1,0]:lo=mid
        else:hi=mid
    bits=int(np.float32((lo+hi)/2).view(np.uint32))
    matches=[]
    for candidate in range(bits-8,bits+9):
        scale=float(np.uint32(candidate).view(np.float32))
        if np.array_equal(samples(modifier(scale)),expected):matches.append(scale)
    if not matches:raise AssertionError(f'Unidentified automatic scale: {lens.model}, {w}x{h}, {crop}, {flags}')
    return matches,auto


def scale_cases(index):
    """Direct scales, sparse maps, and deliberately strong diagnostic profiles."""
    specs=[]
    for name in index:
        j=json.loads((ROOT/'golden/lens'/name/'case.json').read_text(encoding='utf-8'))
        s=j['settings']
        if s['lensfun_distortion'] and s['lensfun_scale']:
            specs.append((name,s['lensfun'],j['width'],j['height'],'profiles'))
    # Reuse the stored engine profile, without regenerating or editing its goldens.
    j=json.loads((ROOT/'golden/engine/lens_all/case.json').read_text(encoding='utf-8'))
    specs.append(('engine',j['settings']['lensfun'],240,160,'profiles'))
    # Strong polynomial stress probes expose unresolved numerical choices rather
    # than hiding them behind the ordinary coordinate tolerance.
    for i,(model,attrs) in enumerate([('poly3','k1=".1"'),('poly3','k1="-.03"'),
                                    ('poly5','k1=".1" k2="-.01"'),('poly5','k1="-.04" k2=".03"'),
                                    ('ptlens','a=".02" b="-.03" c=".01"'),('ptlens','a="-.01" b=".03" c="-.02"')]):
        for ar in [1,1.5,2]:
            xml=f'<lensdatabase><lens><maker>Test</maker><model>Test 50mm f/2</model><mount>Test</mount><cropfactor>1.3</cropfactor><aspect-ratio>{ar}</aspect-ratio><calibration><distortion model="{model}" focal="50" {attrs}/><tca model="linear" focal="50" kr="1.02" kb=".97"/><vignetting model="pa" focal="50" aperture="4" distance="1000" k1="-.1"/></calibration></lens></lensdatabase>'
            p=dict(xml=xml,crop=1.3,focal=50,aperture=4,distance=1000)
            for w,h,crop in [(96,64,1.3),(301,200,1.7),(600,400,1),(64,96,1.3),(101,101,1.3),(120,60,2),(7,5,1.3)]:
                specs.append((f'stress_{i}_{ar}_{w}_{h}',dict(p,crop=crop),w,h,'stress'))
    xmls=[];rows=[];geometries=[];subpixels=[]
    for name,p,w,h,suite in specs:
        if p['xml'] not in xmls:xmls.append(p['xml'])
        _,lens=optics._saved_lens(p['xml'])
        for flags in [40,41,42,43,33]:
            values,modifier=measured_scale(lens,p['crop'],w,h,p['focal'],p['aperture'],p['distance'],flags)
            xs=[0,w//2,w-1];ys=[0,h//2,h-1];g=[];sub=[]
            for y in ys:
                a=modifier.apply_geometry_distortion(yu=y,width=w,height=1)
                b=modifier.apply_subpixel_geometry_distortion(yu=y,width=w,height=1)
                # Native None is recorded separately; zero padding keeps the arrays compact.
                g.append(np.zeros((3,2),np.float32) if a is None else a[0,xs])
                sub.append(np.zeros((3,3,2),np.float32) if b is None else b[0,xs])
            rows.append(dict(name=f'{name}_{flags}',suite=suite,xml_index=xmls.index(p['xml']),width=w,height=h,
                             crop=p['crop'],focal=p['focal'],aperture=p['aperture'],distance=p['distance'],flags=flags,
                             scales=values,geometry=a is not None,subpixel=b is not None))
            geometries.append(g);subpixels.append(sub)
    with Case('lens','scales') as case:
        case.json(xmls=xmls,cases=rows,method='three-ray explicit-scale float32 search')
        case.array('geometry',np.array(geometries,np.float32))
        case.array('subpixel',np.array(subpixels,np.float32))


def defaults_case(record):
    a=inputs()['colours'];p=profile(record,35)
    rows=[]
    for changes in [{},{'lensfun':p},{'lensfun':p,'lensfun_enabled':True,'lensfun_distortion':False,'lensfun_tca':False,'lensfun_scale':False,'lensfun_vignette':False}]:
        s=settings(**changes)
        rows.append(dict(settings=changes,geometry_identity=bool(np.array_equal(optics.lens_geometry(a,s),a)),
                         shading_identity=bool(np.array_equal(optics.lens_shading(a,s),a))))
    with Case('lens','defaults') as case:case.json(cases=rows)


def matching_cases():
    xml='<lensdatabase version="1"><mount><name>A</name><compat>B</compat></mount><mount><name>B</name><compat>C</compat></mount><mount><name>C</name></mount>'
    for model,variant,crop in [('Camera','',1),('Crop','',1.5),('Twin','one',1),('Twin','two',1)]:
        xml+=f'<camera><maker>Test</maker><model>{model}</model><variant>{variant}</variant><mount>A</mount><cropfactor>{crop}</cropfactor></camera>'
    for model,mount,crop in [('Near','A',1.04166),('Near','A',1.04167),('Direct','B',1),('Transitive','C',1),('Tie','A',1),('Tie','A',1.0008),('Crop','A',1),('Crop','A',1.5)]:
        xml+=f'<lens><maker>Test</maker><model>{model} 50mm f/2</model><mount>{mount}</mount><cropfactor>{crop}</cropfactor><calibration><distortion model="poly3" focal="50" k1=".01"/></calibration></lens>'
    xml+='</lensdatabase>'
    db=lf.Database(xml=xml,load_common=False,load_bundled=False)
    cams=tuple(sorted(db.cameras,key=lambda c:(c.maker,c.model,c.variant or '')))
    records=[]
    for node in ET.fromstring(xml).findall('lens'):
        saved='<lensdatabase version="1">'+ET.tostring(node,encoding='unicode')+'</lensdatabase>'
        records.append(dict(id=hashlib.sha256(saved.encode()).hexdigest()[:20],maker=node.findtext('maker'),model=node.findtext('model'),mounts=[n.text for n in node.findall('mount')],crop=float(node.findtext('cropfactor')),xml=saved,file='synthetic.xml'))
    records=tuple(sorted(records,key=lambda r:(r['maker'],r['model'],r['crop'])))
    optics.database=lambda:db;optics.cameras=lambda:cams;optics.lens_records=lambda:records
    matches=[]
    for cam in ['Camera','Crop','Twin']:
        for lens in ['Near 50mm f/2','Direct 50mm f/2','Transitive 50mm f/2','Tie 50mm f/2','Crop 50mm f/2','No match']:
            metadata=dict(make='Test',camera=cam,lens=lens,focal_length=50,aperture=2)
            c,rs,reason=optics.match_metadata(metadata);p,why=optics.automatic_profile(metadata)
            matches.append(dict(metadata=metadata,camera=None if c is None else c.model,candidates=[r['id'] for r in rs],reason=reason,profile=p,profile_reason=why))
    with Case('lens','matching') as case:
        case.json(xml=xml,records=records,matches=matches,compatible=[dict(camera=c.model,variant=c.variant or '',ids=[r['id'] for r in optics.compatible_lenses(c)]) for c in cams])


def interpolation_cases(records):
    cases=[]
    for record in records:
        lens=optics.profile_lens(record)
        focals=sorted(set([lens.min_focal*.8,lens.min_focal,(lens.min_focal+lens.max_focal)/2,lens.max_focal,lens.max_focal*1.2]+[c.focal for c in lens.calib_distortion]))
        for focal in focals:
            for aperture,distance in [(0,1000),(1,1),(2,10),(3.5,20),(8,1000),(128,100000)]:
                cases.append(dict(xml=record['xml'],focal=focal,aperture=aperture,distance=distance,
                                  distortion=plain_cal(lens.interpolate_distortion(focal)),tca=plain_cal(lens.interpolate_tca(focal)),vignetting=plain_cal(lens.interpolate_vignetting(focal,aperture,distance))))
    # Synthetic, deliberately non-monotone values identify interpolation and endpoint rules.
    for model in ['poly3','poly5','ptlens','tca_linear','tca_poly3','mixed']:
        kind='tca' if model.startswith('tca') else 'distortion'
        entries=[]
        for i,(f,k) in enumerate(zip([10,20,40,80],[.01,.02,.08,.03])):
            actual=model.replace('tca_','') if model!='mixed' else ['poly3','poly3','ptlens','ptlens'][i]
            terms=(f'kr="{1+k}" kb="{1-k}"' if actual=='linear' else f'vr="{1+k}" cr="{k}" br="{-k}"' if kind=='tca' else f'a="{k}" b="{-k/2}" c="{k/3}"' if actual=='ptlens' else f'k1="{k}" k2="{k/2}"')
            entries.append(f'<{kind} model="{actual}" focal="{f}" {terms}/>')
        xml='<lensdatabase version="1"><lens><maker>Test</maker><model>Test 10-100mm f/2</model><mount>Test</mount><cropfactor>1</cropfactor><calibration>'+''.join(entries)+'</calibration></lens></lensdatabase>'
        _,lens=optics._saved_lens(xml)
        for focal in [5,10,15,20,25,30,35,40,50,60,80,100]:
            cases.append(dict(xml=xml,focal=focal,aperture=4,distance=1000,distortion=plain_cal(lens.interpolate_distortion(focal)),tca=plain_cal(lens.interpolate_tca(focal)),vignetting=None))
    # Avoid repeating full XML for every numerical row.
    xmls=list(dict.fromkeys(c['xml'] for c in cases))
    for c in cases:c['xml_index']=xmls.index(c.pop('xml'))
    with Case('lens','interpolation') as case:case.json(cases=cases,xmls=xmls)


def validation_cases(records):
    r=next(r for r in records if r['model']=='Canon EF-M 22mm f/2 STM')
    cases=[]
    for change in [{},{'crop':.09},{'crop':20.1},{'focal':23},{'focal':0},{'aperture':129},{'distance':0},{'orientation':0},{'orientation':9},{'aperture':0},{'aperture':1},{'aperture':2}]:
        cond=dict(crop=1.613,focal=22,aperture=4,distance=1000,orientation=1,camera='Test');cond.update(change)
        try:p=optics.make_profile(r,**cond);error=None
        except ValueError as e:p=None;error=str(e)
        cases.append(dict(conditions=cond,profile=p,error=error))
    invalid_xml=['<!DOCTYPE lensdatabase><lensdatabase/>','<lensdatabase/>','<wrong/>','<lensdatabase><lens/><lens/></lensdatabase>','x'*256001]
    validation=[]
    for xml in invalid_xml:
        try:optics._saved_lens(xml);error=None
        except ValueError as e:error=str(e)
        validation.append(dict(xml=xml if len(xml)<256001 else None,oversize=len(xml)>256000,error=error))
    with Case('lens','validation') as case:case.json(record=r,profiles=cases,xml=validation)


def main():
    records=extract()
    chosen=[next(r for r in records if r['model']==m and (m!=MODELS[0] or r['crop']>1) and (m!=MODELS[1] or r['crop']==1)) for m in MODELS]
    index=[]
    for i,r in enumerate(chosen):
        lens=optics.profile_lens(r);f=lens.min_focal
        for d,t,s in itertools.product([False,True],repeat=3):
            name=f'm{i}_flags{int(d)}{int(t)}{int(s)}';index.append(name)
            map_case(name,r,f,96,64,d,t,s,pixels=(d and t and s),input_name='colours' if i in [0,1,2] else None)
        focal_values=sorted(set([lens.min_focal*.8,lens.min_focal,(lens.min_focal+lens.max_focal)/2,lens.max_focal,lens.max_focal*1.2]))
        for j,focal in enumerate(focal_values):
            name=f'm{i}_focal{j}';index.append(name)
            map_case(name,r,focal,301,200,crop=r['crop']*1.1,aperture=3.5,distance=20,pixels=i==0 and focal==lens.min_focal)
        for j,(w,h,crop) in enumerate([(96,64,r['crop']*.8),(80,80,r['crop']),(120,60,r['crop']*1.5)]):
            name=f'm{i}_sensor{j}';index.append(name)
            map_case(name,r,f,w,h,crop=crop,aperture=0 if j==2 else 8,scale=j!=1,pixels=i==5 and j==0)
    for i in [0,1,2]:
        r=chosen[i];f=optics.profile_lens(r).min_focal
        name=f'm{i}_large';index.append(name);map_case(name,r,f,1450,1000,pixels=True)
        name=f'm{i}_portrait';index.append(name);map_case(name,r,f,96,64,pixels=True,orientation=6)
    scale_cases(index)
    interpolation_cases(records)
    validation_cases(records)
    radius_estimator()
    defaults_case(chosen[0])
    database_cases(records)
    matching_cases()
    with Case('lens','index') as case:case.json(cases=index,library_version=lf.lensfun_version)
    print(f'Generated {len(index)} map cases and database cases; {sum(p.stat().st_size for p in (ROOT/"golden"/"lens").rglob("*") if p.is_file())} bytes')


if __name__=='__main__':main()
