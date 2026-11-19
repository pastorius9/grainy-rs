"""Generated-only codec parity corpus. Never writes to the oracle or opens user data.

Run normally to regenerate; --check DIR checks Rust exports using Pillow/imagecodecs.
The imported oracle is the authority, including rasterio's high-bit decoding and PIL resizing.
"""
import sys
sys.dont_write_bytecode = True
import os
os.environ['PYTHONDONTWRITEBYTECODE'] = '1'
from common import Case, GOLDEN, ROOT, np, write_json
import io
import json
import struct
import zlib
import hashlib
import subprocess
from pathlib import Path
from PIL import Image, features
import PIL
import imagecodecs
import tifffile
import piexif
from luma import engine, rasterio, colorio

BASE = GOLDEN / 'codecs'

def freeze_profile(data):
    data = bytearray(data)
    data[24:36] = struct.pack('>6H', 2026, 1, 1, 0, 0, 0)
    data[84:100] = bytes(16)
    return bytes(data)

original_profile = colorio.profile
original_cms_profile = imagecodecs.cms_profile
def cms_profile(*args,**kwargs):
    return freeze_profile(original_cms_profile(*args,**kwargs))
imagecodecs.cms_profile = cms_profile
def profile(name='sRGB'):
    return freeze_profile(original_profile(name))
colorio.profile = rasterio.profile = profile

def check_exports(folder):
    report = []
    manifest = json.loads((folder / 'manifest.json').read_text(encoding='utf-8'))
    for row in manifest:
        path = folder / row['file']
        case = BASE / row['case']
        exif = (case / 'exif.bin').read_bytes() if row['metadata'] else None
        icc = (BASE / 'profiles' / (row['profile'] + '.icc')).read_bytes()
        if path.suffix == '.jxl':
            actual = imagecodecs.jpegxl_decode(path.read_bytes())
            # The modular encoder writes the oracle's implicit sRGB colour encoding.
            info = {}
        elif row['format'] == 'PNG 16-bit':
            actual = imagecodecs.png_decode(path.read_bytes())
            with Image.open(path) as im: info = dict(im.info)
        elif path.suffix == '.tif':
            actual = tifffile.imread(path)
            with tifffile.TiffFile(path) as tif:
                tags = tif.pages[0].tags
                info = {'icc_profile': tags[34675].value}
                if row['metadata']:
                    assert bytes(tags[700].value) == (case / 'xmp.bin').read_bytes()
                    assert bytes(tags[33723].value) == (case / 'iptc.bin').read_bytes()
                    # Pillow's appended IFDs reduce rationals and replace non-ASCII TIFF text.
                    oracle_tiff=io.BytesIO();tifffile.imwrite(oracle_tiff,np.zeros((1,1,3),'u1'),photometric='rgb',metadata=None)
                    colorio.append_tiff_exif(oracle_tiff,exif)
                    expected = piexif.load(oracle_tiff.getvalue())
                    actual_exif = piexif.load(str(path))
                    for key in ('Exif', 'GPS'):
                        assert actual_exif[key] == expected[key], (path, key)
                    for tag in (269,270,271,272,274,282,283,296,306,315,33432,40091,40092,40093,40094,40095):
                        if tag in expected['0th']: assert actual_exif['0th'].get(tag)==expected['0th'][tag], (path,tag)
        else:
            with Image.open(path) as im:
                actual = np.asarray(im).copy(); info = dict(im.info)
        expected = np.load(case / row['expected'])
        if expected.shape[-1:] == (1,): expected = expected[..., 0]
        lossless = path.suffix not in ('.jpg', '.avif')
        if lossless: assert np.array_equal(actual, expected), (path, np.max(np.abs(actual.astype(float)-expected)))
        if path.suffix != '.jxl': assert info.get('icc_profile') == icc, (path, 'ICC')
        if exif and path.suffix in ('.png', '.jpg', '.avif'):
            got = info.get('exif', b'')
            assert got.removeprefix(b'Exif\0\0') == exif.removeprefix(b'Exif\0\0'), (path, 'EXIF')
        if row['metadata'] and path.suffix in ('.png', '.avif'):
            got = info.get('XML:com.adobe.xmp', info.get('xmp'))
            if isinstance(got, str): got = got.encode()
            assert got == (case / 'xmp.bin').read_bytes(), (path, 'XMP')
        if row['metadata'] and path.suffix=='.jxl':
            data=path.read_bytes();pos=12;chunks={}
            while pos<len(data):
                length=int.from_bytes(data[pos:pos+4],'big');chunks[data[pos+4:pos+8]]=data[pos+8:pos+length];pos+=length
            assert chunks[b'Exif']==bytes(4)+exif.removeprefix(b'Exif\0\0')
            assert chunks[b'xml ']==(case/'xmp.bin').read_bytes()
        if row['metadata'] and path.suffix=='.jpg':
            data=path.read_bytes();assert data[2:4]==b'\xff\xed';n=int.from_bytes(data[4:6],'big');app=data[6:4+n];assert app.startswith(b'Photoshop 3.0\0');assert (case/'iptc.bin').read_bytes() in app
        if path.suffix=='.jpg':
            reference=io.BytesIO();Image.fromarray(expected).save(reference,format='JPEG',quality=int(row.get('quality',95)),subsampling=0,
                icc_profile=icc,exif=exif or b'')
            oracle_folder=folder/'oracle-jpeg';oracle_folder.mkdir(exist_ok=True)
            oracle_path=oracle_folder/path.name;oracle_path.write_bytes(reference.getvalue())
            if row['metadata']:
                settings=json.loads((case/'case.json').read_text(encoding='utf-8'))
                colorio.jpeg_iptc(oracle_path,settings['metadata'],settings['keywords'])
            assert path.read_bytes()==oracle_path.read_bytes(),(path,'JPEG file differs from Pillow')
            with Image.open(path) as actual_header,Image.open(reference) as oracle_header:
                assert actual_header.quantization==oracle_header.quantization,(path,'quality tables')
                assert all((c[1],c[2])==(1,1) for c in actual_header.layer),(path,'4:4:4 sampling')
        row_result=dict(file=row['file'], exact_pixels=bool(np.array_equal(actual, expected)), lossless=lossless,
                           max_sample_error=float(np.max(np.abs(actual.astype(float)-expected.astype(float)))))
        if path.suffix=='.jpg':row_result['pillow_file_identical']=True
        report.append(row_result)
    write_json(folder / 'pillow-check.json', report)
    print(json.dumps(report, indent=1))

