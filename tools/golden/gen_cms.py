"""Goldens for grainy-cms: built-in ICC profiles, LittleCMS conversions (float, 8-bit, 16-bit, planar 8-bit),
grey/CMYK/CIELAB transforms, calibrated PNG/TIFF and Lab profiles, engine.load_image on generated files,
output_rgb for every destination, arbitrary (test) ICC profiles, the transform cache, display profile checks
and the oracle's errors for invalid profile data.

Profile bytes carry their creation time (header bytes 24..36, the UTC time the oracle made them); everything
else must match byte for byte. Test profiles are generated here with LittleCMS (imagecodecs and the oracle's
own luma_lcms2 library through ctypes) and stored as bytes.
"""
import re
import ctypes as ct
import hashlib
import shutil
import tempfile
from pathlib import Path

from common import ORACLE, Case, inputs, np

import imagecodecs
from luma import colorio, display_profiles, engine, native_color

assert native_color.available(), 'the oracle native LittleCMS library is needed'
ins = inputs()
rng = np.random.default_rng(20261113)

BUILTIN = ['sRGB', 'Adobe RGB', 'Display P3', 'ProPhoto RGB', 'Linear sRGB', 'Linear ProPhoto', 'Luma Wide',
           'Gray sRGB']
RGB = [name for name in BUILTIN if name != 'Gray sRGB']


def slug(name):
    return name.lower().replace(' ', '_')


def as_u8(data):
    return np.frombuffer(data, np.uint8).copy()


def mask_temp(message):
    """Replace the machine's temporary folder (repr-escaped or not) with a fixed `<tmp>` placeholder."""
    return re.sub(r"[A-Za-z]:(?:\\\\|\\|/)[^']*?gen_cms_[a-z0-9_]+", '<tmp>', message)


def error_of(call):
    """(kind, message) of the oracle's exception, or None when the call succeeds."""
    try:
        call()
    except Exception as error:   # noqa: BLE001 - every oracle error is recorded
        return [type(error).__name__, mask_temp(str(error))]
    return None


# ---------------------------------------------------------------- built-in profiles
for name in BUILTIN:
    with Case('cms', f'profile_{slug(name)}') as case:
        data = colorio.profile(name)
        case.json(name=name, size=len(data))
        case.array('bytes', as_u8(data))
with Case('cms', 'profile_pillow_srgb') as case:   # engine.srgb_profile (Pillow's LittleCMS 2.19)
    data = engine.srgb_profile()
    case.json(size=len(data))
    case.array('bytes', as_u8(data))


# ---------------------------------------------------------------- test profiles (arbitrary ICC data)
def lcms_library():
    lib = ct.WinDLL(str(ORACLE / 'assets' / 'native' / 'luma_lcms2.dll'))
    v = ct.c_void_p
    for name, restype, argtypes in [
        ('cmsCreateProfilePlaceholder', v, [v]), ('cmsSetDeviceClass', None, [v, ct.c_uint32]),
        ('cmsSetColorSpace', None, [v, ct.c_uint32]), ('cmsSetPCS', None, [v, ct.c_uint32]),
        ('cmsPipelineAlloc', v, [v, ct.c_uint32, ct.c_uint32]),
        ('cmsStageAllocCLut16bit', v, [v, ct.c_uint32, ct.c_uint32, ct.c_uint32, v]),
        ('cmsPipelineInsertStage', ct.c_int, [v, ct.c_int, v]), ('cmsPipelineFree', None, [v]),
        ('cmsWriteTag', ct.c_int, [v, ct.c_uint32, v]), ('cmsD50_XYZ', v, []),
        ('cmsMLUalloc', v, [v, ct.c_uint32]), ('cmsMLUsetASCII', ct.c_int, [v, ct.c_char_p, ct.c_char_p, ct.c_char_p]),
        ('cmsMLUfree', None, [v]), ('cmsSaveProfileToMem', ct.c_int, [v, v, ct.POINTER(ct.c_uint32)]),
        ('cmsCloseProfile', ct.c_int, [v])]:
        function = getattr(lib, name)
        function.restype = restype
        function.argtypes = argtypes
    return lib


def sig(text):
    return int.from_bytes(text.encode('ascii'), 'big')


def lut_profile(matrix_profile, grid=17):
    """An RGB display profile with A2B0/B2A0 CLUTs (PCS Lab) sampled from a matrix/TRC profile."""
    lib = lcms_library()
    lab4 = imagecodecs.cms_profile('lab4')
    levels = np.round(np.linspace(0, 1, grid) * 65535).astype(np.uint16)
    nodes = np.stack(np.meshgrid(levels, levels, levels, indexing='ij'), -1).reshape(1, -1, 3)   # first input slowest
    a2b = imagecodecs.cms_transform(np.ascontiguousarray(nodes), matrix_profile, lab4, colorspace='rgb',
                                    outcolorspace='lab', outdtype='uint16', intent=1)
    b2a = imagecodecs.cms_transform(np.ascontiguousarray(nodes), lab4, matrix_profile, colorspace='lab',
                                    outcolorspace='rgb', outdtype='uint16', intent=1)
    handle = lib.cmsCreateProfilePlaceholder(None)
    lib.cmsSetDeviceClass(handle, sig('mntr'))
    lib.cmsSetColorSpace(handle, sig('RGB '))
    lib.cmsSetPCS(handle, sig('Lab '))
    for tag, table in (('A2B0', a2b), ('B2A0', b2a)):
        table = np.ascontiguousarray(table.reshape(-1), np.uint16)
        pipeline = lib.cmsPipelineAlloc(None, 3, 3)
        stage = lib.cmsStageAllocCLut16bit(None, grid, 3, 3, table.ctypes.data)
        assert stage and lib.cmsPipelineInsertStage(pipeline, 1, stage)
        assert lib.cmsWriteTag(handle, sig(tag), pipeline)
        lib.cmsPipelineFree(pipeline)
    assert lib.cmsWriteTag(handle, sig('wtpt'), lib.cmsD50_XYZ())
    mlu = lib.cmsMLUalloc(None, 1)
    lib.cmsMLUsetASCII(mlu, b'en', b'US', b'Grainy test LUT RGB')
    assert lib.cmsWriteTag(handle, sig('desc'), mlu)
    lib.cmsMLUfree(mlu)
    size = ct.c_uint32(0)
    assert lib.cmsSaveProfileToMem(handle, None, ct.byref(size))
    buffer = ct.create_string_buffer(size.value)
    assert lib.cmsSaveProfileToMem(handle, buffer, ct.byref(size))
    lib.cmsCloseProfile(handle)
    return buffer.raw[:size.value]


