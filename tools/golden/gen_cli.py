"""Goldens for grainy-cli / grainy_engine::files: whole exports through luma/engine.py `export_image`.

Small generated sources (PNG and TIFF, 8 and 16 bit, with and without an embedded ICC profile, one grey, one JPEG)
are written to golden/cli/_sources/ from the standard `garden` input; each case calls the oracle's `export_image`
with a settings dict and export options, decodes the file the oracle wrote and stores
  expected.npy   the decoded pixels of the oracle's output (Pillow / imagecodecs / tifffile),
  quantised.npy  the integer samples the oracle handed to the encoder (captured from engine._quantize*),
  the ICC profile, EXIF, XMP and IPTC blocks found in the file, and `read_metadata` of the file.
For AVIF and JPEG XL the Rust side cannot decode: `gen_cli.py --check DIR` decodes the files the Rust test wrote
(DIR/manifest.json) with Pillow / imagecodecs and compares them with these goldens.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_cli.py
"""
import sys
sys.dont_write_bytecode = True
import hashlib
import io
import json
import os
import struct
import tempfile
from pathlib import Path

from common import GOLDEN, inputs, np, write_json

import imagecodecs
import piexif
import tifffile
from PIL import Image

from luma import colorio, engine, rasterio

BASE = GOLDEN / 'cli'


# ICC creation time and profile id are frozen so the generator is deterministic (as gen_codecs.py does).
def freeze_profile(data):
    data = bytearray(data)
    data[24:36] = struct.pack('>6H', 2026, 1, 1, 0, 0, 0)
    data[84:100] = bytes(16)
    return bytes(data)


_cms_profile = imagecodecs.cms_profile
imagecodecs.cms_profile = lambda *a, **k: freeze_profile(_cms_profile(*a, **k))
_profile = colorio.profile
colorio.profile = rasterio.profile = lambda name='sRGB': freeze_profile(_profile(name))

# The integer samples handed to the encoder.
CAPTURED = []
_q, _qd = engine._quantize, engine._quantize8_dithered
engine._quantize = lambda *a, **k: CAPTURED.append(_q(*a, **k).copy()) or CAPTURED[-1]
engine._quantize8_dithered = lambda *a, **k: CAPTURED.append(_qd(*a, **k).copy()) or CAPTURED[-1]

RICH = dict(title='A & B <film>', creator='Zoe Writer', copyright='(c) 2026', caption='Line 1\nLine 2 & <3>',
            city='Seoul', location='Rooftop', country='KR', DateTimeOriginal='2024:02:03 04:05:06',
            latitude=-33.8587, longitude=151.214)
KEYWORDS = ' travel, film|tokyo, ,a&b '


def source_exif(orientation=1):
    return piexif.dump({'0th': {271: b'Camera Co', 272: b'Model 42', 274: orientation, 315: b'Old Author', 33432: b'Old copyright', 282: (300, 1), 283: (300, 1), 296: 2},
        'Exif': {33434: (1, 125), 33437: (28, 10), 34855: 400, 37386: (50, 1), 37382: (3, 2), 36867: b'2020:01:02 03:04:05', 42036: b'Lens 50', 42033: b'SERIAL42', 37500: b'MAKERNOTE', 40965: 1},
        'GPS': {1: b'N', 2: ((37, 1), (33, 1), (30, 1)), 3: b'W', 4: ((122, 1), (24, 1), (15, 1))},
        'Interop': {1: b'R98'}, '1st': {}, 'thumbnail': None})


# ---------------------------------------------------------------------------------------------- sources
SOURCES = BASE / '_sources'
ICC_DIR = BASE / '_icc'