def rgb_pattern(h=19, w=31):
    y, x = np.mgrid[:h, :w]
    return np.stack([(x*37+y*13)%256, (x*3+y*67)%256, (x*x+y*11)%256], -1).astype('uint8')

RICH = dict(title='A & B <film>', creator='작가 Zoë', copyright='© 2026', caption='Line 1\nLine 2 & <3>',
            city='서울', location='屋上', country='KR', DateTimeOriginal='2024:02:03 04:05:06',
            latitude=-33.8587, longitude=151.214)
KEYWORDS = ' travel, film|東京, ,한글, a&b '

def source_exif(orientation=1):
    return piexif.dump({'0th': {271:b'Camera Co', 272:b'Model 42', 274:orientation, 315:b'Old Author', 33432:b'Old copyright', 282:(300,1),283:(300,1),296:2},
        'Exif':{33434:(1,125),33437:(28,10),34855:400,37386:(50,1),37382:(3,2),36867:b'2020:01:02 03:04:05',42036:b'Lens 50',42033:b'SERIAL42',37500:b'MAKERNOTE',40965:1},
        'GPS':{1:b'N',2:((37,1),(33,1),(30,1)),3:b'W',4:((122,1),(24,1),(15,1))},
        'Interop':{1:b'R98'},'1st':{},'thumbnail':None})

def add_file(name, filename, writer, max_size=None, working='sRGB'):
    with Case('codecs', name) as case:
        path = case.folder / filename
        writer(path)
        os.utime(path, ns=(1700000000000000000,1700000000000000000))
        record = dict(file=filename,max_size=max_size,working=working)
        try:
            # Record the actual decoder's samples, before transfer, alpha and orientation.
            if path.suffix == '.png': raw = imagecodecs.png_decode(path.read_bytes())
            elif path.suffix == '.tif': raw = tifffile.imread(path)
            else:
                with Image.open(path) as im:
                    if max_size and im.format=='JPEG': im.draft(im.mode,(max_size*2,max_size*2))
                    if im.mode not in ('CMYK','L','LA','1','I;16','I;16B','I;16L','LAB'):
                        im = im.convert('RGBA' if 'A' in im.getbands() or 'transparency' in im.info else 'RGB')
                    raw = np.asarray(im)
            case.array('decoded', raw)
            comparison = raw
            if path.suffix == '.tif':
                with tifffile.TiffFile(path) as tif:
                    comparison = tif.pages[0].asarray()
                    if tif.pages[0].planarconfig == 2: comparison = np.moveaxis(comparison,0,-1)
            if comparison.ndim == 2: comparison = comparison[...,None]
            case.array('decoded_f32',comparison.astype('f4'))
            pixels, info = engine.load_image(path,max_size,working)
            info.pop('file_fingerprint')  # asserted against the generated file's live stat in Rust
            record['metadata']=info
            record['read_metadata']=engine.read_metadata(path)
            case.array('linear',pixels)
            if max_size and working=='sRGB': case.array('thumbnail',np.asarray(engine.thumbnail(path,max_size)))
        except Exception as exc:
            record['error']=mask_root(type(exc).__name__+': '+str(exc))
        case.json(**record)

def mask_root(message):
    """Replace this checkout's folder (repr-escaped or not, either slash) with a fixed `<root>` placeholder."""
    root = str(ROOT)
    for variant in (root.replace('\\', '\\\\'), root, root.replace('\\', '/')):
        message = message.replace(variant, '<root>')
    return message