matrix_test = imagecodecs.cms_profile('rgb', whitepoint=(.3127, .329), primaries=(.66, .33, .28, .63, .155, .07),
                                      gamma=2.4)
x = np.linspace(0, 1, 1024)
curves = np.round(np.stack([x ** 1.8, x ** 2.2, x ** 2.6]) * 65535).astype(np.uint16)
table_test = imagecodecs.cms_profile('rgb', whitepoint=(.3457, .3585), primaries=(.70, .29, .17, .78, .13, .04),
                                     transferfunction=curves)
lut_test = lut_profile(matrix_test)
CUSTOM = {'matrix_gamma24': matrix_test, 'table_curves': table_test, 'lut_clut17': lut_test}
for name, data in CUSTOM.items():
    with Case('cms', f'custom_{name}') as case:
        case.json(size=len(data), info=imagecodecs.cms_info(data))
        case.array('bytes', as_u8(data))

PROFILES = {name: colorio.profile(name) for name in BUILTIN}
PROFILES.update(CUSTOM)


# ---------------------------------------------------------------- inputs
def hand_rgb():
    values = [-1.0, -0.5, -1e-6, -0.0, 0.0, 1e-7, 1e-3, .04045, .18, .5, .999, 1.0, 1.0000001, 1.5, 2.0, 6.0]
    greys = [[v, v, v] for v in values]
    greys += [[1, 0, 0], [0, 1, 0], [0, 0, 1], [1, 1, 0], [0, 1, 1], [1, 0, 1], [0, 0, 0], [1, 1, 1]]
    mixed = np.asarray(values, np.float32)[rng.integers(0, len(values), (72, 3))]
    return np.ascontiguousarray(np.concatenate([np.asarray(greys, np.float32), mixed]).reshape(4, 24, 3),
                                np.float32)


hand = hand_rgb()
mix = np.ascontiguousarray(np.concatenate(
    [ins[name][::4, ::4] for name in ('colours', 'patches', 'wide', 'noise')] + [hand]), np.float32)   # 68 x 24
special = np.array([[[np.nan, 0, 0], [0, np.nan, .5], [np.inf, 0, 0], [-np.inf, .5, .5], [np.nan] * 3,
                     [np.inf] * 3, [1e30, 0, 0], [-1e30, 1, 1]]], np.float32)

cube8 = np.round(np.linspace(0, 255, 8)).astype(np.uint8)
cube16 = np.round(np.linspace(0, 65535, 8)).astype(np.uint16)
grid8 = np.stack(np.meshgrid(cube8, cube8, cube8, indexing='ij'), -1).reshape(-1, 3)
grid16 = np.stack(np.meshgrid(cube16, cube16, cube16, indexing='ij'), -1).reshape(-1, 3)
grey8 = np.repeat(np.arange(256, dtype=np.uint8)[:, None], 3, 1)
grey16 = np.repeat((np.arange(256, dtype=np.uint32) * 257 + rng.integers(0, 257, 256)).clip(0, 65535)
                   .astype(np.uint16)[:, None], 3, 1)
u8 = np.ascontiguousarray(np.concatenate([grid8, grey8, rng.integers(0, 256, (256, 3))]).astype(np.uint8)
                          .reshape(32, 32, 3))
u16 = np.ascontiguousarray(np.concatenate([grid16, grey16, rng.integers(0, 65536, (256, 3))]).astype(np.uint16)
                           .reshape(32, 32, 3))
with Case('cms', 'inputs') as case:
    case.array('mix', mix)
    case.array('hand', hand)
    case.array('special', special)
    case.array('u8', u8)
    case.array('u16', u16)

# ---------------------------------------------------------------- convert between every pair of built-in spaces
for source in RGB:
    for destination in RGB:
        src, dst = PROFILES[source], PROFILES[destination]
        with Case('cms', f'convert_{slug(source)}__{slug(destination)}') as case:
            case.json(source=source, destination=destination)
            out = colorio.convert(mix, src, dst)
            assert out.dtype == np.float32
            case.array('f32', out)
            # The imagecodecs paths of colorio.convert: integer input or output (intent 1, flags 0).
            case.array('u8_u8', colorio.convert(u8, src, dst, 'uint8'))
            case.array('u16_u16', colorio.convert(u16, src, dst, 'uint16'))
            if source in ('sRGB', 'Adobe RGB', 'Display P3'):
                case.array('u8_f32', colorio.convert(u8, src, dst))
                case.array('u16_f32', colorio.convert(u16, src, dst))
                case.array('f32_u16', colorio.convert(mix, src, dst, 'uint16'))
                case.array('f32_u8', colorio.convert(mix, src, dst, 'uint8'))