def make_sources():
    SOURCES.mkdir(parents=True, exist_ok=True)
    garden = inputs()['garden']
    srgb = np.clip(engine.to_srgb(garden[::4, ::4].copy()), 0, 1)        # 40 x 60, display encoded
    a8 = np.round(srgb * 255).astype('uint8')
    a16 = np.round(srgb * 65535).astype('uint16')
    gray = np.round(srgb[..., 1] * 255).astype('uint8')
    gray16 = np.round(srgb[..., 1] * 65535).astype('uint16')
    p3, adobe = colorio.profile('Display P3'), colorio.profile('Adobe RGB')

    def png16(path, icc=None):
        with open(path, 'wb') as handle:
            if icc is None:
                handle.write(imagecodecs.png_encode(a16, level=6))
            else:
                rasterio.write_png16(handle, a16, icc)

    def tif(path, pixels, icc=None, photometric='rgb', extra=()):
        tags = list(extra)
        if icc is not None:
            tags.append((34675, 'B', len(icc), icc, False))
        tifffile.imwrite(path, pixels, photometric=photometric, metadata=None, extratags=tags)

    Image.fromarray(a8).save(SOURCES / 'png8.png')
    png16(SOURCES / 'png16.png')
    tif(SOURCES / 'tiff8.tif', a8)
    tif(SOURCES / 'tiff16.tif', a16)
    Image.fromarray(a8).save(SOURCES / 'png8_p3.png', icc_profile=p3)
    png16(SOURCES / 'png16_adobe.png', adobe)
    tif(SOURCES / 'tiff8_adobe.tif', a8, adobe)
    tif(SOURCES / 'tiff16_p3.tif', a16, p3)
    Image.fromarray(gray).save(SOURCES / 'gray8.png')
    tif(SOURCES / 'gray16.tif', gray16, photometric='minisblack')
    Image.fromarray(a8).save(SOURCES / 'png8_exif.png', exif=source_exif(6))
    Image.fromarray(a8).save(SOURCES / 'jpeg.jpg', quality=95, subsampling=0)
    # Sources the Rust side cannot colour-manage yet (grainy-cms exposes RGB transforms only): refused with a message.
    Image.fromarray(gray).save(SOURCES / 'gray8_icc.png', icc_profile=colorio.profile('Gray sRGB'))
    import zlib
    data = io.BytesIO()
    Image.fromarray(a8).save(data, format='PNG')
    raw = data.getvalue()
    (SOURCES / 'png8_gamma.png').write_bytes(raw[:33] + rasterio.png_chunk(b'gAMA', struct.pack('>I', 45455)) + raw[33:])
    Image.fromarray(a8).save(SOURCES / 'png8_badicc.png', icc_profile=b'this is not an ICC profile' * 8)
    for path in SOURCES.iterdir():
        os.utime(path, ns=(1700000000000000000, 1700000000000000000))


# ---------------------------------------------------------------------------------------------- settings
MIX = dict(exposure=.4, contrast=20., highlights=-40., shadows=30., whites=10., blacks=-10., saturation=15., vibrance=20.,
           temperature=8., tint=-4., sharpen=40., clarity=20., texture=15., vignette=-20.)
FILM = dict(film_look='kodak-portra-400', film_strength=70., exposure=.2)
WIDE = dict(working_space='ProPhoto', saturation=20., exposure=.3, contrast=10.)
OFF = dict(MIX, panels_off=['basic', 'detail', 'effects'])
NOISE = dict(noise_luma=40., noise_color=30., grain=40., grain_size=60., grain_roughness=70.)   # not bit-exact
SETTINGS = {'default': {}, 'mix': MIX, 'film': FILM, 'wide': WIDE, 'off': OFF, 'noise': NOISE,
            'mono': dict(monochrome=True, contrast=15.), 'film_wide': dict(FILM, working_space='ProPhoto')}
INEXACT = {'noise'}

OPTIONS = dict(format='PNG 16-bit', quality=95, longest=None, color_space='sRGB', keep_metadata=False, user_metadata=None,
               keywords='', grayscale=False, dither=False)
EXTENSION = {'JPEG': '.jpg', 'PNG': '.png', 'PNG 16-bit': '.png', 'TIFF 8-bit': '.tif', 'TIFF 16-bit': '.tif',
             'TIFF HDR 32-bit': '.tif', 'AVIF': '.avif', 'JPEG XL': '.jxl'}

CASES = []


