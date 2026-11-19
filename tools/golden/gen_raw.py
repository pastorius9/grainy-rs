"""Synthetic RAW/DCP parity data. Oracle is read-only; no owner data is opened.

Run with the oracle interpreter, -B recommended. All files are generated here.
"""
import sys
sys.dont_write_bytecode = True
import os
os.environ['PYTHONDONTWRITEBYTECODE'] = '1'
from common import Case as BaseCase, inputs, settings, np, ORACLE, GOLDEN, ROOT, write_json
from pathlib import Path
import argparse
import shutil
import struct
import json
import hashlib
import io
import itertools
import unicodedata
import rawpy
import tifffile
from luma import dcp, rawcolor, raw_defaults

ACTIVE_CASES = set()
DEFAULT_SETTINGS = settings()
EXHAUSTIVE = False


class Case(BaseCase):
    """RAW-local lossless array/settings pool, read through grainy-testkit.

    Only bulk development outputs are sampled; all decoder arrays, source arrays,
    arithmetic, no-ops, and representative development paths remain full-frame.
    """
    def __enter__(self):
        ACTIVE_CASES.add(self.folder.name)
        return super().__enter__()

    def array(self, name, value):
        a = np.ascontiguousarray(value)
        if a.dtype == np.bool_:
            a = a.astype(np.uint8)
        stream = io.BytesIO()
        np.save(stream, a, allow_pickle=False)
        data = stream.getvalue()
        key = hashlib.sha256(data).hexdigest()
        path = GOLDEN / 'raw' / '_arrays' / (key + '.npy')
        path.parent.mkdir(parents=True, exist_ok=True)
        if not path.exists():
            path.write_bytes(data)
        self.meta.setdefault('arrays', {})[name] = key
        return value

    def json(self, **values):
        if 'settings' in values:
            value = values['settings']
            data = json.dumps(value, sort_keys=True, ensure_ascii=False).encode('utf8')
            key = hashlib.sha256(data).hexdigest()
            write_json(GOLDEN / 'raw' / '_settings' / key / 'case.json',
                       {k: v for k, v in value.items() if k not in DEFAULT_SETTINGS or v != DEFAULT_SETTINGS[k]})
            values['settings'] = {'shared_settings': key}
        super().json(**values)

    def output(self, value, full):
        if full or EXHAUSTIVE:
            self.array('output', value)
        else:
            # Float64, row-major reductions are reproducible across Python/Rust.
            pixels = value.astype(np.float64).reshape(-1, 3)
            self.json(sample=dict(stride=4, shape=list(value.shape),
                                  mean=pixels.mean(axis=0), min=pixels.min(axis=0),
                                  max=pixels.max(axis=0)))
            self.array('output', value[::4, ::4])


def fixture(case, name):
    path = GOLDEN / 'raw' / '_files' / case.folder.name / name
    path.parent.mkdir(parents=True, exist_ok=True)
    case.json(file=path.relative_to(GOLDEN / 'raw').as_posix())
    return path


def profile_path(name):
    return GOLDEN / 'raw' / '_files' / ('profile_' + name) / 'profile.dcp'


def development_design(name, file_index, variants):
    """Cover every pair of WB mode, profile/toggle variant and working space.

    The original six toggle bundles are retained verbatim. Include default and
    extreme WB/all-on/all-off combinations in both spaces, plus the complete
    matrix on one Bayer file. All file/setting pairs remain covered as well.
    """
    universe = list(itertools.product(range(6), range(variants), range(2)))
    chosen = [row for row in universe if
              EXHAUSTIVE or name in ('bayer_0_16', 'triple_asset') or
              row[2] == (row[0] + row[1] + file_index) % 2 or
              (row[0] in (0, 3, 4) and row[1] in (0, 1, variants - 1))]
    for a, b in itertools.combinations(range(3), 2):
        assert {(row[a], row[b]) for row in chosen} == {(row[a], row[b]) for row in universe}
    return set(chosen)


def prune_generated(full):
    """Remove obsolete generated files only, strictly within this crate's goldens."""
    root = (GOLDEN / 'raw').resolve()
    assert root == (ROOT / 'golden' / 'raw').resolve()
    live = {root / 'generator.json', root / '_settings' / 'defaults' / 'case.json'}
    for path in root.glob('*/case.json'):
        if full and path.parent.name not in ACTIVE_CASES:
            continue
        live.add(path)
        value = json.loads(path.read_text(encoding='utf8'))
        live.update(root / '_arrays' / (key + '.npy') for key in value.get('arrays', {}).values())
        if 'file' in value:
            live.add(root / value['file'])
        key = value.get('settings', {}).get('shared_settings')
        if key:
            live.add(root / '_settings' / key / 'case.json')
    assert all(path.resolve().is_relative_to(root) for path in live)
    for path in root.rglob('*'):
        if path.is_file() and path not in live:
            assert path.resolve().is_relative_to(root)
            path.unlink()
    for path in sorted(root.rglob('*'), key=lambda p: len(p.parts), reverse=True):
        if path.is_dir() and not any(path.iterdir()):
            path.rmdir()