# NaN / infinity / huge values on a few pairs
with Case('cms', 'convert_special') as case:
    pairs = [('sRGB', 'Display P3'), ('Luma Wide', 'sRGB'), ('Linear ProPhoto', 'Linear sRGB'), ('sRGB', 'sRGB')]
    case.json(pairs=pairs)
    for i, (source, destination) in enumerate(pairs):
        case.array(f'out{i}', colorio.convert(special, PROFILES[source], PROFILES[destination]))
# The full-size standard inputs for the two conversions the app runs most (stored as outputs only).
for name in ('colours', 'wide'):
    with Case('cms', f'convert_full_{name}') as case:
        case.json(input=name)
        case.array('luma_wide__srgb', colorio.convert(ins[name], PROFILES['Luma Wide'], PROFILES['sRGB']))
        case.array('linear_prophoto__linear_srgb',
                   colorio.convert(ins[name], PROFILES['Linear ProPhoto'], PROFILES['Linear sRGB']))

# ---------------------------------------------------------------- custom profiles, both directions
for name, data in CUSTOM.items():
    with Case('cms', f'convert_custom_{name}') as case:
        for other in ('sRGB', 'Luma Wide', 'Linear sRGB'):
            case.array(f'to__{slug(other)}', colorio.convert(mix, data, PROFILES[other]))
            case.array(f'from__{slug(other)}', colorio.convert(mix, PROFILES[other], data))
        case.array('u8_to__srgb', colorio.convert(u8, data, PROFILES['sRGB'], 'uint8'))
        case.array('u16_from__srgb', colorio.convert(u16, PROFILES['sRGB'], data, 'uint16'))

# ---------------------------------------------------------------- convert8_planar (8-bit in, float planes out)
for source in RGB + list(CUSTOM):
    for destination in ('Linear sRGB', 'Linear ProPhoto'):
        with Case('cms', f'planar8_{slug(source)}__{slug(destination)}') as case:
            case.json(source=source, destination=destination)
            planes = native_color.convert8_planar(u8, PROFILES[source], PROFILES[destination])
            assert planes.dtype == np.float32 and planes.shape == (3, 32, 32)
            case.array('planes', planes)

# ---------------------------------------------------------------- output_rgb, both working spaces, every destination
garden = np.ascontiguousarray(ins['garden'][::2, ::2], np.float32) if 'garden' in ins else None
for working in ('sRGB', 'ProPhoto'):
    for destination in RGB + list(CUSTOM):
        target = CUSTOM.get(destination, destination)
        with Case('cms', f'output_{slug(working)}__{slug(destination)}') as case:
            case.json(working_space=working, destination=destination, custom=destination in CUSTOM)
            out = engine.output_rgb(mix, working, target)
            assert out.dtype == np.float32
            case.array('output', out)
    if garden is not None:
        with Case('cms', f'output_garden_{slug(working)}') as case:
            case.array('input', garden)
            case.array('display_p3', engine.output_rgb(garden, working, 'Display P3'))
            case.array('adobe_rgb', engine.output_rgb(garden, working, 'Adobe RGB'))

# ---------------------------------------------------------------- display_rgb
with Case('cms', 'display_rgb') as case:
    case.array('none', colorio.display_rgb(mix, None))
    case.array('display_p3', colorio.display_rgb(mix, PROFILES['Display P3']))
    case.array('matrix_gamma24', colorio.display_rgb(mix, CUSTOM['matrix_gamma24']))