def case(name, source, settings='default', **options):
    assert all(c[0] != name for c in CASES), name
    CASES.append((name, source, settings, dict(OPTIONS, **options)))


ALL_SOURCES = ['png8', 'png16', 'tiff8', 'tiff16', 'png8_p3', 'png16_adobe', 'tiff8_adobe', 'tiff16_p3', 'gray8', 'gray16', 'png8_exif']
# Every format with the default settings.
for fmt, tag in [('JPEG', 'jpeg95'), ('PNG', 'png'), ('PNG 16-bit', 'png16'), ('TIFF 8-bit', 'tiff8'), ('TIFF 16-bit', 'tiff16'), ('AVIF', 'avif'), ('JPEG XL', 'jxl')]:
    case(f'fmt_{tag}', 'png8', format=fmt)
case('fmt_jpeg50', 'png8', 'mix', format='JPEG', quality=50)
case('fmt_jpeg100', 'png8', 'mix', format='JPEG', quality=100)
case('fmt_avif_mix', 'png8_p3', 'mix', format='AVIF', quality=70)
# Every source, lossless 16-bit and 8-bit, default and mixed settings.
for source in ALL_SOURCES:
    case(f'src_{source}_png16', source, 'default', format='PNG 16-bit')
    case(f'src_{source}_tiff8_mix', source, 'mix', format='TIFF 8-bit')
case('src_jpeg_png16', 'jpeg', 'default', format='PNG 16-bit')
case('src_jpeg_tiff8_mix', 'jpeg', 'mix', format='TIFF 8-bit')
# Settings spread.
for key in ('mix', 'film', 'wide', 'off', 'noise', 'mono', 'film_wide'):
    for source in ('png16', 'png8_p3'):
        case(f'set_{key}_{source}_png16', source, key, format='PNG 16-bit')
    case(f'set_{key}_tiff8', 'png8', key, format='TIFF 8-bit')
case('set_film_jpeg', 'png8', 'film', format='JPEG')
# Sizes.
case('size_longest32_png', 'png8', 'mix', format='PNG', longest=32)
case('size_longest100_tiff16', 'png8', 'mix', format='TIFF 16-bit', longest=100)
case('size_box_inside', 'png16', 'mix', format='PNG 16-bit', longest=(40, 40, 'inside'))
case('size_box_cover', 'png16', 'mix', format='PNG 16-bit', longest=(40, 40, 'cover'))
case('size_box_open_height', 'png8', 'default', format='TIFF 8-bit', longest=(0, 30, 'inside'))
case('size_box_enlarge_refused', 'png8', 'default', format='PNG', longest=(500, 500, 'inside'))
case('size_oriented_longest', 'png8_exif', 'mix', format='PNG', longest=24)
case('size_jpeg', 'png8', 'mix', format='JPEG', longest=30, quality=85)
# Output colour spaces.
for space in ('sRGB', 'Display P3', 'Adobe RGB', 'ProPhoto RGB'):
    tag = space.replace(' ', '_')
    case(f'cs_{tag}_mix_png16', 'png8', 'mix', format='PNG 16-bit', color_space=space)
    case(f'cs_{tag}_wide_tiff16', 'png16_adobe', 'wide', format='TIFF 16-bit', color_space=space)
    case(f'cs_{tag}_film_tiff8', 'png8', 'film', format='TIFF 8-bit', color_space=space)