def profile_json(p):
    result = dict(p)
    for k in ('hs1', 'hs2', 'hs3', 'look'):
        a = p.get(k)
        if a is not None:
            result[k] = dict(dims=list(a.shape[:3]), data=a.ravel(), double=a.dtype == np.float64)
    return result


def info_json(info):
    result = dict(info)
    if result.get('base_profile'):
        result['base_profile'] = profile_json(result['base_profile'])
    return result


def pack_dcp(tags, endian='<'):
    """Standalone, deterministic TIFF/DCP writer for branch/validation fixtures."""
    formats = {1:'B',2:'B',3:'H',4:'I',5:'I',7:'B',9:'i',10:'i',11:'f',12:'d'}
    entries = bytearray(); payload = bytearray()
    start = 8 + 2 + len(tags)*12 + 4
    for tag,kind,value in tags:
        if kind == 2:
            block = value.encode('utf8') + b'\0'; count = len(block)
        elif kind == 7:
            block = bytes(value); count = len(block)
        else:
            value = np.asarray(value).ravel().tolist()
            block = struct.pack(endian+str(len(value))+formats[kind],*value)
            count = len(value)//2 if kind in (5,10) else len(value)
        slot = block.ljust(4,b'\0') if len(block)<=4 else struct.pack(endian+'I',start+len(payload))
        if len(block)>4:
            payload.extend(block)
        entries.extend(struct.pack(endian+'HHI',tag,kind,count)+slot)
    return (b'II' if endian=='<' else b'MM')+struct.pack(endian+'HIH',0x4352,8,len(tags))+entries+b'\0'*4+payload


CM1=np.array([[.83,-.08,-.04],[-.14,1.12,.08],[-.03,.09,.71]])
CM2=np.array([[.91,-.16,-.08],[-.21,1.19,.05],[-.02,.06,.63]])
FM1=np.array([[.62,.24,.1043],[.25,.68,.07],[.04,.13,.6551]])
FM2=np.array([[.65,.20,.1143],[.24,.69,.07],[.03,.14,.6551]])
CC1=np.array([[1.02,.01,0],[0,.99,.01],[0,0,1.01]])
CC2=np.array([[.99,0,.01],[.01,1.01,0],[0,.01,1.02]])


def custom_profiles():
    table = np.empty((3,6,4,3),np.float32)
    for v in range(3):
        for h in range(6):
            for s in range(4):
                table[v,h,s] = [(h-2)*7+s*1.5, .7+.08*s+.03*v, 1 if s==0 else .8+.12*v+.03*h]
    base=[(50708,2,'Grainy Synthetic'),(50936,2,'Synthetic dual'),(50932,2,'grainy'),
          (50721,12,CM1),(50722,12,CM2),(50778,3,[17]),(50779,3,[21]),
          (50937,4,[6,4,3]),(50938,11,table),(50939,11,table*np.array([.7,1,1],np.float32)),
          (50981,4,[6,4,3]),(50982,11,table),(51107,4,[1]),(51108,4,[1]),
          (51109,12,[.25]),(50940,12,[0,0,.16,.1,.5,.6,.9,.98])]
    triple=[tag for tag in base if tag[0]!=50936]+[(50936,2,'Synthetic triple'),(52529,3,[23]),
        (52531,12,(CM1+CM2)/2+np.eye(3)*.03),(52537,11,table*np.array([1.3,1,1],np.float32))]
    return {
        'synthetic_dual':pack_dcp(base),
        'synthetic_forward':pack_dcp(base+[(50964,12,FM1),(50965,12,FM2)]),
        'synthetic_big_endian':pack_dcp(base, '>'),
        'synthetic_triple':pack_dcp(triple),
        'synthetic_single':pack_dcp([(50721,12,CM1),(50778,3,[21])]),
        'synthetic_hue_one':pack_dcp([(50721,12,CM1),(50937,4,[1,2,1]),(50938,11,[0,1,1,30,.5,1.2])]),
        'synthetic_custom':pack_dcp([(50721,12,CM1),(50722,12,CM2),(50778,3,[255]),(50779,3,[21]),
                                   (52533,7,struct.pack('<HIIII',0,40,100,38,100))]),
    }