# ---------------------------------------------------------------- invalid profile data and the oracle's errors
srgb = PROFILES['sRGB']
bad = {
    'empty': b'',
    'short': srgb[:100],
    'truncated': srgb[:300],
    'garbage': rng.integers(0, 256, 600).astype(np.uint8).tobytes(),
    'bad_magic': srgb[:36] + b'xxxx' + srgb[40:],
    'gray': PROFILES['Gray sRGB'],
    'lab': imagecodecs.cms_profile('lab4'),
    'input_class': CUSTOM['matrix_gamma24'][:12] + b'scnr' + CUSTOM['matrix_gamma24'][16:],
}
folder = Path(tempfile.mkdtemp(prefix='gen_cms_'))
try:
    results = {}
    for name, data in bad.items():
        path = folder / f'{name}.icc'
        path.write_bytes(data)
        lin = PROFILES['Linear sRGB']
        results[name] = {
            'convert_to': error_of(lambda: colorio.convert(hand, srgb, data)),
            'convert_from': error_of(lambda: colorio.convert(hand, data, srgb)),
            'convert_u8': error_of(lambda: colorio.convert(u8, data, srgb, 'uint8')),
            'planar8': error_of(lambda: native_color.convert8_planar(u8, data, lin)),
            'profile_path': error_of(lambda: colorio.profile(str(path))),
            'checked': error_of(lambda: display_profiles._checked(data, 'name.icc')),
        }
    for name, data in bad.items():
        with Case('cms', f'bad_{name}') as case:
            case.json(errors=results[name])
            case.array('bytes', as_u8(data) if data else np.zeros(0, np.uint8))
    with Case('cms', 'errors_misc') as case:
        case.json(
            native_not_bytes=error_of(lambda: native_color.convert(hand, 'x', srgb)),
            native_shape=error_of(lambda: native_color.convert(hand[..., :2].copy(), srgb, srgb)),
            native_dtype=error_of(lambda: native_color.convert(hand.astype(np.float64), srgb, srgb)),
            planar8_dtype=error_of(lambda: native_color.convert8_planar(u16, srgb, srgb)),
            profile_missing=error_of(lambda: colorio.profile(str(folder / 'missing.icc'))),
        )

    # ------------------------------------------------------------ display profiles (pure parts)
    checked = {}
    for name, data in list(PROFILES.items()):
        result = error_of(lambda: display_profiles._checked(data, f'{name}.icc'))
        if result is None:
            _, description, digest = display_profiles._checked(data, f'{name}.icc')
            checked[name] = {'description': description, 'digest': digest}
        else:
            checked[name] = {'error': result}
    with Case('cms', 'display_checked') as case:
        case.json(checked=checked)

    def plain(profile):
        return {'mode': profile.mode, 'screen': profile.screen, 'path': profile.path, 'description': profile.description,
                'message': profile.message, 'error': profile.error, 'digest': profile.digest,
                'data_sha256': hashlib.sha256(profile.data).hexdigest() if profile.data else None}

    p3_file = folder / 'p3.icc'
    p3_file.write_bytes(PROFILES['Display P3'])
    lut_file = folder / 'lut.icc'
    lut_file.write_bytes(CUSTOM['lut_clut17'])
    resolved = {
        'off': plain(display_profiles.resolve('off', 'S1')),
        'manual_none': plain(display_profiles.resolve('manual', 'S1', '')),
        'manual_p3': plain(display_profiles.resolve('manual', 'S1', str(p3_file))),
        'manual_lut': plain(display_profiles.resolve('manual', 'S1', str(lut_file))),
        'manual_garbage': plain(display_profiles.resolve('manual', 'S1', str(folder / 'garbage.icc'))),
        'manual_gray': plain(display_profiles.resolve('manual', 'S1', str(folder / 'gray.icc'))),
        'bogus_mode': plain(display_profiles.resolve('bogus', 'S1', '')),
        'auto_windows_none': plain(display_profiles.resolve('auto', '', '')),
    }
    # The Windows lookup and the macOS branch with the system calls replaced (as the Rust port injects them).
    original = (display_profiles.windows_profile_path, display_profiles.mac_profile_data, display_profiles.MAC,
                display_profiles.SYSTEM)
    try:
        display_profiles.windows_profile_path = lambda screen: str(p3_file)
        resolved['auto_windows_p3'] = plain(display_profiles.resolve('auto', r'\\.\DISPLAY1', ''))

        def failing(screen):
            raise ValueError('Windows 모니터 이름을 확인할 수 없습니다.')
        display_profiles.windows_profile_path = failing
        resolved['auto_windows_error'] = plain(display_profiles.resolve('auto', 'bad', ''))
        display_profiles.MAC, display_profiles.SYSTEM = True, 'macOS'
        display_profiles.mac_profile_data = lambda screen: PROFILES['Display P3']
        resolved['auto_mac_p3'] = plain(display_profiles.resolve('auto', 'Studio Display (2)', ''))
        resolved['auto_mac_noname'] = plain(display_profiles.resolve('auto', ' (2)', ''))
        display_profiles.mac_profile_data = lambda screen: None
        resolved['auto_mac_none'] = plain(display_profiles.resolve('auto', 'Studio Display (2)', ''))
        display_profiles.mac_profile_data = lambda screen: bad['garbage']
        resolved['auto_mac_garbage'] = plain(display_profiles.resolve('auto', 'Studio Display (2)', ''))
        resolved['manual_mac_p3'] = plain(display_profiles.resolve('manual', 'Studio Display (2)', str(p3_file)))
        resolved['off_mac'] = plain(display_profiles.resolve('off', 'Studio Display (2)', ''))
    finally:
        (display_profiles.windows_profile_path, display_profiles.mac_profile_data, display_profiles.MAC,
         display_profiles.SYSTEM) = original
    for value in resolved.values():   # temporary paths differ per run: keep the file name only
        if value['path']:
            value['path'] = Path(value['path']).name
    with Case('cms', 'display_resolve') as case:
        case.json(resolved=resolved)
finally:
    shutil.rmtree(folder, ignore_errors=True)

# ---------------------------------------------------------------- the transform cache (bounded LRU, its counters)
pad = b'\0' * (10 * 1024 * 1024)
padded = [srgb + pad, PROFILES['Display P3'] + pad, PROFILES['Adobe RGB'] + pad]
names = RGB + list(CUSTOM)
steps = [(a, b, False) for a in names[:4] for b in names[:5]]            # 20 distinct pairs: evicts 4
steps += [(names[3], names[4], False), (names[0], names[0], False)]      # one hit, one evicted miss
steps += [(names[0], names[1], True), (names[0], names[1], True)]        # planar8 keys are separate
steps += [('pad0', 'pad1', False), ('pad1', 'pad2', False), ('pad0', 'pad1', False)]   # byte budget: 32 MB
labelled = dict(PROFILES)
labelled.update({f'pad{i}': data for i, data in enumerate(padded)})
native_color.clear_cache()
record = []
for source, destination, planar8 in steps:
    native_color._transform(labelled[source], labelled[destination], planar8)
    record.append({'source': source, 'destination': destination, 'planar8': planar8, **native_color.cache_info()})
with Case('cms', 'cache') as case:
    case.json(steps=record, max_transforms=native_color.MAX_TRANSFORMS, max_profile_bytes=native_color.MAX_PROFILE_BYTES,
              pad=len(pad))
