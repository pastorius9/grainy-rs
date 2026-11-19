"""Goldens for RAW files through grainy_engine::files (load_photo, export_photo, photo_info) and grainy-cli.

Oracle functions: luma/engine.py `load_image` (the RAW branch: metadata dictionary, native `rawcolor.decode` path,
legacy `postprocess` path, `resize_float`) and `export_image`; luma/app.py `import_reason` for the error texts.

Sources (golden/rawload/_files/): the oracle repository's own assets/test-triple-illuminant.dng and small synthetic
DNGs made with gen_raw.py's `write_dng` (four Bayer patterns with the orientations 1/3/6/8, a linear DNG, a dual
calibrated linear DNG, a file with a non-ASCII name, files LibRaw refuses). No camera sample is downloaded and no photo
of the owner is read. Every file is stored once and shared by the cases, so `size_mb` and `file_fingerprint['size']`
are reproducible; the fingerprint's path and modification time belong to the checkout and are compared live.

Cases:
  load_<file>_<mode>_<space>_<max>   engine.load_image(path, max_size, working_space, raw_options)
       pixels.npy (float32; every `stride`-th pixel when the frame is large) and metadata (case.json).
       mode: legacy | as_shot | negative (a negative forces the native path); space: sRGB | ProPhoto;
       max: full (None) | m<N> (max_size N). Half-size development is used where the longest side is more than
       twice max_size, which the case file records as `half`.
  export_<name>                      engine.export_image of a RAW source: expected.npy (decoded output file),
       quantised.npy (the integers handed to the encoder), as gen_cli.py stores them.
  error_<name>                       a file the oracle cannot decode: its message and `import_reason`.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_rawload.py
"""
import sys
sys.dont_write_bytecode = True
import json
import os
import shutil
import tempfile
import traceback
from pathlib import Path

from common import GOLDEN, ORACLE, np, settings, write_json

import imagecodecs
import tifffile
from PIL import Image

from luma import engine
from gen_raw import write_dng

BASE = GOLDEN / 'rawload'
FILES = BASE / '_files'

# name -> (file name, write_dng options or None for the oracle's own asset)
SOURCES = {
    'bayer0': ('bayer0.dng', dict(pattern=0, bits=16, black=1024, orientation=1)),
    'bayer1': ('bayer1.dng', dict(pattern=1, bits=14, black=256, dual=True, orientation=3)),
    'bayer2': ('bayer2.dng', dict(pattern=2, bits=12, black=64, orientation=6)),
    'bayer3': ('bayer3.dng', dict(pattern=3, bits=16, black=1024, dual=True, forward=True, orientation=8)),
    'linear': ('linear.dng', dict(linear=True)),
    'lineardual': ('lineardual.dng', dict(linear=True, dual=True, forward=True)),
    'triple': ('triple.dng', None),
    'unicode': ('합성 사진 №1.dng', dict(pattern=0, bits=16, black=1024)),
}
BROKEN = {
    'integer_deflate': ('integer_deflate.dng', dict(compression='deflate')),
}


def make_files():
    FILES.mkdir(parents=True, exist_ok=True)
    for name, (file, options) in {**SOURCES, **BROKEN}.items():
        path = FILES / file
        if options is None:
            shutil.copyfile(ORACLE / 'assets' / 'test-triple-illuminant.dng', path)
        else:
            write_dng(path, **options)
    (FILES / 'junk.dng').write_bytes(bytes(range(256)) * 20)
    (FILES / 'junk.nef').write_bytes(bytes(range(256)) * 20)
    (FILES / 'junk.cr2').write_bytes(b'')
    for path in FILES.iterdir():
        os.utime(path, ns=(1700000000000000000, 1700000000000000000))


MODES = {
    'legacy': dict(raw_mode='legacy'),
    'as_shot': dict(raw_mode='as_shot'),
    'negative': dict(raw_mode='legacy', negative=True, negative_base=[.8, .7, .6], negative_peak=[.02, .03, .01]),
}
SPACES = ['sRGB', 'ProPhoto']
FULL_FRAMES = {'bayer0'}      # these keep every pixel of the full-size frame, the others every second one