def profiles():
    all_profiles={n:(ORACLE/'assets'/f'{n}.dcp').read_bytes() for n in ('test-camera','test-triple-illuminant')}
    all_profiles.update(custom_profiles())
    for name,data in all_profiles.items():
        p=dcp.parse(data)
        with Case('raw','profile_'+name) as c:
            path=fixture(c,'profile.dcp');path.write_bytes(data)
            record=dcp.load_profile(path)
            c.json(profile=profile_json(p),record=record,tags={k:(dict(bytes=list(v)) if isinstance(v,bytes) else v) for k,v in dcp.read_tags(data).items()})
            sweeps=[]
            for t in [2000.,2856.,3077.,4000.,5003.,6504.,10000.,50000.]:
                white=rawcolor.temperature_xyz(t,10.)
                w=dcp.weight(p,t,white)
                for signature in ['',p.get('signature','')]:
                    info=dict(analog=[1.02,.99,1.01],calibration=[CC1.tolist(),CC2.tolist(),np.eye(3).tolist()],calibration_signature=signature)
                    neutral=rawcolor.individual_matrix(p,info,w)@dcp.interpolate(p,'cm',w)@white
                    neutral=np.maximum(neutral,.01);neutral/=neutral[1]
                    solved=rawcolor.neutral_white(p,neutral,info)
                    sweeps.append(dict(temperature=t,white=white,weight=w,neutral=neutral,info=info,
                        cm=dcp.interpolate(p,'cm',w),fm=dcp.interpolate(p,'fm',w),hs=profile_json({'hs1':dcp.interpolate(p,'hs',w)})['hs1'] if p.get('hs1') is not None else None,
                        calibration=dcp.calibration_matrix(info['calibration'],w),individual=rawcolor.individual_matrix(p,info,w),
                        neutral_white=solved,camera=dcp.camera_to_xyz(p,neutral,t,info['calibration'],info['analog'],signature,white),
                        camera_inferred=dcp.camera_to_xyz(p,neutral,t,info['calibration'],info['analog'],signature)))
            c.json(sweeps=sweeps)
            matching=[]
            for meta in [dict(camera=p.get('camera','')),dict(make='Grainy',camera='Synthetic'),dict(camera='other'),dict(unique_camera=p.get('camera',''))]:
                matching.append(dict(metadata=meta,result=dcp.camera_matches(p,meta)))
            c.json(matching=matching)
        for input_name,a in inputs().items():
            if input_name=='garden':continue
            for kind in ['hs1','look']:
                if p.get(kind) is None:continue
                for encoding in [0,1]:
                    with Case('raw',f'table_{name}_{kind}_{encoding}_{input_name}') as c:
                        c.array('input',a);c.array('output',dcp.table_map(a,p[kind],encoding))
                        c.json(profile=name,kind=kind,encoding=encoding)
            with Case('raw',f'tone_{name}_{input_name}') as c:
                c.array('input',a);c.array('output',dcp.tone_map(a,p['curve']));c.json(curve=p['curve'])
    # Parser rejection corpus, including ranges, duplicates, singular matrices and unsupported features.
    base=[(50721,12,CM1)]
    invalids={'short':b'II','bad_magic':b'II'+b'\0'*20,'singular':pack_dcp([(50721,12,np.zeros((3,3)))]),
              'duplicate':pack_dcp(base+base),'missing_cm':pack_dcp([(50778,3,[21])]),
              'exposure':pack_dcp(base+[(51109,12,[11])]),'bad_curve':pack_dcp(base+[(50940,12,[0,0,0,1])]),
              'zero_denominator':pack_dcp([(50721,10,[1,0]*9)]),'nan':pack_dcp([(50721,12,[float('nan')]*9)]),
              'incomplete_dual':pack_dcp(base+[(50722,12,CM2)]),'third_without_cm':pack_dcp(base+[(52529,3,[21])]),
              'hdr':pack_dcp(base+[(52551,7,struct.pack('<HHf',1,1,1.))]),'gainmap':pack_dcp(base+[(52525,4,[1])]),
              'encoding':pack_dcp(base+[(51107,4,[2])]),'bad_table':pack_dcp(base+[(50937,4,[1,2,1]),(50938,11,[0,1,2,0,1,1])])}
    for name,data in invalids.items():
        with Case('raw','invalid_'+name) as c:
            fixture(c,'profile.dcp').write_bytes(data)
            try:dcp.parse(data)
            except Exception as e:c.json(error=type(e).__name__,message=str(e))
            else:raise AssertionError(name)


def temperatures():
    values=[]
    for k in [2000.,2500.,2856.,3000.,3077.,3200.,4000.,5000.,6504.,12000.,50000.]:
        for t in [-150.,-25.,0.,37.,150.]:
            xyz=rawcolor.temperature_xyz(k,t)
            values.append(dict(kelvin=k,tint=t,xyz=xyz,reverse=rawcolor.xyz_temperature(xyz),adapt=dcp.adapt_white(xyz)))
    with Case('raw','temperature') as c:c.json(values=values)