native_color.clear_cache()
# ================================================================ second run: what loading image files needs
# Grey, CMYK and CIELAB sources (imagecodecs.cms_transform with colorspace='gray'|'cmyk'|'lab'), the calibrated
# profiles of rasterio._png_profile / _tiff_profile (imagecodecs.cms_profile('rgb'|'gray', whitepoint=...,
# primaries=..., gamma=...|transferfunction=...)), the Lab profiles, cms_info for them, and engine.load_image on
# generated PNG/TIFF files. No CMYK ICC file ships in the oracle's venv (no *.icc / *.icm in site-packages), so
# the CMYK profiles are built here with LittleCMS (as lut_profile above).
import struct
import zlib

import tifffile
from PIL import Image
from luma import rasterio
from common import ROOT

INFO_KEYS = ('colorspace', 'deviceclass', 'description', 'pcs', 'channels', 'version')


def info_of(data):
    details = imagecodecs.cms_info(data)
    return {key: details[key] for key in INFO_KEYS}


def lcms_cmyk_library():
    lib = lcms_library()
    lib.cmsStageAllocCLut16bit.argtypes = [ct.c_void_p, ct.c_uint32, ct.c_uint32, ct.c_uint32, ct.c_void_p]
    return lib


def ink_rgb(cmyk, gain):
    """A naive ink model (sRGB-encoded result) for test CMYK profiles; cmyk in 0..1."""
    c, m, y, k = (np.clip(cmyk[..., i], 0, 1) ** gain for i in range(4))
    return np.stack([(1 - c) * (1 - k), (1 - m) * (1 - k), (1 - y) * (1 - k)], -1).astype(np.float32)


def cmyk_profile(grid, gain, description, with_b2a=True, device_class='prtr'):
    """A CMYK printer profile with an A2B0 CLUT (and optionally B2A0), PCS Lab, made with LittleCMS."""
    lib = lcms_cmyk_library()
    srgb_icc, lab4 = colorio.profile('sRGB'), imagecodecs.cms_profile('lab4')
    levels = np.linspace(0, 1, grid)
    nodes = np.stack(np.meshgrid(levels, levels, levels, levels, indexing='ij'), -1).reshape(1, -1, 4)
    rgb = np.ascontiguousarray(ink_rgb(nodes, gain))
    a2b = imagecodecs.cms_transform(rgb, srgb_icc, lab4, colorspace='rgb', outcolorspace='lab', outdtype='uint16',
                                    intent=1)
    tables = [('A2B0', a2b, 4, 3)]
    if with_b2a:
        lgrid = 17
        lab_levels = np.round(np.linspace(0, 1, lgrid) * 65535).astype(np.uint16)
        lab_nodes = np.stack(np.meshgrid(lab_levels, lab_levels, lab_levels, indexing='ij'), -1).reshape(1, -1, 3)
        back = imagecodecs.cms_transform(np.ascontiguousarray(lab_nodes), lab4, srgb_icc, colorspace='lab',
                                         outcolorspace='rgb', outdtype='float32', intent=1)
        back = np.clip(np.nan_to_num(back), 0, 1)
        k = 1 - back.max(-1)
        safe = np.where(k < 1, 1 - k, 1)
        cmy = np.where(k[..., None] < 1, (1 - back - k[..., None]) / safe[..., None], 0)
        ink = np.concatenate([cmy, k[..., None]], -1) ** (1 / gain)
        b2a = np.round(np.clip(ink, 0, 1) * 65535).astype(np.uint16)
        tables.append(('B2A0', b2a, 3, 4))
    handle = lib.cmsCreateProfilePlaceholder(None)
    lib.cmsSetDeviceClass(handle, sig(device_class))
    lib.cmsSetColorSpace(handle, sig('CMYK'))
    lib.cmsSetPCS(handle, sig('Lab '))
    for tag, table, inputs_, outputs in tables:
        table = np.ascontiguousarray(table.reshape(-1), np.uint16)
        points = grid if inputs_ == 4 else 17
        pipeline = lib.cmsPipelineAlloc(None, inputs_, outputs)
        stage = lib.cmsStageAllocCLut16bit(None, points, inputs_, outputs, table.ctypes.data)
        assert stage and lib.cmsPipelineInsertStage(pipeline, 1, stage)
        assert lib.cmsWriteTag(handle, sig(tag), pipeline)
        lib.cmsPipelineFree(pipeline)
    assert lib.cmsWriteTag(handle, sig('wtpt'), lib.cmsD50_XYZ())
    mlu = lib.cmsMLUalloc(None, 1)
    lib.cmsMLUsetASCII(mlu, b'en', b'US', description.encode('ascii'))
    assert lib.cmsWriteTag(handle, sig('desc'), mlu)
    lib.cmsMLUfree(mlu)
    size = ct.c_uint32(0)
    assert lib.cmsSaveProfileToMem(handle, None, ct.byref(size))
    buffer = ct.create_string_buffer(size.value)
    assert lib.cmsSaveProfileToMem(handle, buffer, ct.byref(size))
    lib.cmsCloseProfile(handle)
    return buffer.raw[:size.value]


SRGB_TABLE = engine.to_linear(np.linspace(0, 1, 65536, dtype=np.float32))
assert SRGB_TABLE.dtype == np.float32
ramp256 = np.linspace(0, 1, 256)
CURVES3 = (np.round(np.stack([ramp256 ** 1.8, ramp256 ** 2.2, ramp256 ** 2.6]) * 65535)
           .astype(np.float32) / 65535).astype(np.float32)          # TIFF TransferFunction: uint16 / 65535
CURVE1 = CURVES3[1].copy()