def import_reason(path, error):
    """luma.app.import_reason without importing Qt: its two sentences for LibRawFileUnsupportedError."""
    last = ''.join(traceback.format_exception(type(error), error, None)).strip().splitlines()[-1]
    if 'LibRawFileUnsupportedError' not in last:
        return last
    if Path(path).suffix.lower() in ('.nef', '.nrw'):
        return '이 압축 방식은 지원하지 않습니다. 니콘의 고효율(HE) RAW라면 카메라에서 "무손실 압축"으로 찍거나 DNG로 변환해 주세요.'
    return '지원하지 않는 RAW 형식입니다.'


def metadata_json(metadata):
    result = json.loads(json.dumps(metadata, default=lambda v: v.tolist() if hasattr(v, 'tolist') else str(v)))
    result['file_fingerprint'].pop('path')      # a checkout path; the Rust test compares the live one
    result['file_fingerprint'].pop('mtime_ns')  # git does not keep modification times
    return result


def load_cases():
    cases = []
    for name in SOURCES:
        sizes = [None, 40, 16] + ([79, 39] if name == 'bayer0' else [])
        for mode in MODES:
            for space in SPACES:
                for size in sizes:
                    cases.append((name, mode, space, size))
    for name, mode, space, size in cases:
        tag = 'full' if size is None else f'm{size}'
        folder = BASE / f'load_{name}_{mode}_{space}_{tag}'
        folder.mkdir(parents=True, exist_ok=True)
        path = FILES / SOURCES[name][0]
        options = settings(working_space=space, **MODES[mode])
        pixels, metadata = engine.load_image(path, size, space, options)
        array = np.ascontiguousarray(np.asarray(pixels, dtype=np.float32))
        stride = 2 if (size is None and name not in FULL_FRAMES) else 1
        np.save(folder / 'pixels.npy', np.ascontiguousarray(array[::stride, ::stride]))
        write_json(folder / 'case.json', dict(file=SOURCES[name][0], max_size=size, working_space=space, mode=mode,
                                              settings=MODES[mode], stride=stride, shape=list(array.shape),
                                              native=bool(metadata.get('raw_native')),
                                              type=type(pixels).__name__, metadata=metadata_json(metadata)))
    return [f'load_{n}_{m}_{s}_{"full" if z is None else f"m{z}"}' for n, m, s, z in cases]


MIX = dict(exposure=.4, contrast=20., highlights=-40., shadows=30., saturation=15., vibrance=20., temperature=8.,
           tint=-4., sharpen=40., clarity=20.)
EXPORTS = []


def export_case(name, source, settings_dict, **options):
    EXPORTS.append((name, source, settings_dict, dict(dict(format='PNG 16-bit', quality=95, longest=None,
                                                           color_space='sRGB'), **options)))


for _mode in ('legacy', 'as_shot', 'negative'):
    export_case(f'export_bayer0_{_mode}_png16', 'bayer0', dict(MODES[_mode]))
    export_case(f'export_bayer2_{_mode}_mix_tiff8', 'bayer2', dict(MODES[_mode], **MIX), format='TIFF 8-bit')
export_case('export_bayer1_as_shot_wide_png16', 'bayer1', dict(raw_mode='as_shot', working_space='ProPhoto', saturation=20.))
export_case('export_bayer1_legacy_wide_p3', 'bayer1', dict(raw_mode='legacy', working_space='ProPhoto', saturation=20.),
            format='TIFF 16-bit', color_space='Display P3')
export_case('export_bayer3_as_shot_flip_longest', 'bayer3', dict(raw_mode='as_shot'), format='PNG', longest=48)
export_case('export_bayer3_legacy_flip_longest', 'bayer3', dict(raw_mode='legacy'), format='PNG', longest=48)
export_case('export_triple_as_shot_film_tiff16', 'triple', dict(raw_mode='as_shot', film_look='kodak-portra-400', film_strength=70.),
            format='TIFF 16-bit')