def rational(matrix):
    return tuple(x for v in np.asarray(matrix).ravel() for x in (int(round(float(v)*1000000)),1000000))


def write_dng(path,pattern=0,bits=16,black=64,dual=False,forward=False,linear=False,compression=None,orientation=1,endian='<',floating=False,lossy=False,bigtiff=False):
    h,w=64,80;yy,xx=np.mgrid[:h,:w];maximum=(1<<bits)-1
    rgb=np.stack([.04+.7*xx/(w-1),.05+.6*yy/(h-1),.08+.55*(xx+yy)/(w+h-2)],-1)
    patterns=[[0,1,1,2],[1,0,2,1],[1,2,0,1],[2,1,1,0]]
    if linear:pixels=np.uint16(black+rgb*(maximum-black))
    else:
        cfa=np.array(patterns[pattern]).reshape(2,2)[yy%2,xx%2]
        pixels=np.uint16(black+np.take_along_axis(rgb,cfa[...,None],axis=2)[...,0]*(maximum-black))
    tags=[(50706,'B',4,(1,4,0,0),False),(50707,'B',4,(1,3,0,0),False),
          (271,'s',0,'Grainy',False),(272,'s',0,'Synthetic',False),(50708,'s',0,'Grainy Synthetic',False),
          (274,'H',1,orientation,False),(50717,'I',1,maximum,False),
          (50728,'2I',3,(1,2,1,1,2,3),False),(50778,'H',1,17 if dual else 21,False),
          (50721,'2i',9,rational(CM1),False)]
    if linear:tags += [(50714,'2I',3,(black,1,black+2,1,black+4,1),False)]
    else:tags += [(33421,'H',2,(2,2),False),(33422,'B',4,patterns[pattern],False),
                 (50713,'H',2,(2,2),False),(50714,'2I',4,(black,1,black+1,1,black+2,1,black+3,1),False)]
    if dual:tags += [(50722,'2i',9,rational(CM2),False),(50779,'H',1,21,False),
                     (50723,'2i',9,rational(CC1),False),(50724,'2i',9,rational(CC2),False),
                     (50727,'2I',3,(102,100,99,100,101,100),False),(50931,'s',0,'grainy',False),(50932,'s',0,'grainy',False)]
    if forward:tags += [(50964,'2i',9,rational(FM1),False),(50965,'2i',9,rational(FM2),False)]
    if floating:
        pixels=pixels.astype(np.float32)/maximum
        tags=[t for t in tags if t[0] not in (50713,50714,50717)]
        tags += [(50717,'I',1,1,False)]
        bits=32
    if lossy:
        pixels=np.uint8(rgb*255)
        tags=[t for t in tags if t[0] not in (50713,50714,50717)]
        tags += [(50717,'I',1,65535,False)]
        tifffile.imwrite(path,pixels,photometric=34892,planarconfig='contig',metadata=None,extratags=tags,
                         compression='jpeg',tile=(32,32),compressionargs=dict(outcolorspace='RGB'))
        # The codestream is JPEG, but DNG uses its own compression identifier.
        with tifffile.TiffFile(path,mode='r+') as tif:tif.pages[0].tags[259].overwrite(34892)
    else:
        tifffile.imwrite(path,pixels,photometric=34892 if linear else 32803,planarconfig='contig' if linear else None,metadata=None,extratags=tags,bigtiff=bigtiff,
                         byteorder=endian,bitspersample=bits,compression=compression,
                         **(dict(tile=(32,32),predictor=3) if floating else dict(rowsperstrip=16)))
        if floating:
            # LibRaw's float tile reader assumes LONG TileByteCounts, while
            # tifffile optimises these tiny tiles to SHORT. Emit explicit LONGs.
            with tifffile.TiffFile(path) as tif:
                tag=tif.pages[0].tags[325];offset=tag.offset;counts=tag.value
            with path.open('r+b') as f:
                f.seek(0,2);start=f.tell();f.write(struct.pack(endian+str(len(counts))+'I',*counts))
                f.seek(offset+2);f.write(struct.pack(endian+'HI',4,len(counts)))
                f.write(struct.pack(endian+'I',start))


def metadata(raw):
    return dict(sizes=list(raw.sizes[:8]),pixel_aspect=raw.sizes.pixel_aspect,flip=raw.sizes.flip,
        num_colors=raw.num_colors,color_desc=raw.color_desc.decode('ascii').rstrip('\0'),
        black_level_per_channel=raw.black_level_per_channel,white_level=raw.white_level,
        camera_white_level_per_channel=raw.camera_white_level_per_channel,
        camera_whitebalance=raw.camera_whitebalance,daylight_whitebalance=raw.daylight_whitebalance,
        rgb_xyz_matrix=raw.rgb_xyz_matrix,color_matrix=raw.color_matrix)