# name -> (kind, whitepoint, primaries, gamma, transfer); the exact imagecodecs.cms_profile arguments the
# oracle's _png_profile / _tiff_profile pass for such tags.
D65, SRGB_PRIMARIES = (.3127, .329), (.64, .33, .30, .60, .15, .06)
CALIBRATED = {
    'png_rgb_gamma': ('rgb', D65, SRGB_PRIMARIES, 1 / (45455 / 100000), None),
    'png_rgb_gamma_chrm': ('rgb', (.3457, .3585), (.64, .33, .21, .71, .15, .06), 1 / (50000 / 100000), None),
    'png_rgb_chrm': ('rgb', (.3127, .329), (.68, .32, .265, .69, .15, .06), None, 'srgb'),
    'png_gray_gamma': ('gray', D65, None, 1 / (45455 / 100000), None),
    'png_gray_chrm': ('gray', (.31, .33), None, None, 'srgb'),
    'tiff_rgb_chroma': ('rgb', (.3127, .329), (.64, .33, .21, .71, .15, .06), None, 'srgb'),
    'tiff_rgb_curves3': ('rgb', D65, SRGB_PRIMARIES, None, 'curves3'),
    'tiff_rgb_curve1': ('rgb', D65, SRGB_PRIMARIES, None, 'curve1'),
    'tiff_gray_curve': ('gray', (.3457, .3585), None, None, 'curve1'),
}
TRANSFERS = {'srgb': SRGB_TABLE, 'curves3': CURVES3, 'curve1': CURVE1}


def calibrated(kind, white, primaries, gamma, transfer):
    args = {'whitepoint': white}
    if primaries is not None:
        args['primaries'] = primaries
    if gamma is not None:
        args['gamma'] = gamma
    else:
        args['transferfunction'] = TRANSFERS[transfer]
    return imagecodecs.cms_profile(kind, **args)


LOADING = {name: calibrated(*spec) for name, spec in CALIBRATED.items()}
for name, (kind, white, primaries, gamma, transfer) in CALIBRATED.items():
    with Case('cms', f'calibrated_{name}') as case:
        data = LOADING[name]
        case.json(kind=kind, transfer=transfer, info=info_of(data), size=len(data))
        case.array('bytes', as_u8(data))
        case.array('white', np.asarray(white, np.float64))
        if primaries is not None:
            case.array('primaries', np.asarray(primaries, np.float64))
        if gamma is not None:
            case.array('gamma', np.asarray([gamma], np.float64))
with Case('cms', 'calibrated_transfers') as case:
    case.array('srgb', SRGB_TABLE)
    case.array('curves3', CURVES3)
    case.array('curve1', CURVE1)
# Lab profiles: lab4 (rasterio._linear for untagged CIELAB), lab2, lab4 with a D65 white point.
LAB = {'lab4': imagecodecs.cms_profile('lab4'), 'lab2': imagecodecs.cms_profile('lab2'),
       'lab4_d65': imagecodecs.cms_profile('lab4', whitepoint=D65)}
CMYK = {'cmyk_clut9': cmyk_profile(9, 0.8, 'Grainy test CMYK CLUT9'),
        'cmyk_clut5_a2b': cmyk_profile(5, 1.3, 'Grainy test CMYK A2B only', with_b2a=False)}
for name, data in {**LAB, **CMYK}.items():
    with Case('cms', f'profile_{name}') as case:
        case.json(info=info_of(data), size=len(data))
        case.array('bytes', as_u8(data))
with Case('cms', 'info_builtin') as case:
    case.json(info={name: info_of(PROFILES[name]) for name in BUILTIN})

# ---------------------------------------------------------------- transforms to the working profiles
special_values = [np.nan, np.inf, -np.inf, 1e30, -1e30]
gray_values = np.concatenate([np.asarray([-1.0, -0.5, -1e-6, -0.0, 0.0, 1e-7, 1e-3, .04045, .18, .5, .999, 1.0,
                                          1.0000001, 1.5, 2.0, 6.0]), np.linspace(0, 1, 239), special_values])
gray_in = np.ascontiguousarray(gray_values.astype(np.float32).reshape(13, 20))
levels5 = np.asarray([0, 25, 50, 75, 100], np.float32)
cmyk_grid = np.stack(np.meshgrid(levels5, levels5, levels5, levels5, indexing='ij'), -1).reshape(-1, 4)
cmyk_edges = np.asarray([[-10, 0, 0, 0], [150, 0, 0, 0], [0, 0, 0, 120], [0, 0, 0, -5], [100, 100, 100, 100],
                         [np.nan, 0, 0, 0], [0, np.nan, 50, 50], [np.inf, 0, 0, 0], [1e-3, 99.9, 0.5, 33.3],
                         [12.5, 37.5, 62.5, 87.5], [3, 97, 41, 7]], np.float32)
cmyk_in = np.ascontiguousarray(np.concatenate([cmyk_grid, cmyk_edges]).astype(np.float32).reshape(12, 53, 4))
ab = np.asarray([-128, -64, 0, 64, 127], np.float32)
lab_grid = np.stack(np.meshgrid(levels5, ab, ab, indexing='ij'), -1).reshape(-1, 3)
lab_edges = np.asarray([[-5, 0, 0], [105, 0, 0], [50, 200, -200], [np.nan, 0, 0], [50, np.nan, 0], [100, 0, 0],
                        [0, 0, 0], [53.2, 80.1, 67.2], [87.7, -86.2, 83.2], [32.3, 79.2, -107.9],
                        [np.inf, 0, 0], [50, -1e30, 0], [12.25, 3.5, -7.75], [99.99, 0.01, -0.01], [1e-4, 0, 0]],
                       np.float32)