case('cs_p3_jpeg', 'png8', 'mix', format='JPEG', color_space='Display P3')
case('cs_adobe_avif', 'png8', 'default', format='AVIF', color_space='Adobe RGB', quality=70)
case('cs_jxl_p3_refused', 'png8', format='JPEG XL', color_space='Display P3')
# Grayscale and dither.
case('gray_png', 'gray8', format='PNG', grayscale=True)
case('gray_tiff16', 'gray16', format='TIFF 16-bit', grayscale=True)
case('gray_tiff8_mono_look', 'gray8', 'mono', format='TIFF 8-bit', grayscale=True)
case('gray_jpeg', 'gray8', format='JPEG', grayscale=True, quality=90)
case('gray_png16_ignored', 'gray8', format='PNG 16-bit', grayscale=True)
case('gray_colour_source', 'png8', format='PNG', grayscale=True)
case('gray_p3_ignored', 'gray8', format='PNG', grayscale=True, color_space='Display P3')
case('gray_off', 'gray8', format='PNG', grayscale=False)
case('gray_mix_tiff8', 'gray8', 'mix', format='TIFF 8-bit', grayscale=True)
case('dither_png', 'png8', format='PNG', dither=True)
case('dither_tiff8_mix', 'png16', 'mix', format='TIFF 8-bit', dither=True)
case('dither_gray', 'gray8', format='PNG', dither=True, grayscale=True)
case('dither_tiff16_unused', 'png16', 'mix', format='TIFF 16-bit', dither=True)
case('dither_jpeg', 'png8', 'mix', format='JPEG', dither=True)
# Metadata.
for fmt, tag in [('PNG', 'png'), ('PNG 16-bit', 'png16'), ('TIFF 8-bit', 'tiff8'), ('TIFF 16-bit', 'tiff16'), ('JPEG', 'jpeg'), ('AVIF', 'avif'), ('JPEG XL', 'jxl')]:
    case(f'meta_rich_{tag}', 'png8_exif', 'mix', format=fmt, keep_metadata=True, user_metadata=RICH, keywords=KEYWORDS)
    case(f'meta_source_{tag}', 'png8_exif', 'default', format=fmt, keep_metadata=True)
case('meta_rich_p3_tiff16', 'png8_exif', 'mix', format='TIFF 16-bit', keep_metadata=True, user_metadata=RICH, keywords=KEYWORDS, color_space='Display P3')
case('meta_gray_png', 'gray8', 'default', format='PNG', keep_metadata=True, user_metadata=RICH, keywords=KEYWORDS, grayscale=True)
case('meta_no_exif_source', 'png8', 'default', format='PNG', keep_metadata=True, user_metadata=dict(creator='Only Creator'))
case('meta_clear_gps', 'png8_exif', 'default', format='TIFF 8-bit', keep_metadata=True, user_metadata=dict(latitude=None, longitude=' '))
case('meta_off_png', 'png8_exif', 'default', format='PNG', keep_metadata=False, user_metadata=RICH, keywords=KEYWORDS)
# Refusals by the oracle itself.
case('badicc_png', 'png8_badicc', format='PNG')
case('hdr_tiff_without_hdr', 'png8', format='TIFF HDR 32-bit')


# ---------------------------------------------------------------------------------------------- decoding
def decode(path, fmt):
    if fmt == 'PNG 16-bit':
        return imagecodecs.png_decode(path.read_bytes())
    if path.suffix in ('.tif', '.tiff'):
        return tifffile.imread(path)
    if fmt == 'JPEG XL':
        return imagecodecs.jpegxl_decode(path.read_bytes())
    if fmt == 'AVIF':
        try:
            with Image.open(path) as im:
                return np.asarray(im.convert('RGB')).copy()
        except Exception:
            return imagecodecs.avif_decode(path.read_bytes())
    with Image.open(path) as im:
        return np.asarray(im).copy()


def jpeg_app13(data):
    pos = 2
    while pos + 4 <= len(data) and data[pos] == 0xFF:
        marker, length = data[pos + 1], int.from_bytes(data[pos + 2:pos + 4], 'big')
        if marker == 0xED:
            return data[pos + 4:pos + 2 + length]
        if marker == 0xDA:
            break
        pos += 2 + length
    return None