def files():
    cases=[]
    for pattern in range(4):
        for bits in [12,14,16]:cases.append((f'bayer_{pattern}_{bits}',dict(pattern=pattern,bits=bits,black=2**(bits-6),dual=pattern%2==1,forward=pattern==3,orientation=[1,3,6,8][pattern])))
    cases.extend([('linear',dict(linear=True)),('linear_dual',dict(linear=True,dual=True,forward=True)),
                  ('deflate',dict(compression='deflate')),('deflate_float',dict(compression=8,floating=True)),
                  ('deflate_linear',dict(compression=8,floating=True,linear=True)),('lossy',dict(lossy=True,linear=True)),
                  ('big_endian',dict(endian='>',dual=True))])
    for file_index,(name,options) in enumerate(cases+[('triple_asset',None)]):
        with Case('raw','decode_'+name) as c:
            path=fixture(c,'합성 RAW.dng')
            if options is None:shutil.copyfile(ORACLE/'assets/test-triple-illuminant.dng',path)
            else:write_dng(path,**options)
            c.json(options=options,linear_raw=raw_defaults.linear_raw(path))
            try:
                with rawpy.imread(str(path)) as r:c.json(metadata=metadata(r))
            except rawpy.LibRawError as e:
                c.json(error=type(e).__name__,message=str(e));continue
            for half in [False,True]:
                for mode,color in [('camera',rawpy.ColorSpace.raw),('srgb',rawpy.ColorSpace.sRGB),('prophoto',rawpy.ColorSpace.ProPhoto)]:
                    with rawpy.imread(str(path)) as r:
                        args=dict(use_camera_wb=mode!='camera',no_auto_bright=True,output_bps=16,gamma=(1,1),output_color=color,half_size=half)
                        if mode=='camera':args.update(use_auto_wb=False,user_wb=[1,1,1,1],adjust_maximum_thr=0)
                        c.array(f'{mode}_{int(half)}',r.postprocess(**args))
                with rawpy.imread(str(path)) as r:
                    meta=dict(make='Grainy',camera='Synthetic',format='DNG') if options is not None else dict(make='Luma',camera='Test Triple',format='DNG')
                    source=rawcolor.decode(r,meta,16 if half else None,path)
                    c.array(f'source_{int(half)}',np.asarray(source));c.json(**{f'info_{int(half)}':info_json(source.raw_info),f'as_shot_{int(half)}':[meta['raw_as_shot_kelvin'],meta['raw_as_shot_tint']]})
            # Full sensor frame, already stored above, reused for all development tests.
            with rawpy.imread(str(path)) as r:
                source=rawcolor.decode(r,dict(make='Grainy',camera='Synthetic',format='DNG'),None,path)
            modes=[dict(raw_mode='as_shot'),dict(raw_mode='daylight'),dict(raw_mode='sample',raw_neutral=[.5,1,.7]),
                   dict(raw_mode='custom',raw_kelvin=2000,raw_tint=-150),dict(raw_mode='custom',raw_kelvin=50000,raw_tint=150),
                   dict(raw_mode='custom',raw_kelvin=4300,raw_tint=17)]
            # Triple fixture's camera restriction differs; standalone fixture records below match only synthetic files.
            records=[None]
            if options is not None:
                records += [dcp.load_profile(profile_path('synthetic_forward'))]
            design=development_design(name,file_index,7 if options is not None else 1)
            c.json(development_design=dict(factors=['mode','profile_toggle','space'],
                                          levels=[6,7 if options is not None else 1,2],
                                          rows=sorted(design)))
            for mi,mode in enumerate(modes):
                for ri,record in enumerate(records):
                    for si,space in enumerate(['sRGB','ProPhoto']):
                        toggles=[{}] if not record else [{},{'dcp_huesat':False},{'dcp_look':False},{'dcp_tone':False},{'dcp_exposure':False},dict(dcp_huesat=False,dcp_look=False,dcp_tone=False,dcp_exposure=False)]
                        for ti,toggle in enumerate(toggles):
                            if (mi,0 if ri==0 else ti+1,si) not in design:continue
                            s=settings(**mode,dcp_profile=record,working_space=space,**toggle)
                            with Case('raw',f'develop_{name}_{mi}_{ri}_{space}_{ti}') as dev:
                                p=dcp.compiled(record) if record else rawcolor.default_profile(source.raw_info)
                                dev.json(source=name,settings=s,white=rawcolor.resolve_white(p,source.raw_info,s,True))
                                dev.output(rawcolor.develop_camera(source,s), full=(
                                    name=='bayer_0_16' and (mi==0 or ri==0)) or (
                                    mi==0 and ri==0 and (si==0 or name=='triple_asset')))
            for neg in [False,True]:
                s=settings(raw_mode='as_shot',negative=neg,negative_base=[.8,.7,.6],negative_peak=[.02,.03,.01])
                with Case('raw',f'linear_frame_{name}_{int(neg)}') as c2:
                    c2.json(source=name,settings=s);c2.array('output',rawcolor.linear_frame(source,s))