def raw_png(path, color, bits, interlace, transparency=False):
    channels={0:1,2:3,3:1,4:2,6:4}[color]; h,w=13,17
    y,x,c=np.indices((h,w,channels)); values=((x*13+y*7+c*31)%(1<<bits)).astype(np.uint16)
    if bits==16: values=(values.astype(np.uint32)*257%65536).astype(np.uint16)
    raw=bytearray(); passes=[(0,0,1,1)] if not interlace else [(0,0,8,8),(4,0,8,8),(0,4,4,8),(2,0,4,4),(0,2,2,4),(1,0,2,2),(0,1,1,2)]
    for x0,y0,dx,dy in passes:
        for row in values[y0::dy,x0::dx]:
            if not row.size: continue
            raw.append(0); vals=row.ravel()
            if bits==16: raw.extend(vals.astype('>u2').tobytes())
            elif bits==8: raw.extend(vals.astype('u1').tobytes())
            else:
                packed=np.zeros((len(vals)*bits+7)//8,'u1')
                for i,v in enumerate(vals): packed[i*bits//8] |= int(v) << (8-bits-(i*bits%8))
                raw.extend(packed)
    chunk=rasterio.png_chunk
    out=b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',w,h,bits,color,0,0,int(interlace)))
    if color==3:
        palette=np.arange((1<<bits)*3,dtype='u1').tobytes();out+=chunk(b'PLTE',palette)
        if transparency: out+=chunk(b'tRNS',bytes(range(1<<bits)))
    elif transparency and color in (0,2): out+=chunk(b'tRNS',struct.pack('>H',0)*(1 if color==0 else 3))
    out+=chunk(b'IDAT',zlib.compress(bytes(raw)))+chunk(b'IEND',b'');path.write_bytes(out)

def generate_jpeg_variants():
    """Fixtures from generated pixels only. No source downloads, user photos, or oracle writes."""
    a = rgb_pattern(33, 35)
    for sub in ('411', '440'):
        add_file('jpeg_sampling_' + sub, 'input.jpg',
                 lambda p: p.write_bytes(imagecodecs.jpeg_encode(a, level=91, subsampling=sub)))
    add_file('jpeg_rgb_components', 'input.jpg',
             lambda p: Image.fromarray(a).save(p, quality=91, keep_rgb=True, subsampling=0))
    cmyk = np.asarray(Image.fromarray(a).convert('CMYK'))
    add_file('jpeg_ycck', 'input.jpg', lambda p: p.write_bytes(imagecodecs.jpeg_encode(
        cmyk, level=91, colorspace='CMYK', outcolorspace='YCCK')))
    def without_adobe(path):
        stream = io.BytesIO(); Image.fromarray(a).convert('CMYK').save(stream, format='JPEG', quality=91)
        data = stream.getvalue()
        assert data[2:4] == b'\xff\xee'
        path.write_bytes(data[:2] + data[4+int.from_bytes(data[4:6], 'big'):])
    add_file('jpeg_cmyk_no_adobe', 'input.jpg', without_adobe)
    for orientation in range(1, 9):
        add_file(f'jpeg_orientation_{orientation}', 'input.jpg',
                 lambda p: Image.fromarray(a).save(p, quality=91, exif=source_exif(orientation)), max_size=7)
    for h, w in ((1, 1), (1, 17), (17, 1), (16, 16), (17, 17)):
        add_file(f'jpeg_size_{w}x{h}', 'input.jpg',
                 lambda p: Image.fromarray(rgb_pattern(h, w)).save(p, quality=95))
    add_file('jpeg_restart_blocks_progressive', 'input.jpg', lambda p: Image.fromarray(a).save(
        p, quality=95, progressive=True, restart_marker_blocks=2, subsampling=1))
    add_file('jpeg_12bit_refused', 'input.jpg', lambda p: p.write_bytes(
        imagecodecs.jpeg_encode(a.astype('u2') * 16, bitspersample=12)))
    # Arithmetic JPEG cannot be written by Pillow/imagecodecs. Transcode coefficients with our
    # already-vendored library; Pillow, not the transcoder, supplies all expected decoded pixels.
    scratch = ROOT / 'crates/grainy-codecs/test-output/jpeg-fixtures'
    scratch.mkdir(parents=True, exist_ok=True)
    executable = scratch / ('arithmetic.exe' if os.name == 'nt' else 'arithmetic')
    env = dict(os.environ, GRAINY_JPEG_FIXTURE_TOOL=str(executable))
    env['PATH'] = str(Path.home() / '.cargo/bin') + os.pathsep + env['PATH']
    subprocess.run(['cargo', 'build', '--offline', '-p', 'grainy-jpeg-sys'], cwd=ROOT, env=env, check=True)
    source = scratch / 'input.jpg'; Image.fromarray(a).save(source, quality=91, subsampling=2)
    for progressive in (False, True):
        add_file(f'jpeg_arithmetic_{progressive}', 'input.jpg', lambda p: subprocess.run(
            [str(executable), str(source), str(p), str(int(progressive))], check=True))

def jpeg_golden_matrix():
    """Every generated JPEG: full resolution plus every attainable Pillow draft denominator."""
    for folder in sorted(BASE.iterdir()):
        path = folder / 'input.jpg'
        if not path.exists(): continue
        meta = json.loads((folder / 'case.json').read_text(encoding='utf-8'))
        drafts = []
        try:
            with Image.open(path) as im:
                full_size = im.size
            requests = [None] + sorted({max(1, min(full_size)//(2*d)) for d in (1,2,4,8)})
            for index, request in enumerate(requests):
                with Image.open(path) as im:
                    if request: im.draft(im.mode, (request*2, request*2))
                    arr = np.asarray(im)
                    if arr.ndim == 2: arr = arr[..., None]
                    filename = f'jpeg_pixels_{index}'
                    np.save(folder / (filename+'.npy'), arr)
                    drafts.append(dict(max_size=request, array=filename, width=im.width, height=im.height))
            meta['jpeg_full_size'] = list(full_size)
        except Exception as exc:
            meta['jpeg_decode_error'] = mask_root(type(exc).__name__ + ': ' + str(exc))
        meta['jpeg_drafts'] = drafts
        write_json(folder / 'case.json', meta)
    # Byte-for-byte encoder fixtures exercise default, quality extremes, gray/RGB, odd/tiny
    # dimensions, EXIF ordering, and the exact ICC marker-size boundary and multi-chunk profiles.
    for mode in ('RGB', 'L'):
        for quality in (-1, 0, 1, 35, 50, 95, 100):
            for metadata in ('none', 'exif', 'both'):
                name = f'jpeg_encode_{mode}_q{quality}_{metadata}'
                with Case('codecs', name) as case:
                    pixels = rgb_pattern(9, 11)
                    if mode == 'L': pixels = pixels[..., :1]
                    case.array('pixels', pixels)
                    icc = profile('Gray sRGB' if mode == 'L' else 'sRGB') if metadata == 'both' else b''
                    exif = source_exif(6) if metadata != 'none' else b''
                    (case.folder / 'icc.bin').write_bytes(icc); (case.folder / 'exif.bin').write_bytes(exif)
                    Image.fromarray(pixels[..., 0] if mode == 'L' else pixels).save(
                        case.folder/'expected.jpg', quality=quality, subsampling=0, icc_profile=icc, exif=exif)
                    case.json(quality=quality)
    for length in (65519, 65520, 131039):
        with Case('codecs', f'jpeg_encode_icc_{length}') as case:
            pixels = rgb_pattern(1, 1); case.array('pixels', pixels)
            icc = bytes((i*37)%256 for i in range(length)); exif = source_exif()
            (case.folder/'icc.bin').write_bytes(icc); (case.folder/'exif.bin').write_bytes(exif)
            Image.fromarray(pixels).save(case.folder/'expected.jpg', quality=95, subsampling=0,
                                         icc_profile=icc, exif=exif)
            case.json(quality=95)

def jpeg_marker_goldens():
    stream = io.BytesIO(); Image.fromarray(rgb_pattern()).save(stream, format='JPEG', quality=95)
    base = stream.getvalue(); icc = profile(); midpoint = len(icc)//2
    def marker(kind, payload): return bytes((255,kind)) + struct.pack('>H',len(payload)+2) + payload
    first = b'ICC_PROFILE\0' + bytes((1,2)) + icc[:midpoint]
    second = b'ICC_PROFILE\0' + bytes((2,2)) + icc[midpoint:]
    exif = source_exif(6)
    for name, head, after_sof in [
        ('reversed_icc', marker(0xe2,second)+marker(0xe2,first), False),
        ('incomplete_icc', marker(0xe2,first), False),
        ('duplicate_icc', marker(0xe2,first)*2, False),
        ('late_icc', marker(0xe2,b'ICC_PROFILE\0'+bytes((1,1))+icc), True),
        ('split_exif', marker(0xe1,exif[:23])+marker(0xe1,b'Exif\0\0'+exif[23:]), False),
        ('unknown_app', marker(0xef,b'unknown')+marker(0xfe,b'comment'), False),
    ]:
        with Case('codecs', 'jpeg_markers_'+name) as case:
            offset = base.index(b'\xff\xda') if after_sof else 2
            path = case.folder/'input.jpg'; path.write_bytes(base[:offset]+head+base[offset:])
            with Image.open(path) as im:
                expected_icc = im.info.get('icc_profile')
                (case.folder/'profile.bin').write_bytes(expected_icc or b'')
                pixels=np.asarray(im);case.array('decoded_f32',pixels.astype('f4'))
                case.json(file='input.jpg',marker_only=True,profile=expected_icc is not None,
                          orientation=im.getexif().get(274,1))

def generate():
    BASE.mkdir(parents=True,exist_ok=True)
    schema=ROOT/'crates/grainy-codecs/assets/exif-types.json'
    write_json(schema,{name:{str(k):v['type'] for k,v in piexif.TAGS[name].items()} for name in ('0th','Exif','GPS')})
    profiles=BASE/'profiles';profiles.mkdir(exist_ok=True)
    for name in ('sRGB','Linear sRGB','Linear ProPhoto','Luma Wide','Adobe RGB','Display P3','ProPhoto RGB','Gray sRGB'):
        (profiles/(name+'.icc')).write_bytes(profile(name))
    write_json(BASE/'versions.json',dict(pillow=PIL.__version__,libjpeg_turbo=features.version('libjpeg_turbo'),numpy=np.__version__,imagecodecs=imagecodecs.__version__,tifffile=tifffile.__version__,
        oracle_sources={Path(m.__file__).name:hashlib.sha256(Path(m.__file__).read_bytes()).hexdigest() for m in (engine,rasterio,colorio)}))
    a=rgb_pattern()
    for progressive in (False,True):
        for sub in (0,1,2):
            for quality in (35,95,100):
                name=f'jpeg_{"progressive" if progressive else "baseline"}_{[444,422,420][sub]}_q{quality}'
                add_file(name,'input.jpg',lambda p:Image.fromarray(a).save(p,quality=quality,subsampling=sub,progressive=progressive))
    for mode in ('L','CMYK'):
        for progressive in (False,True):
            add_file(f'jpeg_{mode}_{progressive}','input.jpg',lambda p:Image.fromarray(a).convert(mode).save(p,quality=93,progressive=progressive))
    add_file('jpeg_restart','input.jpg',lambda p:Image.fromarray(rgb_pattern(67,65)).save(p,quality=90,restart_marker_rows=1))
    for size in (7,2):
        add_file(f'jpeg_draft_{size}','input.jpg',lambda p:Image.fromarray(rgb_pattern(63,65)).save(p,quality=95),max_size=size)
    generate_jpeg_variants()
    for name,pattern in [('smooth',np.tile(np.arange(47,dtype='u1')[None,:,None]*5,(33,1,3))),('noise',np.random.default_rng(42).integers(0,256,(47,53,3),dtype='u1'))]:
        for sub in (0,1,2):
            add_file(f'jpeg_{name}_{sub}','input.jpg',lambda p:Image.fromarray(pattern).save(p,quality=85,subsampling=sub))
        for lossless in (False,True):
            add_file(f'webp_{name}_{lossless}','input.webp',lambda p:Image.fromarray(pattern).save(p,lossless=lossless,quality=85))
    for orientation in range(1,9):
        add_file(f'orientation_{orientation}','input.png',lambda p:Image.fromarray(a).save(p,exif=source_exif(orientation)),max_size=11)
    for color,depths in ((0,(1,2,4,8,16)),(2,(8,16)),(3,(1,2,4,8)),(4,(8,16)),(6,(8,16))):
        for depth in depths:
            for interlace in (False,True):
                add_file(f'png_c{color}_b{depth}_i{int(interlace)}','input.png',lambda p:raw_png(p,color,depth,interlace))
        if color in (0,2,3):
            add_file(f'png_c{color}_trns','input.png',lambda p:raw_png(p,color,8,True,True))
    for mode in ('RGB','RGBA','L','1','P'):
        add_file('bmp_'+mode,'input.bmp',lambda p:Image.fromarray(a).convert(mode).save(p))
    for lossless in (False,True):
        for mode in ('RGB','RGBA'):
            rgba=np.concatenate((a,((a[...,:1].astype(int)*7)%256).astype('u1')),-1)
            add_file(f'webp_{lossless}_{mode}','input.webp',lambda p:Image.fromarray(a if mode=='RGB' else rgba).save(p,lossless=lossless,quality=90,exact=True))
    for bits in (8,16):
        pixels=a if bits==8 else a.astype('uint16')*257+np.uint16(1)
        for endian in ('<','>'):
            for compression in (None,'lzw','deflate'):
                for tiled in (False,True):
                    name=f'tiff_{bits}_{"le" if endian=="<" else "be"}_{compression or "none"}_{"tile" if tiled else "strip"}'
                    add_file(name,'input.tif',lambda p:tifffile.imwrite(p,pixels,photometric='rgb',byteorder=endian,compression=compression,metadata=None,**({'tile':(16,16)} if tiled else {'rowsperstrip':5})))
    for dtype in ('float32','float64'):
        add_file(f'tiff_{dtype}','input.tif',lambda p:tifffile.imwrite(p,(a.astype(dtype)/80-.2),photometric='rgb',metadata=None))
    for photo in ('minisblack','miniswhite'):
        add_file('tiff_'+photo,'input.tif',lambda p:tifffile.imwrite(p,a[...,0].astype('uint16')*257,photometric=photo,metadata=None))
    add_file('tiff_planar','input.tif',lambda p:tifffile.imwrite(p,np.moveaxis(a,-1,0),photometric='rgb',planarconfig='separate',metadata=None))
    for kind in ('unassalpha','assocalpha'):
        rgba=np.concatenate((a,(a[...,:1].astype('uint16')*7%256).astype('uint8')),-1)
        add_file('tiff_'+kind,'input.tif',lambda p:tifffile.imwrite(p,rgba,photometric='rgb',extrasamples=kind,metadata=None))
    add_file('tiff_multipage','input.tif',lambda p:tifffile.imwrite(p,np.stack((a,a)),photometric='rgb',metadata=None))
    for compression in ('lzw','deflate'):
        add_file('tiff_predictor_'+compression,'input.tif',lambda p:tifffile.imwrite(p,a.astype('u2')*257,photometric='rgb',compression=compression,predictor=True,tile=(16,16),metadata=None))
    palette=np.vstack((np.arange(256,dtype='u2')*257,np.arange(255,-1,-1,dtype='u2')*257,np.full(256,12345,dtype='u2')))
    add_file('tiff_palette','input.tif',lambda p:tifffile.imwrite(p,a[...,0],photometric='palette',colormap=palette,metadata=None))
    add_file('tiff_cmyk','input.tif',lambda p:tifffile.imwrite(p,np.asarray(Image.fromarray(a).convert('CMYK')),photometric='separated',metadata=None))
    add_file('tiff_ycbcr','input.tif',lambda p:tifffile.imwrite(p,a,photometric='ycbcr',metadata=None))
    add_file('tiff_lab','input.tif',lambda p:tifffile.imwrite(p,a,photometric='cielab',metadata=None))
    add_file('tiff_float_invalid','input.tif',lambda p:tifffile.imwrite(p,np.full((2,3,3),np.nan,dtype='f4'),photometric='rgb',metadata=None))
    add_file('tiff_signed_invalid','input.tif',lambda p:tifffile.imwrite(p,a.astype('i2'),photometric='rgb',metadata=None))
    add_file('tiff_oriented','input.tif',lambda p:tifffile.imwrite(p,a,photometric='rgb',extratags=[(274,'H',1,7,False)],metadata=None),max_size=7)
    for ext in ('png','jpg','webp','tif'):
        add_file('metadata_'+ext,'input.'+ext,lambda p:Image.fromarray(a).save(p,exif=source_exif(),icc_profile=profile('sRGB')))
    add_file('png_icc_wide','input.png',lambda p:Image.fromarray(a).save(p,icc_profile=profile('Adobe RGB')),max_size=9,working='ProPhoto')
    add_file('untagged_wide','input.png',lambda p:Image.fromarray(a).save(p),working='ProPhoto')
    add_file('gray_icc','input.png',lambda p:Image.fromarray(a[...,0]).save(p,icc_profile=profile('Gray sRGB')),max_size=11)
    add_file('jpeg_icc_preview','input.jpg',lambda p:Image.fromarray(a).save(p,icc_profile=profile('Adobe RGB')),max_size=9)
    def tagged_png(path,kind,payload):
        stream=io.BytesIO();Image.fromarray(a).save(stream,format='PNG');data=stream.getvalue();path.write_bytes(data[:33]+rasterio.png_chunk(kind,payload)+data[33:])
    for name,kind,payload in [('srgb',b'sRGB',b'\0'),('gamma',b'gAMA',struct.pack('>I',45455)),('chroma',b'cHRM',struct.pack('>8I',31270,32900,64000,33000,30000,60000,15000,6000)),('cicp_srgb',b'cICP',bytes([1,13,0,1])),('cicp_p3',b'cICP',bytes([12,13,0,1])),('cicp_invalid',b'cICP',bytes([9,16,0,1])),('gamma_invalid',b'gAMA',bytes(4))]:
        add_file('png_'+name,'input.png',lambda p:tagged_png(p,kind,payload))
    add_file('png_profile_mismatch','input.png',lambda p:Image.fromarray(a).save(p,icc_profile=profile('Gray sRGB')))
    # CMS stand-in inputs and outputs are generated independently of any Rust colour engine.
    for name in ('metadata_png','metadata_jpg','metadata_webp','metadata_tif','png_icc_wide','untagged_wide','gray_icc','jpeg_icc_preview','tiff_lab','png_srgb','png_gamma','png_chroma','png_cicp_srgb','png_cicp_p3'):
        folder=BASE/name;meta=json.loads((folder/'case.json').read_text(encoding='utf-8')); path=folder/meta['file']
        calls=[]; orig=rasterio.convert; original_transform=imagecodecs.cms_transform
        def convert(p,src,dst,*args,**kwargs):
            out=orig(p,src,dst,*args,**kwargs);calls.append((p.copy(),src,dst,out.copy()));return out
        def transform(p,src,dst,*args,**kwargs):
            out=original_transform(p,src,dst,*args,**kwargs);calls.append((p.copy(),src,dst,out.copy()));return out
        rasterio.convert=convert;imagecodecs.cms_transform=transform
        # Full decode exercises the same float path; optimized previews are separately compared end-to-end.
        engine.load_image(path,None,meta['working'])
        rasterio.convert=orig;imagecodecs.cms_transform=original_transform
        for i,(p,src,dst,out) in enumerate(calls):
            np.save(folder/f'cms_input_{i}.npy',p);np.save(folder/f'cms_output_{i}.npy',out)
            (folder/f'cms_source_{i}.icc').write_bytes(src);(folder/f'cms_destination_{i}.icc').write_bytes(dst)
        meta['cms_calls']=len(calls);write_json(folder/'case.json',meta)
    rng=np.random.default_rng(123)
    for name,arr in [('random',(rng.random((17,29,3))*1.4-.2).astype('f4')),('single',np.array([[[.2,.5,2]]],dtype='f4')),('thin',rng.random((1,31,3),dtype='f4'))]:
        for longest in (1,7,29,100):
            with Case('codecs',f'resize_{name}_{longest}') as case:
                case.array('input',arr);case.array('bilinear',engine.resize_float(arr,longest));case.array('lanczos',engine.resize_export(arr,longest));case.array('hdr',engine.resize_export(arr,longest,True));case.json(longest=longest)
    with Case('codecs','sizing') as case:
        rows=[]
        for shape in ((17,31,3),(300,200,3),(1,99,3)):
            for size in (20,1000,(0,0,'inside'),(10,9,'inside'),(10,9,'cover'),(0,9,'inside'),(10,0,'cover'),(999,999,'inside')):
                rows.append(dict(shape=shape,size=size,expected=engine.export_longest(shape,size)))
        case.json(cases=rows,distances=[dict(quality=q,distance=engine.jxl_distance(q)) for q in (-10,0,29,30,90,95,100,110)])
    with Case('codecs','dither_tile') as case:
        case.array('output',engine._dither_tile());case.json(state=np.random.default_rng(1905).bit_generator.state)
    arrays={'bounds':np.array([-.5,0,1/510,.5,1-1/510,1,1.2],dtype='f4')[None,:,None].repeat(3,2),
            'tile_wrap':rng.random((259,263,3),dtype='f4'), 'halfsteps':(np.arange(256,dtype='f4')+.5)[None,:,None].repeat(3,2)/255}
    for name,arr in arrays.items():
        with Case('codecs','quantize_'+name) as case:
            case.array('input',arr);case.array('u8',engine._quantize(arr.copy(),255,np.uint8));case.array('u16',engine._quantize(arr.copy(),65535,np.uint16));case.array('dither',engine._quantize8_dithered(arr));case.json()
    for name,delta in [('equal',0.),('within',.00009),('outside',.00011),('nan',float('nan'))]:
        arr=np.full((7,13,3),.5,'f4');arr[...,0]+=delta
        with Case('codecs','gray_'+name) as case:
            case.array('input',arr);gray=engine._neutral_gray(arr);case.json(neutral=gray is not None)
            if gray is not None:case.array('output',gray)
    source=BASE/'metadata_jpg'/'input.jpg'
    for name,metadata,keywords,color in [('empty',{},'','sRGB'),('rich',RICH,KEYWORDS,'Adobe RGB'),('clear',dict(latitude=None,longitude=' ',caption='',creator='',copyright=''),' , ','sRGB'),('zero',dict(latitude=0,longitude=0),'','sRGB')]:
        with Case('codecs','meta_'+name) as case:
            exif=colorio.export_exif(source,metadata,(31,19),color)
            (case.folder/'exif.bin').write_bytes(exif);(case.folder/'iptc.bin').write_bytes(colorio.iptc_bytes(metadata,keywords));(case.folder/'xmp.bin').write_bytes(colorio.xmp_metadata(metadata,keywords,exif))
            (case.folder/'xmp_no_exif.bin').write_bytes(colorio.xmp_metadata(metadata,keywords))
            jpeg=case.folder/'iptc.jpg';jpeg.write_bytes(source.read_bytes());colorio.jpeg_iptc(jpeg,metadata,keywords)
            for endian in ('<','>'):
                buffer=io.BytesIO();tifffile.imwrite(buffer,a,photometric='rgb',byteorder=endian,metadata=None)
                suffix='le' if endian=='<' else 'be';(case.folder/f'tiff_before_{suffix}.bin').write_bytes(buffer.getvalue());colorio.append_tiff_exif(buffer,exif);(case.folder/f'tiff_after_{suffix}.bin').write_bytes(buffer.getvalue())
            case.json(metadata=metadata,keywords=keywords,color=color,source='../metadata_jpg/input.jpg',size=[31,19])
    for name,src in [('missing',BASE/'nonexistent.jpg'),('png',BASE/'metadata_png/input.png'),('webp',BASE/'metadata_webp/input.webp'),('tiff',BASE/'metadata_tif/input.tif')]:
        with Case('codecs','exif_source_'+name) as case:
            (case.folder/'exif.bin').write_bytes(colorio.export_exif(src,RICH,None,'sRGB'));case.json(source=src.relative_to(BASE).as_posix(),metadata=RICH)
    for name,exif,xmp in [('bare',None,None),('exif',b'Exif\0\0MM\0*',None),('xmp',None,b'<x/>'),('both',b'II*\0',b'<x/>')]:
        with Case('codecs','jxl_'+name) as case:
            (case.folder/'output.bin').write_bytes(engine._jxl_container(b'\xff\x0a\x12\x34',exif,xmp));case.json(exif=list(exif or b''),xmp=list(xmp or b''))
    # Raw-container fallback without decoding RAW files.
    for kind in ('cr3','raf'):
        with Case('codecs','container_'+kind) as case:
            exif=source_exif()[6:]
            if kind=='raf':
                jpeg=source.read_bytes();header=bytearray(92);header[:14]=b'FUJIFILMCCD-RAW';header[84:92]=struct.pack('>II',92,len(jpeg));data=bytes(header)+jpeg
            else:data=struct.pack('>I',8+len(exif))+b'CMT1'+exif
            path=case.folder/('input.'+kind);path.write_bytes(data);case.json(file=path.name,metadata=engine.read_metadata(path))
    # Separate Canon TIFF boxes exercise _container_tags' prefix rewriting, including its GPS limitation.
    def standalone(entries,table):
        from PIL import TiffImagePlugin
        ifd=TiffImagePlugin.ImageFileDirectory_v2()
        for tag,value in entries.items():
            typ=piexif.TAGS[table][tag]['type'];ifd.tagtype[tag]=typ
            if typ==5:
                value=TiffImagePlugin.IFDRational(*value) if isinstance(value[0],int) else tuple(TiffImagePlugin.IFDRational(*v) for v in value)
            if typ==2:value=value.decode('ascii')
            ifd[tag]=value
        handle=io.BytesIO();ifd.save(handle);return handle.getvalue()
    with Case('codecs','container_cr3_separate') as case:
        tables=piexif.load(source_exif());payload=b''
        for kind,table in [(b'CMT1','0th'),(b'CMT2','Exif'),(b'CMT4','GPS')]:
            entries={k:v for k,v in tables[table].items() if k not in (34665,34853,40965,37500)}
            block=standalone(entries,table);payload+=struct.pack('>I',len(block)+8)+kind+block
        path=case.folder/'input.cr3';path.write_bytes(payload);case.json(file=path.name,metadata=engine.read_metadata(path))
    with Case('codecs','metadata_long') as case:
        m=dict(caption='한'*10901,creator=None,title=True);keywords=' , a, a, '
        (case.folder/'iptc.bin').write_bytes(colorio.iptc_bytes(m,keywords));(case.folder/'xmp.bin').write_bytes(colorio.xmp_metadata(m,keywords));case.json(metadata=m,keywords=keywords)
    # Export fixture arrays and blocks, independent from the encoder under test.
    for name,arr,metadata,gray,dither in [('color',a.astype('f4')/255,True,False,False),('gray',np.repeat(a[...,:1],3,2).astype('f4')/255,True,True,True),('plain',a.astype('f4')/255,False,False,True),('tiny',np.array([[[.2,.5,.7]]],dtype='f4'),False,False,False)]:
        with Case('codecs','export_'+name) as case:
            case.array('input',arr);frame=engine._neutral_gray(arr) if gray else arr
            case.array('u8',engine._quantize8_dithered(frame) if dither else engine._quantize(frame.copy(),255,np.uint8));case.array('u16',engine._quantize(frame.copy(),65535,np.uint16));case.array('rgb8',engine._quantize8_dithered(arr) if dither else engine._quantize(arr.copy(),255,np.uint8));case.array('rgb16',engine._quantize(arr.copy(),65535,np.uint16))
            exif=colorio.export_exif(source,RICH,(31,19),'sRGB');(case.folder/'exif.bin').write_bytes(exif);(case.folder/'xmp.bin').write_bytes(colorio.xmp_metadata(RICH,KEYWORDS,exif));(case.folder/'iptc.bin').write_bytes(colorio.iptc_bytes(RICH,KEYWORDS));case.json(metadata=RICH,keywords=KEYWORDS,keep_metadata=metadata,gray=gray,dither=dither)
            for quality in (0,1,35,50,95,100):
                pixels=engine._quantize8_dithered(frame) if dither else engine._quantize(frame.copy(),255,np.uint8)
                path=case.folder/f'expected_q{quality}.jpg'
                Image.fromarray(pixels[...,0] if gray else pixels).save(path,quality=quality,subsampling=0,
                    icc_profile=profile('Gray sRGB' if gray else 'sRGB'),exif=exif if metadata else b'')
                if metadata:colorio.jpeg_iptc(path,RICH,KEYWORDS)
    for working,destination in [('ProPhoto','sRGB'),('sRGB','Adobe RGB'),('ProPhoto','Display P3')]:
        name='export_cms_'+working+'_'+destination.replace(' ','_')
        with Case('codecs',name) as case:
            arr=a.astype('f4')/255;src=profile('Luma Wide' if working=='ProPhoto' else 'sRGB');dst=profile(destination);converted=colorio.convert(arr,src,dst)
            case.array('input',arr);case.array('cms_input_0',arr);case.array('cms_output_0',converted);case.array('output',np.clip(converted,0,1));case.array('u8',engine._quantize(np.clip(converted,0,1).copy(),255,np.uint8));(case.folder/'cms_source_0.icc').write_bytes(src);(case.folder/'cms_destination_0.icc').write_bytes(dst);case.json(working=working,destination=destination)
    for name,hdr in [('export_resized',False),('export_hdr',True)]:
        with Case('codecs',name) as case:
            arr=a.astype('f4')/(100 if hdr else 255);side=engine.export_longest(arr.shape,(13,8,'inside'));resized=engine.resize_export(arr,side,hdr)
            case.array('input',arr);case.array('output',resized);case.array('u8',engine._quantize(resized.copy(),255,np.uint8));case.json(hdr=hdr,limit=[13,8,'inside'])
    jpeg_marker_goldens()
    jpeg_golden_matrix()
    total=sum(p.stat().st_size for p in BASE.rglob('*') if p.is_file())
    assert total < 10_000_000, f'codec goldens too large: {total}'
    print('Generated',len(list(BASE.glob('*/case.json'))),'codec cases,',total,'bytes')

if __name__ == '__main__':
    if len(sys.argv)>1 and sys.argv[1]=='--check': check_exports(Path(sys.argv[2]))
    else: generate()