def blocks(path, fmt, folder):
    """ICC, EXIF, XMP and IPTC blocks found in the oracle's file."""
    found = {}
    icc = exif = xmp = iptc = None
    if path.suffix in ('.tif', '.tiff'):
        with tifffile.TiffFile(path) as tif:
            tags = tif.pages[0].tags
            icc = bytes(tags[34675].value)
            if 700 in tags:
                xmp = bytes(tags[700].value)
            if 33723 in tags:
                iptc = bytes(tags[33723].value)
    elif fmt != 'JPEG XL':
        with Image.open(path) as im:
            info = dict(im.info)
        icc = info.get('icc_profile')
        exif = info.get('exif')
        got = info.get('XML:com.adobe.xmp', info.get('xmp'))
        xmp = got.encode() if isinstance(got, str) else got
        if fmt == 'PNG 16-bit' and exif is None:
            exif = None
        if path.suffix == '.jpg':
            app13 = jpeg_app13(path.read_bytes())
            iptc = app13
    if icc is not None:
        digest = hashlib.sha1(icc).hexdigest()[:12]
        ICC_DIR.mkdir(parents=True, exist_ok=True)
        (ICC_DIR / f'{digest}.icc').write_bytes(icc)
        found['icc'] = digest
    for name, value in (('exif', exif), ('xmp', xmp), ('iptc', iptc)):
        if value is not None:
            (folder / f'{name}.bin').write_bytes(bytes(value).removeprefix(b'Exif\0\0') if name == 'exif' else bytes(value))
            found[name] = True
    return found


def run_case(name, source, settings_key, options, work):
    folder = BASE / name
    folder.mkdir(parents=True, exist_ok=True)
    for old in folder.iterdir():
        old.unlink()
    suffix = EXTENSION[options['format']]
    output = work / (name + suffix)
    src = next(SOURCES.glob(source + '.*'))
    kwargs = dict(options)
    if kwargs['longest'] is not None and not isinstance(kwargs['longest'], int):
        kwargs['longest'] = tuple(kwargs['longest'])
    record = dict(source=src.name, settings_key=settings_key, settings=SETTINGS[settings_key], options=options,
                  extension=suffix, exact=settings_key not in INEXACT)
    CAPTURED.clear()
    try:
        width, height = engine.export_image(src, output, dict(SETTINGS[settings_key]), **kwargs)
    except Exception as error:
        record['error'] = str(error)
        write_json(folder / 'case.json', record)
        return record
    record.update(width=width, height=height, lossless=options['format'] in ('PNG', 'PNG 16-bit', 'TIFF 8-bit', 'TIFF 16-bit'))
    decoded = decode(output, options['format'])
    np.save(folder / 'expected.npy', decoded)
    if CAPTURED:
        quantised = CAPTURED[-1]
        if decoded.ndim == 2 and quantised.ndim == 3 and quantised.shape[-1] == 1:
            quantised = quantised[..., 0]
        np.save(folder / 'quantised.npy', quantised)
        if record['lossless']:
            assert np.array_equal(quantised.reshape(decoded.shape) if quantised.size == decoded.size else quantised, decoded), name
    record['found'] = blocks(output, options['format'], folder)
    if output.suffix not in ('.avif', '.jxl'):
        try:
            record['read_metadata'] = engine.read_metadata(output)
        except Exception as error:
            record['read_metadata_error'] = str(error)
    record['bytes'] = output.stat().st_size
    write_json(folder / 'case.json', record)
    return record


def generate():
    if BASE.exists():
        import shutil
        shutil.rmtree(BASE)
    BASE.mkdir(parents=True)
    make_sources()
    records = {}
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        for name, source, settings_key, options in CASES:
            records[name] = run_case(name, source, settings_key, options, work)
    write_json(BASE / '_index.json', dict(cases=[c[0] for c in CASES], sources=sorted(p.name for p in SOURCES.iterdir())))
    total = sum(p.stat().st_size for p in BASE.rglob('*') if p.is_file())
    errors = {k: v['error'] for k, v in records.items() if 'error' in v}
    print(len(CASES), 'cases,', total, 'bytes;', 'oracle errors:', errors)