lab_in = np.ascontiguousarray(np.concatenate([lab_grid, lab_edges]).astype(np.float32).reshape(10, 14, 3))
gray_u8 = np.ascontiguousarray(np.arange(256, dtype=np.uint8).reshape(16, 16))
gray_u16 = np.ascontiguousarray((np.arange(256, dtype=np.uint32) * 257 + rng.integers(0, 257, 256))
                                .clip(0, 65535).astype(np.uint16).reshape(16, 16))
with Case('cms', 'inputs_loading') as case:
    case.array('gray', gray_in)
    case.array('cmyk', cmyk_in)
    case.array('lab', lab_in)
    case.array('gray_u8', gray_u8)
    case.array('gray_u16', gray_u16)

WORKING = ('Linear sRGB', 'Linear ProPhoto', 'sRGB')
GRAY_SOURCES = {'gray_srgb': PROFILES['Gray sRGB'],
                **{name: data for name, data in LOADING.items() if CALIBRATED[name][0] == 'gray'}}


def to_rgb(data, icc, space, destination, outdtype='float32'):
    return imagecodecs.cms_transform(np.ascontiguousarray(data), icc, destination, colorspace=space,
                                     outcolorspace='rgb', outdtype=outdtype, intent=1)


for name, icc in GRAY_SOURCES.items():
    with Case('cms', f'transform_gray_{name}') as case:
        for destination in WORKING:
            out = to_rgb(gray_in, icc, 'gray', PROFILES[destination])
            assert out.dtype == np.float32 and out.shape == gray_in.shape + (3,)
            case.array(f'to__{slug(destination)}', out)
        case.array('u8_to__linear_srgb', to_rgb(gray_u8, icc, 'gray', PROFILES['Linear sRGB']))
        case.array('u16_to__linear_prophoto', to_rgb(gray_u16, icc, 'gray', PROFILES['Linear ProPhoto']))
        for working in ('sRGB', 'ProPhoto'):   # rasterio._gray8_table: the 256 levels as float32 through cms
            case.array(f'gray8_table__{slug(working)}', rasterio._gray8_table(icc, working, {}))
for name, icc in CMYK.items():
    with Case('cms', f'transform_{name}') as case:
        for destination in WORKING:
            case.array(f'to__{slug(destination)}', to_rgb(cmyk_in, icc, 'cmyk', PROFILES[destination]))
for name, icc in LAB.items():
    with Case('cms', f'transform_{name}') as case:
        for destination in WORKING:
            case.array(f'to__{slug(destination)}', to_rgb(lab_in, icc, 'lab', PROFILES[destination]))
for name, icc in LOADING.items():   # calibrated RGB: rasterio._linear calls colorio.convert (the native path)
    if CALIBRATED[name][0] != 'rgb':
        continue
    with Case('cms', f'transform_rgb_{name}') as case:
        for destination in WORKING:
            case.array(f'to__{slug(destination)}', colorio.convert(mix, icc, PROFILES[destination]))
with Case('cms', 'transform_errors') as case:   # wrong colour space for the profile: cmsCreateTransform fails
    case.json(gray_as_rgb=error_of(lambda: to_rgb(hand, PROFILES['Gray sRGB'], 'rgb', PROFILES['sRGB'])),
              cmyk_as_lab=error_of(lambda: to_rgb(lab_in, CMYK['cmyk_clut9'], 'lab', PROFILES['sRGB'])),
              rgb_as_gray=error_of(lambda: to_rgb(gray_in, PROFILES['sRGB'], 'gray', PROFILES['sRGB'])))


# ---------------------------------------------------------------- end to end: engine.load_image on generated files
H, W = 16, 24
yy, xx = np.mgrid[:H, :W]
base = np.stack([xx / (W - 1), yy / (H - 1), (xx + yy) % 7 / 6], -1)
base[0, :4] = [[0, 0, 0], [1, 1, 1], [1, 0, 0], [0, 1, 0]]


def quantise(a, bits):
    return np.round(np.clip(a, 0, 1) * (2 ** bits - 1)).astype(np.uint8 if bits == 8 else np.uint16)


def png_with(pixels, *chunks):
    raw = imagecodecs.png_encode(np.ascontiguousarray(pixels))
    return raw[:33] + b''.join(rasterio.png_chunk(kind, payload) for kind, payload in chunks) + raw[33:]


def iccp(data):
    return b'iCCP', b'icc\0\0' + zlib.compress(data)


def gama(value):
    return b'gAMA', struct.pack('>I', value)


def chrm(*values):
    return b'cHRM', struct.pack('>8I', *(round(v * 100000) for v in values))


def rationals(values, denominator=100000):
    return tuple(int(x) for v in values for x in (round(v * denominator), denominator))


gray8 = quantise(base[..., 0] * .7 + base[..., 1] * .3, 8)
gray16 = quantise(base[..., 0] * .7 + base[..., 1] * .3, 16)
rgb8, rgb16 = quantise(base, 8), quantise(base, 16)
inks = np.stack([base[..., 0], base[..., 1], base[..., 2], (xx % 5) / 4 * yy / (H - 1)], -1)
cmyk8, cmyk16 = quantise(inks, 8), quantise(inks, 16)
lab_float = np.stack([base[..., 0] * 100, (base[..., 1] - .5) * 200, (base[..., 2] - .5) * 180], -1)
lab_float[0, :3] = [[0, 0, 0], [100, 0, 0], [50, -128, 127]]