def arithmetic():
    for name,a in inputs().items():
        if name=='garden':continue
        with Case('raw','false_color_'+name) as c:
            c.array('input',a);c.array('output',rawcolor.reduce_false_color(a))
        with Case('raw','plain_'+name) as c:
            s=settings(negative=True,negative_base=[.8,.7,.6],negative_peak=[.02,.03,.01],negative_range=12.,negative_density=2.5)
            c.array('input',a);c.array('develop',rawcolor.develop_camera(a,s));c.array('linear',rawcolor.linear_frame(a,s));c.json(settings=s)
    for shape in [(1,1,3),(2,4,3),(0,0,3)]:
        a=np.zeros(shape,np.float32)
        with Case('raw','false_color_small_'+str(shape[0])) as c:c.array('input',a);c.array('output',rawcolor.reduce_false_color(a))
    # Non-DNG default profile, absent calibration, partial calibration and all
    # standalone profiles on reusable synthetic camera-space pixels.
    a=inputs()['colours']*.6
    info=dict(make='Grainy',camera='Synthetic',format='DNG',xyz_to_camera=CM1.tolist(),neutral=[.5,1.,.7],daylight_neutral=[.6,1.,.8])
    with Case('raw','default_profile') as c:
        p=rawcolor.default_profile(info)
        c.json(info=info,profile=profile_json(p),white=rawcolor.neutral_white(p,info['neutral'],info),
               calibration=[dict(input=[],weight=.3,output=dcp.calibration_matrix([],.3)),
                   dict(input=[CC1],weight=.3,output=dcp.calibration_matrix([CC1],.3)),
                   dict(input=[CC1],weight=[.2,.3,.5],output=dcp.calibration_matrix([CC1],np.array([.2,.3,.5])))])
    for name in ['none','test-camera','test-triple-illuminant',*custom_profiles()]:
        record=None if name=='none' else dcp.load_profile(profile_path(name))
        one=dict(info)
        if record:one['camera']=record['camera']
        source=rawcolor.CameraSource(a,one)
        for mode in ['as_shot','daylight','sample','custom']:
            for space in ['sRGB','ProPhoto']:
                s=settings(raw_mode=mode,raw_neutral=[.5,1,.7],raw_kelvin=4300.,raw_tint=-18.,working_space=space,dcp_profile=record)
                with Case('raw',f'camera_math_{name}_{mode}_{space}') as c:
                    c.array('input',a);c.json(info=info_json(one),settings=s)
                    c.output(rawcolor.develop_camera(source,s),full=mode=='as_shot' or name=='synthetic_triple')
    noops=[{},dict(raw_mode='legacy'),dict(raw_mode='as_shot'),dict(negative=True),dict(negative=True,negative_base=[.8,.7,.6])]
    with Case('raw','noops') as c:
        c.array('input',a);c.json(values=[dict(settings=s,enabled=rawcolor.enabled(s)) for s in noops])
        for i,s in enumerate(noops):c.array(str(i),rawcolor.linear_frame(a,s))


def defaults():
    infos=[dict(make='NIKON CORPORATION',camera='NIKON Z 6',format='NEF',iso=iso,serial='  AB123\0') for iso in [None,100,400,800,1600,3200,6400,12800,32000,51200,1e6,12345.678,.00012,'3200','nan',0,1e9]]
    infos += [dict(make='Ｃａｎｏｎ Inc.',camera='Canon Ｒ６',format='CR3',iso=1600),dict(make='Straße Co Ltd',camera='Straße Ö',format='DNG',iso=6400,linear_raw=True),dict(format='JPG'),dict(make='Nikon',camera='Z6',format='NEF',serial='')]
    photo_info=dict(make='NIKON CORPORATION',camera='NIKON Z 6',format='NEF',serial='AB123')
    photos=[]
    for i,iso in enumerate([100,800,6400]):
        photos.append(dict(path='synthetic.NEF',info={**photo_info,'iso':iso},settings=settings(**{k:float(i*15+1) for k in raw_defaults.ISO_KEYS})))
    anchors=raw_defaults.iso_anchors(photos)
    rules=[];rule_inputs=[]
    for scope in ['master','model','serial']:
        s=settings(raw_mode='sample',raw_neutral=[.5,1,.7],exposure=.7)
        args=dict(scope=scope,groups=['화이트 밸런스','빛','디테일'],anchors=anchors,name='test '+scope)
        rule=raw_defaults.make_rule(photo_info,s,**args);rules.append(rule);rule_inputs.append(dict(info=photo_info,settings=s,**args,rule=rule))
    result=[]
    for info in infos:
        result.append(dict(info=info,key=raw_defaults.camera_key(info),serial=raw_defaults.serial(info),is_raw=raw_defaults.is_raw(info),iso=raw_defaults.iso_value(info),luma=raw_defaults.builtin_luma_nr(raw_defaults.iso_value(info)),detail=raw_defaults.builtin_iso_detail(raw_defaults.iso_value(info)),
                           empty=raw_defaults.resolve(info,[]),resolved=raw_defaults.resolve(info,rules),base=raw_defaults.resolve(info,rules,base=settings(exposure=-1))))
    retargets=[]
    for mode in ['legacy','sample','custom']:
        for info in [dict(format='JPG'),dict(format='NEF',camera='Wrong'),dict(format='DNG',camera='Grainy Synthetic')]:
            s=settings(raw_mode=mode,raw_neutral=[.5,1,.7],dcp_profile=dcp.load_profile(profile_path('synthetic_forward')))
            retargets.append(dict(settings=s,metadata=info,output=rawcolor.retarget(s,info),enabled=rawcolor.enabled(s)))
    with Case('raw','defaults') as c:
        c.json(photos=photos,anchors=anchors,rule_inputs=rule_inputs,rules=rules,values=result,retargets=retargets,
               keys=[dict(input=s,output=raw_defaults.key(s)) for s in ['Straße K Å','ＣＡＮＯＮ ① ﬃ','카메라 １２','İıΣςσ']])