export_case('export_triple_legacy_jpeg', 'triple', dict(raw_mode='legacy', exposure=.3), format='JPEG', quality=90)
export_case('export_linear_as_shot_tiff8_mix', 'linear', dict(MODES['as_shot'], **MIX), format='TIFF 8-bit')
export_case('export_lineardual_negative_png16', 'lineardual', dict(MODES['negative']))
export_case('export_unicode_name_png', 'unicode', dict(raw_mode='as_shot'), format='PNG')

CAPTURED = []
_q, _qd = engine._quantize, engine._quantize8_dithered
engine._quantize = lambda *a, **k: CAPTURED.append(_q(*a, **k).copy()) or CAPTURED[-1]
engine._quantize8_dithered = lambda *a, **k: CAPTURED.append(_qd(*a, **k).copy()) or CAPTURED[-1]
EXTENSION = {'JPEG': '.jpg', 'PNG': '.png', 'PNG 16-bit': '.png', 'TIFF 8-bit': '.tif', 'TIFF 16-bit': '.tif'}


def decode(path, fmt):
    if fmt == 'PNG 16-bit':
        return imagecodecs.png_decode(path.read_bytes())
    if path.suffix in ('.tif', '.tiff'):
        return tifffile.imread(path)
    with Image.open(path) as image:
        return np.asarray(image).copy()


def export_cases(work):
    names = []
    for name, source, settings_dict, options in EXPORTS:
        folder = BASE / name
        folder.mkdir(parents=True, exist_ok=True)
        fmt = options['format']
        output = work / (name + EXTENSION[fmt])
        kwargs = dict(options, keep_metadata=options.get('keep_metadata', False))
        CAPTURED.clear()
        width, height = engine.export_image(FILES / SOURCES[source][0], output, dict(settings_dict), **kwargs)
        decoded = decode(output, fmt)
        np.save(folder / 'expected.npy', decoded)
        np.save(folder / 'quantised.npy', CAPTURED[-1])
        record = dict(file=SOURCES[source][0], settings=settings_dict, options=options, extension=EXTENSION[fmt],
                      width=width, height=height, lossless=fmt != 'JPEG', bytes=output.stat().st_size)
        if options.get('keep_metadata'):
            record['read_metadata'] = engine.read_metadata(output)
        write_json(folder / 'case.json', record)
        names.append(name)
    return names


def error_cases():
    names = []
    for name, file in [('integer_deflate', 'integer_deflate.dng'), ('junk_dng', 'junk.dng'), ('junk_nef', 'junk.nef'),
                       ('empty_cr2', 'junk.cr2')]:
        folder = BASE / f'error_{name}'
        folder.mkdir(parents=True, exist_ok=True)
        path = FILES / file
        record = dict(file=file)
        for mode in ('legacy', 'as_shot'):
            try:
                engine.load_image(path, None, 'sRGB', settings(**MODES[mode]))
                record[mode] = dict(loaded=True)
            except Exception as error:
                record[mode] = dict(loaded=False, error_class=type(error).__name__, message=str(error),
                                    reason=import_reason(path, error))
        write_json(folder / 'case.json', record)
        names.append(f'error_{name}')
    return names


def generate():
    if BASE.exists():
        shutil.rmtree(BASE)
    BASE.mkdir(parents=True)
    make_files()
    loads = load_cases()
    with tempfile.TemporaryDirectory() as tmp:
        exports = export_cases(Path(tmp))
    errors = error_cases()
    # info: metadata of load_image at 64 px with default settings, as photo_info reports it
    infos = {}
    for name in SOURCES:
        _, metadata = engine.load_image(FILES / SOURCES[name][0], 64, 'sRGB', settings())
        infos[name] = metadata_json(metadata)
    write_json(BASE / '_index.json', dict(loads=loads, exports=exports, errors=errors, info=infos,
                                          files={k: v[0] for k, v in SOURCES.items()}))
    total = sum(p.stat().st_size for p in BASE.rglob('*') if p.is_file())
    print(len(loads), 'load cases,', len(exports), 'exports,', len(errors), 'errors;', total, 'bytes')


if __name__ == '__main__':
    generate()