# ---------------------------------------------------------------------------------------------- --check
def check(folder):
    """Decode the AVIF / JPEG XL / JPEG files the Rust test wrote and compare them with the goldens."""
    manifest = json.loads((folder / 'manifest.json').read_text(encoding='utf-8'))
    report = []
    for row in manifest:
        path = folder / row['file']
        case = BASE / row['case']
        meta = json.loads((case / 'case.json').read_text(encoding='utf-8'))
        fmt = meta['options']['format']
        quantised = np.load(case / 'quantised.npy')
        decoded = decode(path, fmt)
        if decoded.ndim == 3 and quantised.ndim == 2:
            quantised = quantised[..., None].repeat(3, -1)
        if decoded.ndim == 2 and quantised.ndim == 3:
            quantised = quantised[..., 0]
        diff = np.abs(decoded.astype(np.int64) - quantised.astype(np.int64))
        oracle = np.abs(np.load(case / 'expected.npy').astype(np.int64) - quantised.astype(np.int64)) if (case / 'expected.npy').exists() else None
        entry = dict(case=row['case'], format=fmt, shape_ok=list(decoded.shape) == list(quantised.shape),
                     max_error_vs_quantised=int(diff.max()), mean_error=float(diff.mean()),
                     oracle_max_error=int(oracle.max()) if oracle is not None and oracle.shape == diff.shape else None,
                     oracle_mean_error=float(oracle.mean()) if oracle is not None and oracle.shape == diff.shape else None)
        if fmt == 'JPEG XL':
            assert entry['max_error_vs_quantised'] == 0, entry          # the Rust encoder is lossless
            data = path.read_bytes()
            if meta['options']['keep_metadata']:
                pos, chunks = 12, {}
                while pos < len(data):
                    length = int.from_bytes(data[pos:pos + 4], 'big')
                    chunks[data[pos + 4:pos + 8]] = data[pos + 8:pos + length]
                    pos += length
                if (case / 'xmp.bin').exists():
                    assert chunks.get(b'xml ') == (case / 'xmp.bin').read_bytes(), (row, 'XMP')
        if fmt in ('AVIF', 'JPEG'):
            with Image.open(path) as im:
                icc = im.info.get('icc_profile')
            digest = meta['found'].get('icc')
            expected = (ICC_DIR / f'{digest}.icc').read_bytes() if digest else None
            same = icc is not None and expected is not None and icc[:24] == expected[:24] and icc[36:] == expected[36:]
            entry['icc_equal_except_time'] = bool(same)
            assert same, (row, 'ICC')
        if fmt == 'AVIF' and meta['options']['keep_metadata']:
            with Image.open(path) as im:
                info = dict(im.info)
            exif = (case / 'exif.bin').read_bytes() if (case / 'exif.bin').exists() else None
            if exif is not None and info.get('exif') is not None:
                # Pillow's AVIF writer drops the Orientation tag (it is 1) from the oracle's file; the Rust file keeps it.
                # and reduces rationals (28/10 -> 14/5); sub-IFD pointers differ with the layout.
                def values(raw):
                    from fractions import Fraction
                    data = piexif.load(raw)
                    out = {}
                    for table in ('0th', 'Exif', 'GPS'):
                        for tag, v in data[table].items():
                            if (table, tag) in (('0th', 274), ('0th', 34665), ('0th', 34853)):
                                continue
                            if isinstance(v, tuple) and len(v) == 2 and all(isinstance(n, int) for n in v):
                                v = Fraction(*v)
                            elif isinstance(v, tuple):
                                v = tuple(Fraction(*n) if isinstance(n, tuple) else n for n in v)
                            out[(table, tag)] = v
                    return out
                assert values(bytes(info['exif'])) == values(exif), (row, 'EXIF')
                entry['exif_equal_except_orientation_tag_pointers_and_reduced_rationals'] = True
            xmp = (case / 'xmp.bin').read_bytes() if (case / 'xmp.bin').exists() else None
            got = info.get('xmp')
            if xmp is not None and got is not None:
                assert (got if isinstance(got, bytes) else got.encode()) == xmp, (row, 'XMP')
                entry['xmp_equal'] = True
        report.append(entry)
    write_json(folder / 'check.json', report)
    print(json.dumps(report, indent=1))


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--check':
        check(Path(sys.argv[2]))
    else:
        generate()