def unicode_tables():
    # Pin Python's Unicode version: Rust's current tables and unicode-casefold
    # use different versions, which changes persisted camera keys and rule IDs.
    folds=[];ranges=[];start=None;previous=None;test=set()
    for i in range(0x110000):
        if 0xd800<=i<=0xdfff:continue
        ch=chr(i);fold=ch.casefold()
        if fold!=ch:folds.append((i,fold));test.add(i)
        if unicodedata.normalize('NFKC',ch)!=ch:test.add(i)
        if ch.isalnum():
            if start is None:start=i
            previous=i
        elif start is not None:
            ranges.append((start,previous));test.update([max(0,start-1),start,previous,i]);start=None
    if start is not None:ranges.append((start,previous))
    lines=[f'//! Generated by gen_raw.py from Python Unicode {unicodedata.unidata_version}.',
           '// Full default case folding and Python isalnum, used for persisted camera keys.',
           'const FOLD: &[(u32, &str)] = &[']
    for i,s in folds:lines.append(f'    (0x{i:x}, "'+''.join('\\u{'+format(ord(c),'x')+'}' for c in s)+'"),')
    lines += ['];','const ALNUM: &[(u32, u32)] = &[']
    lines += [f'    (0x{a:x}, 0x{b:x}),' for a,b in ranges]
    lines += ['];', '''pub(crate) fn fold_alnum(s: &str) -> String {
    let mut out = String::new();
    let mut append = |ch: char| {
        let code = ch as u32;
        let i = ALNUM.partition_point(|r| r.1 < code);
        if ALNUM.get(i).is_some_and(|r| r.0 <= code) {
            out.push(ch);
        }
    };
    for ch in s.chars() {
        if let Ok(i) = FOLD.binary_search_by_key(&(ch as u32), |p| p.0) {
            for folded in FOLD[i].1.chars() {
                append(folded);
            }
        } else {
            append(ch);
        }
    }
    out
}''']
    (ROOT/'crates/grainy-raw/src/unicode_data.rs').write_text('\n'.join(lines)+'\n',encoding='utf8',newline='\n')
    with Case('raw','unicode') as c:
        c.json(version=unicodedata.unidata_version,keys=[dict(input=chr(i),output=raw_defaults.key(chr(i)),folded=''.join(ch for ch in chr(i).casefold() if ch.isalnum())) for i in sorted(test) if not 0xd800<=i<=0xdfff])


def calibration_files():
    for name in ['bigtiff_le','bigtiff_be','subifd','no_matrix','bad_neutral','zero_denominator','large_tag','singular']:
        with Case('raw','calibration_'+name) as c:
            path=fixture(c,'input.dng')
            if name.startswith('bigtiff'):
                write_dng(path,linear=True,dual=True,forward=True,bigtiff=True,endian='<' if name.endswith('le') else '>')
            else:
                tags=[(50728,'2I',3,(1,2,1,1,2,3),False)]
                if name=='bad_neutral':tags=[(50728,'2I',3,(0,1,1,1,2,3),False)]
                if name=='zero_denominator':tags=[(50728,'2I',3,(1,0,1,1,2,3),False)]
                if name=='large_tag':tags=[(50728,'d',1025,np.ones(1025),False)]
                if name=='singular':tags=[(50721,'2i',9,rational(np.zeros((3,3))),False)]
                if name=='subifd':tags += [(50721,'2i',9,rational(CM1),False)]
                with tifffile.TiffWriter(path) as t:
                    t.write(np.zeros((16,16,3),np.uint16),photometric=34892,planarconfig='contig',metadata=None,extratags=tags,subifds=1 if name=='subifd' else None)
                    if name=='subifd':t.write(np.zeros((16,16),np.uint16),photometric=32803,metadata=None,extratags=[(50721,'2i',9,rational(CM2),False)])
            info=dict(camera='Generated',xyz_to_camera=np.eye(3).tolist(),neutral=[1,1,1],daylight_neutral=[1,1,1])
            c.json(before=info_json(info),linear_raw=raw_defaults.linear_raw(path))
            try:rawcolor.read_dng_calibration(path,info)
            except ValueError as e:c.json(error=str(e))
            else:c.json(after=info_json(info))