def cielab(bits):
    scale = 2 ** (bits - 8)
    L = np.round(lab_float[..., 0] / 100 * (2 ** bits - 1))
    chroma = np.round(np.clip(lab_float[..., 1:], -128, 127) * scale).astype(np.int64)
    chroma = np.where(chroma < 0, chroma + 2 ** bits, chroma)
    dtype = np.uint8 if bits == 8 else np.uint16
    return np.ascontiguousarray(np.concatenate([L[..., None], chroma], -1).astype(dtype))


def icclab8():
    L = np.round(lab_float[..., 0] / 100 * 255)
    chroma = np.round(np.clip(lab_float[..., 1:] + 128, 0, 255))
    return np.ascontiguousarray(np.concatenate([L[..., None], chroma], -1).astype(np.uint8))


def tiff(data, photometric, extratags=(), iccprofile=None):
    def write(path):
        tifffile.imwrite(path, data, photometric=photometric, extratags=list(extratags), iccprofile=iccprofile,
                         metadata=None)
    return write


def png(data):
    return lambda path: Path(path).write_bytes(data)


white_tag = lambda values: (318, 5, 2, rationals(values), True)          # noqa: E731
primaries_tag = lambda values: (319, 5, 6, rationals(values), True)      # noqa: E731
transfer_tag = lambda curves: (301, 3, curves.size, tuple(int(v) for v in np.round(curves.ravel() * 65535)), True)  # noqa: E731
FILES = {
    'gray8_plain.png': png(png_with(gray8)),
    'gray8_icc.png': png(png_with(gray8, iccp(PROFILES['Gray sRGB']))),
    'gray16_icc_calibrated.png': png(png_with(gray16, iccp(LOADING['png_gray_gamma']))),
    'gray8_gama.png': png(png_with(gray8, gama(45455))),
    'gray16_gama_chrm.png': png(png_with(gray16, gama(55000), chrm(.31, .33, .64, .33, .3, .6, .15, .06))),
    'rgb8_gama.png': png(png_with(rgb8, gama(45455))),
    'rgb8_gama_chrm.png': png(png_with(rgb8, gama(50000), chrm(.3457, .3585, .64, .33, .21, .71, .15, .06))),
    'rgb16_gama.png': png(png_with(rgb16, gama(100000))),
    'rgb16_chrm.png': png(png_with(rgb16, chrm(.3127, .329, .68, .32, .265, .69, .15, .06))),
    'rgb8_srgb_gama.png': png(png_with(rgb8, (b'sRGB', b'\0'), gama(45455))),
    'rgb8_icc_calibrated.png': png(png_with(rgb8, iccp(LOADING['tiff_rgb_curves3']))),
    'gray8_plain.tif': tiff(gray8, 'minisblack'),
    'gray16_icc.tif': tiff(gray16, 'minisblack', iccprofile=PROFILES['Gray sRGB']),
    'gray16_white.tif': tiff(gray16, 'minisblack', [white_tag((.3457, .3585))]),
    'gray8_white_transfer.tif': tiff(gray8, 'minisblack', [white_tag((.3127, .329)), transfer_tag(CURVE1)]),
    'gray_f32.tif': tiff(np.ascontiguousarray((base[..., 0] * 1.5 - .1).astype(np.float32)), 'minisblack'),
    'rgb8_chroma.tif': tiff(rgb8, 'rgb', [white_tag((.3127, .329)), primaries_tag((.64, .33, .21, .71, .15, .06))]),
    'rgb16_primaries.tif': tiff(rgb16, 'rgb', [primaries_tag((.68, .32, .265, .69, .15, .06))]),
    'rgb8_transfer3.tif': tiff(rgb8, 'rgb', [transfer_tag(CURVES3)]),
    'rgb8_transfer1.tif': tiff(rgb8, 'rgb', [white_tag((.3457, .3585)), transfer_tag(CURVE1)]),
    'rgb_f32_chroma.tif': tiff(np.ascontiguousarray((base * 1.2).astype(np.float32)), 'rgb',
                               [white_tag((.3127, .329)), primaries_tag((.64, .33, .3, .6, .15, .06))]),
    'cmyk8_icc.tif': tiff(cmyk8, 'separated', iccprofile=CMYK['cmyk_clut9']),
    'cmyk16_icc.tif': tiff(cmyk16, 'separated', iccprofile=CMYK['cmyk_clut5_a2b']),
    'cmyk8_plain.tif': tiff(cmyk8, 'separated'),
    'lab8.tif': tiff(cielab(8), 'cielab'),
    'lab16.tif': tiff(cielab(16), 'cielab'),
    'icclab8.tif': tiff(icclab8(), 'icclab'),
    'lab8_icc_lab2.tif': tiff(cielab(8), 'cielab', iccprofile=LAB['lab2']),
}
CLI_SOURCES = ['gray8_icc.png', 'png8_gamma.png']   # the two files golden/cli could not load before
sources = {name: None for name in FILES}
sources.update({f'cli_{name}': ROOT / 'golden' / 'cli' / '_sources' / name for name in CLI_SOURCES})
for name, path in sources.items():
    with Case('cms', 'load_' + name.replace('.', '_')) as case:
        if path is None:
            for old in case.folder.glob('source.*'):
                old.unlink()
            path = case.folder / ('source' + Path(name).suffix)
            FILES[name](path)
            case.json(file=path.name)
        else:
            case.json(file=str(path.relative_to(ROOT)).replace('\\', '/'))
        for working in ('sRGB', 'ProPhoto'):
            pixels, metadata = engine.load_image(path, working_space=working)
            assert pixels.dtype == np.float32
            case.array(slug(working), pixels)
            case.json(**{f'input_profile_{slug(working)}': metadata.get('input_profile'),
                         'decoder': metadata.get('decoder')})

print('cms goldens written')