def raw_metadata():
    from PIL import Image, TiffImagePlugin
    from luma.engine import read_metadata
    R=TiffImagePlugin.IFDRational
    for name in ['dng_le','dng_be','nef','cr2','rw2','orf','cr3','raf','empty','invalid_gps']:
        with Case('raw','metadata_'+name) as c:
            exif=Image.Exif();exif.endian='>' if name=='dng_be' else '<'
            for k,v in {271:'Grainy',272:'Synthetic Camera',315:'Test Author',33432:'Generated fixture'}.items():exif[k]=v
            values={33434:R(2,250),33437:R(28,10),34855:3200,37386:R(50,1),36867:'2026:01:02 03:04:05',
                    42036:'Synthetic Lens',37382:R(7,2),42033:' AB123 '}
            gps={1:'S',2:(R(37),R(12),R(3,2)),3:'W',4:(R(127),R(4),R(5,2))}
            if name=='invalid_gps':gps={1:'N',2:(R(37),R(12))}
            if name=='rw2':values.pop(34855);exif[23]=6400
            exif[34665]=values;exif[34853]=gps;block=exif.tobytes()
            ext={'dng_le':'dng','dng_be':'dng','invalid_gps':'dng','empty':'nef'}.get(name,name)
            path=fixture(c,'input.'+ext)
            data=block[6:]
            if name=='empty':data=b'not a RAW file'
            elif name=='raf':
                jpeg=b'\xff\xd8\xff\xe1'+struct.pack('>H',len(block)+2)+block+b'\xff\xd9'
                header=b'FUJIFILMCCD-RAW'.ljust(84,b'\0')+struct.pack('>II',92,len(jpeg));data=header+jpeg
            elif name=='cr3':
                # The oracle labels standalone TIFF root tags by their CMT box.
                camera=pack_dcp([(271,2,'Grainy'),(272,2,'Synthetic Camera'),(315,2,'Test Author'),(33432,2,'Generated fixture')])
                details=pack_dcp([(33434,5,[2,250]),(33437,5,[28,10]),(34855,3,[3200]),(37386,5,[50,1]),
                                  (36867,2,'2026:01:02 03:04:05'),(42036,2,'Synthetic Lens'),(37382,5,[7,2]),(42033,2,' AB123 ')])
                position=pack_dcp([(1,2,'S'),(2,5,[37,1,12,1,3,2]),(3,2,'W'),(4,5,[127,1,4,1,5,2])])
                data=struct.pack('>I',16)+b'ftypcrx '+b'\0'*4
                for marker,payload in [(b'CMT1',camera),(b'CMT2',details),(b'CMT4',position)]:
                    payload=payload[:2]+b'*\0'+payload[4:];data+=struct.pack('>I',len(payload)+8)+marker+payload
            elif name=='rw2':data=data[:2]+b'U\0'+data[4:]
            elif name=='orf':data=data[:2]+b'RO'+data[4:]
            path.write_bytes(data);c.json(output=read_metadata(path))


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--section',choices=['all','profiles','temperatures','files','arithmetic','defaults','unicode','calibration','metadata'],default='all')
    parser.add_argument('--exhaustive',action='store_true',help='Temporary full matrix/full arrays for parity audits; regenerate without this flag before committing')
    args=parser.parse_args();EXHAUSTIVE=args.exhaustive
    write_json(GOLDEN/'raw'/'_settings'/'defaults'/'case.json',DEFAULT_SETTINGS)
    for name,fn in [('profiles',profiles),('temperatures',temperatures),('files',files),('arithmetic',arithmetic),('defaults',defaults),('unicode',unicode_tables),('calibration',calibration_files),('metadata',raw_metadata)]:
        if args.section in ('all',name):
            print(name,flush=True);fn()
    write_json(GOLDEN/'raw'/'generator.json',dict(rawpy=rawpy.__version__,libraw=rawpy.libraw_version,flags=rawpy.flags))
    prune_generated(args.section == 'all')
