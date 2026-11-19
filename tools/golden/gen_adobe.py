"""Goldens for grainy-adobe: lua_data, adobe_develop, adobe_preset, adobe_history and smart_rules.

Every input is written here (nothing is read from a Lightroom catalog). Values that pass between Python and
Rust are written in a tagged form so that int/float/str/table/list survive JSON:
    None -> null, bool -> true/false, int -> {"int": "123"}, float -> {"float": "<repr>"}, str -> {"str": "..."},
    dict -> {"table": [[key, value], ...]} (insertion order), list -> {"list": [...]}.
Failures are recorded as {"error": {"type": "DataError", "message": "..."}}.
"""
import base64
import hashlib
import json
import math
import os
import subprocess
import sys
from datetime import datetime
from pathlib import Path

# Times that carry an offset are converted to the computer's zone by the oracle: generate in UTC so that the
# goldens do not depend on the machine (the Rust tests evaluate with Zone::Fixed(0)).
if os.environ.get('TZ') != 'UTC0':
    sys.exit(subprocess.call([sys.executable, *sys.argv], env={**os.environ, 'TZ': 'UTC0'}))

from common import GOLDEN, Case, write_json

from luma import adobe_develop as develop
from luma import adobe_history as history
from luma import adobe_preset as preset
from luma import lua_data, smart_rules
from luma.engine import defaults, normalized

MODULE = 'adobe'
FIXED = []


def kind(error):
    if isinstance(error, lua_data.DataError):
        return 'DataError'
    if isinstance(error, ValueError):
        return 'ValueError'
    return type(error).__name__


def err(error):
    return {'error': {'type': kind(error), 'message': str(error)}}


def run(function):
    try:
        return {'ok': function()}
    except Exception as error:  # the oracle's exception is the expected value
        return err(error)


def enc(v, xml=False):
    if v is None or isinstance(v, bool):
        return v
    if isinstance(v, int):
        return {'int': str(v)}
    if isinstance(v, float):
        return {'float': repr(v)}
    if isinstance(v, str):
        return {'str': v}
    if isinstance(v, dict):
        if xml and set(v) == {'xml'}:
            return {'table': [[{'str': 'xml'}, {'str': 'XML'}]]}
        return {'table': [[enc(k), enc(x, xml)] for k, x in v.items()]}
    if isinstance(v, list):
        return {'list': [enc(x, xml) for x in v]}
    raise TypeError(type(v))


def lua(v):
    """Python data as Lua table text."""
    if v is None:
        return 'nil'
    if isinstance(v, bool):
        return 'true' if v else 'false'
    if isinstance(v, (int, float)):
        return repr(v)
    if isinstance(v, str):
        return json.dumps(v, ensure_ascii=False)
    if isinstance(v, list):
        return '{' + ', '.join(lua(x) for x in v) + '}'
    if isinstance(v, dict):
        parts = []
        for k, x in v.items():
            key = k if isinstance(k, str) and k.isidentifier() else '[' + lua(k) + ']'
            parts.append(f'{key} = {lua(x)}')
        return '{' + ', '.join(parts) + '}'
    raise TypeError(type(v))


def write(name, **values):
    """Writes golden/adobe/<name>/case.json compactly (these files are large; common.write_json indents)."""
    from common import _plain
    folder = GOLDEN / MODULE / name
    folder.mkdir(parents=True, exist_ok=True)
    text = json.dumps(_plain(values), ensure_ascii=False, separators=(',', ':'), sort_keys=True) + chr(10)
    (folder / 'case.json').write_text(text, encoding='utf-8', newline=chr(10))


# --------------------------------------------------------------------------------------------- lua_data
def lua_parse_cases():
    deep = lambda n, inner='': '{' * n + inner + '}' * n
    texts = [
        # constructs the parser accepts
        '{}', 'return {}', 's = {}', 'return {};', '{};', '  \n\t{ }  ', '{1, 2, 3}', '{1; 2; 3}', '{1, 2, 3,}', '{a = 1, b = "x"}',
        '{a = 1; b = 2;}', '{["k"] = 1, [2] = "two", [3.5] = true}', '{[ "a b" ] = 1}', '{x = {y = {z = 1}}}',
        '{ {1, 2}, {3, 4} }', '{1, [1] = 2}', '{[1] = 1, 2}', '{[2] = "b", [1] = "a"}', '{a = nil, b = false, c = true}', '{nil}',
        'return { settings = { Exposure2012 = 0.5 } }', 's = { value = { settings = { Exposure2012 = 1 } } }',
        '\ufeff{}', '\ufeff\ufeffreturn {a=1}', '{true = 1, false = 2, nil = 3}', '{_a1 = 1, A_ = 2}',
        '{return = 1}', '{s = 1}', '{a = 1} -- trailing comment', '-- head\n{a = 1}', '{a = 1, -- inside\n b = 2}',
        '--[[ long\ncomment ]] {a = 1}', '--[==[ level 2 ]] still ]==] {a = 1}', '{a = --[[x]] 1}', '--[[ unterminated {}',
        '{a = [[long]]}', '{a = [[\nfirst newline dropped]]}', '{a = [==[with ]] inside]==]}', '{a = [[line1\r\nline2\rline3]]}',
        '{a = [=[x]=], b = [==[y]==]}', '{[ [[k]] ] = 1}', '{[[k]] = 1}',
        # strings and escapes
        '{a = "plain"}', "{a = 'single'}", '{a = "tab\\there"}', '{a = "\\a\\b\\f\\n\\r\\t\\v\\\\\\"\\\'"}', '{a = "\\65\\066\\0067"}', '{a = "\\x41\\x62"}',
        '{a = "\\xe2\\x82\\xac"}', '{a = "\\xff"}', '{a = "\\256"}', '{a = "\\255"}', '{a = "\\xZ1"}', '{a = "\\x4"}', '{a = "\\q"}',
        '{a = "\\z   \n   continued"}', '{a = "line\\\nbreak"}', '{a = "line\\\r\nbreak"}', '{a = "raw\nnewline"}', '{a = "raw\rreturn"}',
        '{a = "한글 \U0001f600 text"}', '{a = "\U0001f600" b}', '{a = "unterminated', '{a = "ends with backslash\\', '{a = "\\0"}',
        '{a = "\\1234"}', '{a = "\\u0041"}', "{a = \"it's\"}",
        # numbers
        '{0, 1, -1, +5, 007}', '{1.5, -2.25, .5, 5., 1e3, 1E3, 1.5e-3, -1e2, +1e+2, 2.}', '{0xFF, 0Xff, -0x10, +0x10, 0x0}',
        '{0x10p2, 0x1.8p1, 0xA.8, 0x.8p1, 0x1P-2, -0x1.8p3, 0x1p-1074}', '{1e30, -1e30, 1e20}', '{123456789012345678901234567890}',
        '{170141183460469231731687303715884105727}', '{1e400}', '{-1e400}', '{0x1p99999}', '{1e}', '{1e+}', '{0x}', '{0xg}', '{--1}',
        '{1..2}', '{.}', '{-}', '{+}', '{1 2}', '{3.0, -0.0, 0.0, 1e-5, 123.456}', '{0.1, 0.30000000000000004, 1e-7, 123456789.123456789}',
        '{9007199254740993}', '{-9007199254740993}', '{1e15, 1e16, 12345678901234567890.0}',
        # keys
        '{[1.5] = 1}', '{[true] = 1}', '{[nil] = 1}', '{[{}] = 1}', '{[1] = 1, [1] = 2}', '{a = 1, a = 2}', '{1, [1] = 2}', '{[1.0] = 1, 1}',
        '{[2^53] = 1}', '{[1e30] = 1}', '{[-1] = 1, [0] = 2}', '{["1"] = 1, [1] = 2}', '{a = 1, ["a"] = 2}', '{[1 = 2}', '{[1] 2}', '{[1]}',
        # structure errors
        '', '   ', '\n', '-- only comment', '1', '"string"', 'true', '{', '{1', '{1,', '{a', '{a =', '{a = }', '{a = 1', '{a = 1 b = 2}', '{1 2}',
        '{} {}', '{} x', 'return {} extra', 'return', 'return 1', 'x = {}', 'sx = {}', 's = ', 's {}', 'S = {}', 'return = {}', 'returnx {}',
        '{a.b = 1}', '{f()}', '{1 + 2}', '{a = b}', '{a = {}()}', '{...}', '{,}', '{;}', '{1,,2}', '{a = 1,, b = 2}', '}', ']', '{a = 1}}',
        '{a == 1}', '{a = = 1}', '{"x" = 1}', "{'x' = 1}", '{[x] = 1}', '{a = tru}', '{a = nilx}', '{a = truex}', '{\\}', '{a = 1;;}',
        # unicode whitespace, positions after non-BMP characters
        '{\u00a0a = 1}', '\u3000{a = 1}', '{a = 1,\u2003b = 2}', '{a = "\U0001f600\U0001f600", b = ?}', '\x1c{a=1}',
        # depth
        deep(63), deep(64), deep(65), deep(66), deep(64, '1'), deep(63, '1'), deep(32), deep(1),
        '{' + '{a = ' * 40 + '1' + '}' * 40 + '}',
        # a realistic Lightroom develop table
        'return { Exposure2012 = 0.55, Contrast2012 = 12, ToneCurvePV2012 = { 0, 0, 64, 60, 255, 255 }, WhiteBalance = "As Shot", '
        'HasCrop = true, CropLeft = 0.1, CropTop = 0.1, CropRight = 0.9, CropBottom = 0.9, CropAngle = 0, ProcessVersion = "11.0" }',
    ]
    limits = [
        ('{1, 2, 3, 4}', dict(max_items=5)), ('{1, 2, 3}', dict(max_items=4)), ('{1, 2, 3}', dict(max_items=3)),
        ('{{{1}}}', dict(max_depth=3)), ('{{{{1}}}}', dict(max_depth=3)), ('{{{}}}', dict(max_depth=2)), ('{{{}}}', dict(max_depth=3)),
        ('{a = "0123456789"}', dict(max_bytes=10)), ('{a = 1}', dict(max_bytes=7)), ('{a = 1}', dict(max_bytes=6)),
        ('{a = "한글"}', dict(max_bytes=12)), ('{a = "한글"}', dict(max_bytes=13)), ('{}', dict(max_bytes=0)), ('{}', dict(max_bytes=2)),
        ('{[1]=1,[2]=2,[3]=3}', dict(max_items=6)), ('{[1]=1,[2]=2,[3]=3}', dict(max_items=7)),
    ]
    cases = []
    for text in texts:
        cases.append({'text': text, 'limits': {}})
    for text, lim in limits:
        cases.append({'text': text, 'limits': lim})
    out = []
    for c in cases:
        result = run(lambda c=c: enc(lua_data.parse(c['text'], **c['limits'])))
        out.append({**c, **result})
    return out


def lua_cases():
    write('lua_parse', cases=lua_parse_cases())
    seq_texts = ['{10, 20, 30}', '{}', '{[1] = 1, [3] = 3}', '{[0] = 1}', '{a = 1}', '{[1] = 1, [2] = 2, x = 3}', '{[2] = "b", [1] = "a"}',
                 '{[1.5] = 1}', '{[-1] = 1}', '{[1] = 1, [2] = 2, [3] = {4}}', '{[2] = 1}', '{1, 2, nil, 4}', '{[1e30] = 1}', '{{1}, {2}}']
    cases = []
    for text in seq_texts:
        table = lua_data.parse(text)
        cases.append({'text': text, **run(lambda t=table: enc(lua_data.sequence(t)))})
    for value in (None, 5, 1.5, 'x', True, [1, 2], []):
        cases.append({'value': enc(value), **run(lambda v=value: enc(lua_data.sequence(v)))})
    write('lua_sequence', cases=cases)


# ----------------------------------------------------------------------------------------- adobe_develop
NUMBER_VALUES = [0, 1, -1, 5, -5, 5.0000001, -5.0000001, 100, -100, 101, 255, 256, 0.2, 0.19999, 0.5, 2000, 49999.5, 50000, 50001, 150, 151,
                 1.5, -0.0, 0.0, 1e-9, 1e300, 10 ** 400, -10 ** 400, float('inf'), float('nan'), True, False, None, '', ' ', '5', ' 5 ', '+5', '-5',
                 '.5', '5.', '1e1', '1e', '1e+', '5_0', '1__0', '_1', '1_', 'inf', '-inf', 'nan', 'Infinity', '+0.50', '-0.50', '0x10', 'abc',
                 '1,5', '5 5', '\u00a05\u00a0', '1e400', '5e-324', 'True', {}, {1: 5}, [], [5], {'xml': 'x'}]
RANGES = [(-5, 5), (0, 100), (-100, 100), (0.2, 5), (0, 255), (2000, 50000), (-150, 150), (-4, 4), (0, 1), (-180, 180), (0, 150), (0, 360)]


def number_cases():
    cases = []
    for low, high in RANGES:
        for value in NUMBER_VALUES:
            def call(v=value, lo=low, hi=high):
                result = develop.number(v, lo, hi)
                return {'float': repr(result)}
            cases.append({'value': enc(value), 'low': low, 'high': high, **run(call)})
    write('number', cases=cases)
    bools = [True, False, 'true', 'false', 'TRUE', 'False', 'True', ' true', 'true ', 'falſe', 'tRuE', '1', '0', 1, 0, 1.0, '', None, [], {}, 'yes', 'ſtrue', 'TRUE\u00a0']
    write('boolean', cases=[{'value': enc(v), **run(lambda v=v: develop.boolean(v))} for v in bools])


def curve_cases():
    points = lambda *p: {i + 1: x for i, x in enumerate(p)}
    pair = lambda x, y: {1: x, 2: y}
    values = [
        [0, 0, 255, 255], [0, 0, 128, 140, 255, 255], [0.0, 0.0, 255.0, 255.0], [0, 0, 255], [0], [], [1, 2, 3], [0, 0, 0, 0], [0, 0, 255, 255, 255, 255],
        points(0, 0, 255, 255), points(), points(0, 0, 255), {1: 0, 3: 0}, {2: 0, 3: 0}, {'a': 1},
        ['0,0', '128,140', '255,255'], ['0, 0', ' 255 , 255'], ['0,0,1', '255,255'], ['0', '255'], ['a,b', '1,2'], ['0,0', '255'], ['0,0', '0,10'],
        ['255,255', '0,0'], ['0,0', '256,255'], ['0,-1', '255,255'], ['0,0', '1e2,200'], ['0,0', '255,255', '255,255'], ['', ''], [','] * 2, ['0,0'],
        ['0,0', 5], ['0,0', None], ['0,0', [255, 255]], ['0,0', True], ['0,0', 1.5],
        [[0, 0], [255, 255]], [[0, 0.5], [127.5, 128], [255, 255]], [['0', '0'], ['255', '255']], [[0, 0], '12'], [[0, 0], 'ab'], [[0, 0], 'abc'],
        [[0, 0], 5], [[0, 0], None], [[0, 0], True], [[0, 0], 1.5], [[0, 0], [255]], [[0, 0], [1, 2, 3]], [[0, 0]], [[0, 0], [0, 10]], [[0, 0], [255, 255], [256, 0]],
        [[0, 0], pair(255, 255)], [pair(0, 0), pair(255, 255)], [pair(0, 0), [255, 255]], [pair(0, 0), {1: 255, 3: 255}], [pair(0, 0), {}],
        [pair(0, 0), pair(100, 150), pair(255, 255)], [{1: 'a', 2: 0}, pair(255, 255)],
        points(pair(0, 0), pair(255, 255)), points([0, 0], [255, 255]), points('0,0', '255,255'), points(pair(0, 0), {1: 255, 3: 255}),
        [True, False, True, True], [None, None, None, None], [0, 0, 'x', 255], ['0,0', '255,255'][:1] + ['255,255'],
        [i * 255 / 256 for i in range(257) for _ in (0,)][:0] or [[i, i] for i in range(256)],
        [[i * 0.99, 0] for i in range(257)], [[i, 0] for i in range(257)],
        ['%d,%d' % (i, i) for i in range(256)], ['%d,%d' % (i, i) for i in range(257)],
        5, 'x', None, True, 1.5, (0, 0),
    ]
    values = [list(v) if isinstance(v, tuple) else v for v in values]
    write('curve', cases=[{'value': enc(v), **run(lambda v=v: [[{'float': repr(a)}, {'float': repr(b)}] for a, b in develop.curve(v)])} for v in values])


def lr_settings(**extra):
    base = {
        'Version': '15.0', 'ProcessVersion': '11.0', 'WhiteBalance': 'As Shot', 'Temperature': 5200, 'Tint': 8,
        'Exposure2012': 0.55, 'Contrast2012': 12, 'Highlights2012': -35, 'Shadows2012': 28, 'Whites2012': 10, 'Blacks2012': -14,
        'Texture': 6, 'Clarity2012': 9, 'Dehaze': 3, 'Vibrance': 14, 'Saturation': -4,
        'ParametricShadows': 5, 'ParametricDarks': -3, 'ParametricLights': 4, 'ParametricHighlights': -6,
        'ParametricShadowSplit': 25, 'ParametricMidtoneSplit': 50, 'ParametricHighlightSplit': 75,
        'Sharpness': 40, 'SharpenRadius': 1.0, 'SharpenDetail': 25, 'SharpenEdgeMasking': 0, 'LuminanceSmoothing': 12, 'ColorNoiseReduction': 25,
        'GrainAmount': 20, 'GrainSize': 25, 'GrainFrequency': 50,
        'PostCropVignetteAmount': -20, 'PostCropVignetteMidpoint': 50, 'PostCropVignetteFeather': 60, 'PostCropVignetteRoundness': 0,
        'PostCropVignetteHighlightContrast': 10,
        'ConvertToGrayscale': False, 'HasCrop': True, 'CropLeft': 0.05, 'CropTop': 0.1, 'CropRight': 0.95, 'CropBottom': 0.9, 'CropAngle': 0,
        'CropConstrainToWarp': 0, 'AlreadyApplied': False,
        'ToneCurvePV2012': [0, 0, 60, 52, 190, 200, 255, 255], 'ToneCurvePV2012Red': [0, 0, 255, 250],
        'ToneCurvePV2012Green': [0, 5, 255, 255], 'ToneCurvePV2012Blue': [0, 0, 128, 120, 255, 255], 'ToneCurveName2012': 'Custom',
        'SplitToningShadowHue': 220, 'SplitToningShadowSaturation': 12, 'SplitToningHighlightHue': 40, 'SplitToningHighlightSaturation': 10,
        'SplitToningBalance': -10,
        'ColorGradeShadowHue': 210, 'ColorGradeShadowSat': 8, 'ColorGradeShadowLum': -3, 'ColorGradeMidtoneHue': 30, 'ColorGradeMidtoneSat': 4,
        'ColorGradeMidtoneLum': 1, 'ColorGradeHighlightHue': 50, 'ColorGradeHighlightSat': 6, 'ColorGradeHighlightLum': 2, 'ColorGradeBalance': 5,
        'RedHue': 3, 'RedSaturation': -2, 'GreenHue': 1, 'GreenSaturation': 4, 'BlueHue': -5, 'BlueSaturation': 6,
        'Look': {'Name': 'x'}, 'LensProfileEnable': 1, 'AutoLateralCA': 1, 'RedEyeInfo': [],
    }
    for color_i, color in enumerate(develop.COLORS):
        base['HueAdjustment' + color] = color_i - 3
        base['SaturationAdjustment' + color] = 2 * color_i - 7
        base['LuminanceAdjustment' + color] = 5 - color_i
    base.update(extra)
    return base


def convert_result(values, base=None, is_raw=False, orientation=None):
    """Run develop.convert and describe the result (omitted as an ordered list of pairs)."""
    def call():
        result = develop.convert(values, base=base, is_raw=is_raw, orientation=orientation)
        return {'settings': result['settings'], 'mapped': result['mapped'], 'omitted': [[k, v] for k, v in result['omitted'].items()],
                'summary': develop.summary(result), 'summary5': develop.summary(result, limit=5), 'summary1': develop.summary(result, limit=1),
                'summary0': develop.summary(result, limit=0)}
    return run(call)


def convert_cases():
    cases = []

    def add(label, values, **kwargs):
        raw = kwargs.get('is_raw', False)
        cases.append({'label': label, 'values': enc(values), 'base': kwargs.get('base'), 'is_raw': raw, 'orientation': enc(kwargs.get('orientation')),
                      **convert_result(values, kwargs.get('base'), raw, kwargs.get('orientation'))})

    # every scalar alone: middle, the bounds, just outside, as text (XMP gives strings)
    for key, (target, low, high) in develop.SCALARS.items():
        mid = (low + high) / 2
        for tag, value in (('mid', mid), ('low', low), ('high', high), ('below', low - 1), ('above', high + 1), ('text', '%+.2f' % mid), ('bad', 'x'), ('bool', True)):
            add(f'scalar {key} {tag}', {key: value})
    add('vignette positive', {'PostCropVignetteAmount': 20})
    add('vignette zero', {'PostCropVignetteAmount': 0})
    add('vignette negative', {'PostCropVignetteAmount': -55.5})
    add('vignette below', {'PostCropVignetteAmount': -101})
    add('vignette above', {'PostCropVignetteAmount': 101})
    # the legacy (process 2010) names, with and without the 2012 name
    for old, new in (('Exposure', 'Exposure2012'), ('Contrast', 'Contrast2012'), ('Clarity', 'Clarity2012')):
        add(f'legacy {old}', {old: 1.5})
        add(f'legacy {old} both', {old: 1.5, new: 2})
        add(f'legacy {old} out of range', {old: 4.5})
        add(f'legacy {old} 2012 bad', {old: 1.5, new: 'x'})
    add('legacy exposure 4', {'Exposure': 4})
    add('legacy exposure -4', {'Exposure': -4})
    add('grayscale true', {'ConvertToGrayscale': True})
    add('grayscale text', {'ConvertToGrayscale': 'True'})
    add('grayscale text false', {'ConvertToGrayscale': 'false'})
    add('grayscale bad', {'ConvertToGrayscale': 'maybe'})
    add('grayscale number', {'ConvertToGrayscale': 1})
    # HSL and calibration, each key
    for prefix in ('HueAdjustment', 'SaturationAdjustment', 'LuminanceAdjustment'):
        for i, color in enumerate(develop.COLORS):
            add(f'hsl {prefix}{color}', {prefix + color: 10 + i})
    add('hsl bad', {'HueAdjustmentRed': 101, 'SaturationAdjustmentBlue': 'x'})
    for color in ('Red', 'Green', 'Blue'):
        for suffix in ('Hue', 'Saturation'):
            add(f'calibration {color}{suffix}', {color + suffix: -12.5})
    add('calibration bad', {'RedHue': 101, 'BlueSaturation': 'zz'})
    # tone curves
    curve = [0, 0, 64, 50, 192, 210, 255, 255]
    for suffix in ('', 'Red', 'Green', 'Blue'):
        add(f'curve PV2012{suffix}', {'ToneCurvePV2012' + suffix: curve})
        add(f'curve legacy{suffix}', {'ToneCurve' + suffix: curve})
        add(f'curve strings {suffix}', {'ToneCurvePV2012' + suffix: ['0,0', '100,120', '255,255']})
        add(f'curve both {suffix}', {'ToneCurvePV2012' + suffix: curve, 'ToneCurve' + suffix: curve})
        add(f'curve bad {suffix}', {'ToneCurvePV2012' + suffix: [0, 0, 0, 0]})
    add('curve with name', {'ToneCurvePV2012': curve, 'ToneCurveName2012': 'Linear', 'ToneCurveName': 'x'})
    add('curve name alone', {'ToneCurveName2012': 'Linear'})
    add('curve bad keeps name', {'ToneCurvePV2012': [0, 0, 0, 0], 'ToneCurveName2012': 'Custom'})
    add('curve as lua tables', {'ToneCurvePV2012': {1: {1: 0, 2: 0}, 2: {1: 255, 2: 255}}})
    add('curve attribute error', {'ToneCurvePV2012': ['0,0', 5]})
    add('curve type error', {'ToneCurvePV2012': [[0, 0], 7]})
    # parametric curve and its splits
    for key, i in zip(('ParametricShadows', 'ParametricDarks', 'ParametricLights', 'ParametricHighlights'), range(4)):
        add(f'parametric {key}', {key: 15 - 7 * i})
    add('parametric default splits', {'ParametricShadows': 10, 'ParametricShadowSplit': 25, 'ParametricMidtoneSplit': 50, 'ParametricHighlightSplit': 75})
    add('parametric splits as text', {'ParametricShadows': 10, 'ParametricShadowSplit': '25', 'ParametricMidtoneSplit': '50.0', 'ParametricHighlightSplit': '75.0'})
    add('parametric splits as float', {'ParametricShadows': 10, 'ParametricShadowSplit': 25.0})
    add('parametric custom split', {'ParametricShadows': 10, 'ParametricDarks': 4, 'ParametricShadowSplit': 30})
    add('parametric custom text split', {'ParametricLights': 10, 'ParametricHighlightSplit': ' 75'})
    add('parametric custom split only', {'ParametricMidtoneSplit': 60})
    add('parametric bad', {'ParametricShadows': 101, 'ParametricDarks': 'x'})
    # split toning and colour grading
    for tone in ('Shadow', 'Highlight'):
        add(f'split {tone} hue', {f'SplitToning{tone}Hue': 200})
        add(f'split {tone} sat', {f'SplitToning{tone}Saturation': 33})
        add(f'split {tone} bad', {f'SplitToning{tone}Hue': 361, f'SplitToning{tone}Saturation': 101})
    for tone in ('Shadow', 'Midtone', 'Highlight'):
        for suffix, low, high in (('Hue', 0, 360), ('Sat', 0, 100), ('Lum', -100, 100)):
            add(f'grade {tone}{suffix}', {f'ColorGrade{tone}{suffix}': (low + high) / 3})
            add(f'grade {tone}{suffix} bad', {f'ColorGrade{tone}{suffix}': high + 1})
    add('split balance', {'SplitToningBalance': -40})
    add('grade balance', {'ColorGradeBalance': 25})
    add('both balances', {'SplitToningBalance': -40, 'ColorGradeBalance': 25})
    add('balance bad', {'SplitToningBalance': 101})
    # white balance
    for raw in (True, False):
        add(f'wb as shot raw={raw}', {'WhiteBalance': 'As Shot', 'Temperature': 5200, 'Tint': 8}, is_raw=raw)
        add(f'wb as shot spaced raw={raw}', {'WhiteBalance': ' as  shot ', 'Temperature': 5200}, is_raw=raw)
        add(f'wb as shot alone raw={raw}', {'WhiteBalance': 'AsShot'}, is_raw=raw)
        add(f'wb custom raw={raw}', {'WhiteBalance': 'Custom', 'Temperature': 4300, 'Tint': -12}, is_raw=raw)
        add(f'wb daylight raw={raw}', {'WhiteBalance': 'Daylight', 'Temperature': 5500}, is_raw=raw)
        add(f'wb no tint raw={raw}', {'Temperature': 7000}, is_raw=raw)
        add(f'wb tint only raw={raw}', {'Tint': 10}, is_raw=raw)
        add(f'wb low raw={raw}', {'Temperature': 1999}, is_raw=raw)
        add(f'wb high raw={raw}', {'Temperature': 50001, 'Tint': 3}, is_raw=raw)
        add(f'wb tint bad raw={raw}', {'Temperature': 5000, 'Tint': 151}, is_raw=raw)
        add(f'wb text raw={raw}', {'Temperature': '5600', 'Tint': '-3.5', 'WhiteBalance': 'Custom'}, is_raw=raw)
        add(f'wb as shot with incremental raw={raw}', {'WhiteBalance': 'As Shot', 'IncrementalTemperature': 10, 'IncrementalTint': -4}, is_raw=raw)
    add('wb number', {'WhiteBalance': 5, 'Temperature': 5000}, is_raw=True)
    add('wb ſ', {'WhiteBalance': 'AſShot'}, is_raw=True)
    # crops
    keys = dict(CropLeft=0.1, CropTop=0.2, CropRight=0.8, CropBottom=0.9)
    for orientation in (None, 'AB', 1, '1', 'BA', 'DA', 'CD', 6, '6', 1.0, True, 0, ''):
        add(f'crop orientation {orientation!r}', {'HasCrop': True, 'CropAngle': 0, **keys}, orientation=orientation)
    add('crop no angle', {'HasCrop': True, **keys})
    add('crop angle', {'HasCrop': True, 'CropAngle': 2.5, **keys})
    add('crop angle bad', {'HasCrop': True, 'CropAngle': 'x', **keys})
    add('crop angle out of range', {'HasCrop': True, 'CropAngle': 181, **keys})
    add('crop angle text zero', {'HasCrop': True, 'CropAngle': '0.0', **keys})
    add('crop false', {'HasCrop': False, **keys, 'CropAngle': 3, 'CropConstrainToWarp': 1})
    add('crop false text', {'HasCrop': 'False', **keys})
    add('crop true text', {'HasCrop': 'True', **keys})
    add('crop bad flag', {'HasCrop': 'maybe', **keys})
    add('crop number flag', {'HasCrop': 1, **keys})
    add('crop missing left', {'HasCrop': True, 'CropTop': 0.1, 'CropRight': 0.9, 'CropBottom': 0.9})
    add('crop missing bottom', {'HasCrop': True, 'CropLeft': 0.1, 'CropTop': 0.1, 'CropRight': 0.9})
    add('crop bad value', {'HasCrop': True, **{**keys, 'CropRight': 1.5}})
    add('crop bad text', {'HasCrop': True, **{**keys, 'CropTop': 'x'}})
    add('crop reversed x', {'HasCrop': True, **{**keys, 'CropLeft': 0.9, 'CropRight': 0.1}})
    add('crop reversed y', {'HasCrop': True, **{**keys, 'CropTop': 0.95}})
    add('crop equal', {'HasCrop': True, **{**keys, 'CropLeft': 0.8}})
    add('crop full frame', {'HasCrop': True, 'CropLeft': 0, 'CropTop': 0, 'CropRight': 1, 'CropBottom': 1, 'CropAngle': 0})
    add('crop warp flag', {'HasCrop': True, 'CropConstrainToWarp': 1, **keys})
    add('crop keys without flag', keys)
    add('crop text values', {'HasCrop': True, 'CropLeft': '0.1', 'CropTop': '0.2', 'CropRight': '0.8', 'CropBottom': '0.9', 'CropAngle': '0'})
    # AlreadyApplied
    add('already applied', {'AlreadyApplied': True, 'Exposure2012': 1})
    add('already applied text', {'AlreadyApplied': 'true', 'Exposure2012': 1})
    add('already not applied', {'AlreadyApplied': False, 'Exposure2012': 1})
    add('already applied bad', {'AlreadyApplied': 'x', 'Exposure2012': 1})
    add('already applied number', {'AlreadyApplied': 1, 'Exposure2012': 1})
    # process versions, information keys, unknown keys
    for pv in ('11.0', '6.7', '5.0', 11, 'x', None):
        add(f'process version {pv!r}', {'ProcessVersion': pv, 'Exposure2012': 0.5})
    add('process 2010 set', {'ProcessVersion': '5.0', 'Exposure': 1.0, 'Contrast': 10, 'Clarity': 20, 'Shadows': 15, 'Brightness': 30, 'Saturation': 5})
    add('information only', {k: 'x' for k in sorted(develop.INFORMATION)})
    add('unknown keys', {'Foo': 1, 'Bar': 'x', 'Exposure2012': 1, 'LensProfileName': 'y', 'Look': {'a': 1}})
    add('empty', {})
    add('empty xml complex', {'Exposure2012': {'xml': '<x/>'}, 'ToneCurvePV2012': {'xml': '<x/>'}, 'HasCrop': {'xml': '<x/>'}})
    add('list value', {'Exposure2012': [1], 'ConvertToGrayscale': [True]})
    add('nil value', {'Exposure2012': None, 'ConvertToGrayscale': None, 'HasCrop': None})
    # realistic full sets: RAW and not, every orientation
    for raw in (False, True):
        for orientation in (None, 'AB', 'BC', 'DA', 'CD', 1, 6, '8'):
            add(f'full set raw={raw} orientation={orientation!r}', lr_settings(), is_raw=raw, orientation=orientation)
    add('full set custom wb', lr_settings(WhiteBalance='Custom', Temperature=4100, Tint=-9), is_raw=True)
    add('full set rotated crop', lr_settings(CropAngle=-1.2), is_raw=True)
    add('full set curves legacy', {**lr_settings(), **{k.replace('PV2012', ''): v for k, v in lr_settings().items() if k.startswith('ToneCurvePV2012')}}, is_raw=True)
    # xmp-like (all text) set
    add('full set as text', {k: (str(v) if isinstance(v, (int, float)) and not isinstance(v, bool) else ('True' if v is True else 'False' if v is False else v))
                             for k, v in lr_settings().items() if not isinstance(v, (dict, list))}, is_raw=True)
    # bases
    custom = {**defaults(), 'exposure': 0.75, 'crop': [0.1, 0.1, 0.9, 0.9], 'rotation': 90, 'unknown_key': 5, 'film_look': 'kodak-portra-400',
              'hsl': [[1.0, 2.0, 3.0] for _ in range(8)], 'curve': [[0.0, 0.0], [1.0, 1.0]], 'tone_version': 3, 'panels_off': ['effects', 'basic', 'bogus']}
    add('base custom', {'Contrast2012': 20, 'HueAdjustmentRed': 10}, base=custom)
    add('base custom full', lr_settings(), base=custom, is_raw=True)
    add('base empty', {'Contrast2012': 20}, base={})
    add('base partial', {'Contrast2012': 20}, base={'exposure': 3.0, 'shadows': 12.0})
    add('base old gain', {'Contrast2012': 20}, base={'exposure': 1.0})
    add('base crop null', {'HasCrop': False}, base={'crop': [0.1, 0.1, 0.9, 0.9]})
    add('base short hsl', {'HueAdjustmentRed': 10, 'HueAdjustmentMagenta': 5}, base={'hsl': [[0.0, 0.0, 0.0] for _ in range(3)]})
    # errors
    cases.append({'label': 'values list', 'values': enc([1, 2]), 'base': None, 'is_raw': False, 'orientation': None,
                  **run(lambda: develop.convert([1, 2]))})
    cases.append({'label': 'values int keys', 'values': enc({1: 2}), 'base': None, 'is_raw': False, 'orientation': None, **run(lambda: develop.convert({1: 2}))})
    cases.append({'label': 'values mixed keys', 'values': enc({'a': 2, 1: 2}), 'base': None, 'is_raw': False, 'orientation': None,
                  **run(lambda: develop.convert({'a': 2, 1: 2}))})
    cases.append({'label': 'values nil', 'values': None, 'base': None, 'is_raw': False, 'orientation': None, **run(lambda: develop.convert(None))})
    write('convert', cases=cases)


def xmp(attrs='', children='', head=True, extra_ns='', wrap=True, root_attrs=''):
    body = f'<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" xmlns:xmp="http://ns.adobe.com/xap/1.0/"{extra_ns} {attrs}>{children}</rdf:Description>'
    if wrap:
        body = (f'<x:xmpmeta xmlns:x="adobe:ns:meta/"{root_attrs}><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">{body}</rdf:RDF></x:xmpmeta>')
    return ('<?xpacket begin="\ufeff" id="W5M0MpCehiHzreSzNTczkc9d"?>\n' + body + '\n<?xpacket end="w"?>') if head else body


def xmp_cases():
    seq = lambda *items: '<rdf:Seq>' + ''.join(f'<rdf:li>{i}</rdf:li>' for i in items) + '</rdf:Seq>'
    RDF = 'xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"'
    CRS = 'xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"'
    docs = {
        'attributes': xmp('crs:Exposure2012="+0.50" crs:Contrast2012="+10" crs:WhiteBalance="As Shot" crs:HasCrop="True" crs:CropLeft="0.1"'),
        'children': xmp(children='<crs:Exposure2012>+0.50</crs:Exposure2012><crs:Contrast2012>+10</crs:Contrast2012><crs:HasCrop>False</crs:HasCrop>'),
        'both': xmp('crs:Exposure2012="+0.50"', '<crs:Contrast2012>+10</crs:Contrast2012>'),
        'seq': xmp(children='<crs:ToneCurvePV2012>' + seq('0, 0', '128, 140', '255, 255') + '</crs:ToneCurvePV2012>'),
        'seq empty': xmp(children='<crs:ToneCurvePV2012><rdf:Seq/></crs:ToneCurvePV2012>'),
        'seq empty items': xmp(children='<crs:ToneCurvePV2012><rdf:Seq><rdf:li/><rdf:li></rdf:li></rdf:Seq></crs:ToneCurvePV2012>'),
        'seq nested li': xmp(children='<crs:ToneCurvePV2012><rdf:Seq><rdf:li>a<b>x</b>tail</rdf:li></rdf:Seq></crs:ToneCurvePV2012>'),
        'seq with other children': xmp(children='<crs:ToneCurvePV2012><rdf:Seq><rdf:li>1</rdf:li><x>2</x><rdf:li>3</rdf:li></rdf:Seq></crs:ToneCurvePV2012>'),
        'alt complex': xmp(children='<crs:Name><rdf:Alt><rdf:li xml:lang="x-default">My Preset</rdf:li></rdf:Alt></crs:Name>'),
        'complex with attribute': xmp(children='<crs:Look crs:Foo="1"/>'),
        'child with xml:lang': xmp(children='<crs:Name xml:lang="x-default">Preset</crs:Name>'),
        'complex with elements': xmp(children='<crs:Look><crs:Name>x</crs:Name></crs:Look>'),
        'empty child': xmp(children='<crs:Name/><crs:Exposure2012></crs:Exposure2012>'),
        'empty attribute': xmp('crs:Exposure2012=""'),
        'other namespaces': xmp('xmp:Rating="3" crs:Exposure2012="1"', '<xmp:Label>Red</xmp:Label><crs:Contrast2012>5</crs:Contrast2012>'),
        'non crs only': xmp('xmp:Rating="3"', '<xmp:Label>Red</xmp:Label>'),
        'no fields': xmp(),
        'entities': xmp(children='<crs:Name>A &amp; B &lt;c&gt; &#65;&#x42;</crs:Name>'),
        'cdata': xmp(children='<crs:Name><![CDATA[<raw> & text]]></crs:Name>'),
        'comment in text': xmp(children='<crs:Name>a<!-- hidden -->b</crs:Name>'),
        'comment in li': xmp(children='<crs:T><rdf:Seq><rdf:li>1<!--c-->2</rdf:li></rdf:Seq></crs:T>'),
        'pi in text': xmp(children='<crs:Name>a<?pi data?>b</crs:Name>'),
        'text with children': xmp(children='<crs:Look>lead<crs:Name>x</crs:Name>trail</crs:Look>'),
        'whitespace values': xmp(children='<crs:Name>\n   spaced \n</crs:Name>'),
        'unicode value': xmp(children='<crs:Name>한글 \U0001f600 café</crs:Name>'),
        'duplicate same attribute and child': xmp('crs:Exposure2012="0.5"', '<crs:Exposure2012>0.5</crs:Exposure2012>'),
        'duplicate different': xmp('crs:Exposure2012="0.5"', '<crs:Exposure2012>0.6</crs:Exposure2012>'),
        'duplicate children different': xmp(children='<crs:Exposure2012>1</crs:Exposure2012><crs:Exposure2012>2</crs:Exposure2012>'),
        'duplicate seq same': xmp(children='<crs:T>' + seq(1, 2) + '</crs:T><crs:T>' + seq(1, 2) + '</crs:T>'),
        'duplicate seq different': xmp(children='<crs:T>' + seq(1, 2) + '</crs:T><crs:T>' + seq(1, 3) + '</crs:T>'),
        'duplicate complex same': xmp(children='<crs:Look><a/></crs:Look><crs:Look><a/></crs:Look>'),
        'duplicate complex same tail': xmp(children='<crs:Look><a/></crs:Look> <crs:Look><a/></crs:Look> '),
        'duplicate complex different tail': xmp(children='<crs:Look><a/></crs:Look><crs:Look><a/></crs:Look> '),
        'duplicate complex different': xmp(children='<crs:Look><a/></crs:Look><crs:Look><b/></crs:Look>'),
        'two descriptions': ('<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF ' + RDF + '><rdf:Description ' + CRS + ' crs:Exposure2012="1"/>'
                             '<rdf:Description ' + CRS + ' crs:Contrast2012="2"/></rdf:RDF></x:xmpmeta>'),
        'two descriptions conflict': ('<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF ' + RDF + '><rdf:Description ' + CRS + ' crs:Exposure2012="1"/>'
                                      '<rdf:Description ' + CRS + ' crs:Exposure2012="2"/></rdf:RDF></x:xmpmeta>'),
        'root description': '<rdf:Description ' + RDF + ' ' + CRS + ' crs:Exposure2012="1"/>',
        'nested description': '<a><b><rdf:Description ' + RDF + ' ' + CRS + ' crs:Exposure2012="1"/></b></a>',
        'description in description': ('<rdf:Description ' + RDF + ' ' + CRS + ' crs:Exposure2012="1"><rdf:Description crs:Contrast2012="2"/></rdf:Description>'),
        'other prefix for crs': ('<rdf:RDF ' + RDF + '><rdf:Description xmlns:c="http://ns.adobe.com/camera-raw-settings/1.0/" c:Exposure2012="1">'
                                 '<c:Contrast2012>3</c:Contrast2012></rdf:Description></rdf:RDF>'),
        'other prefix for rdf': ('<r:RDF xmlns:r="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><r:Description ' + CRS + ' crs:Exposure2012="1"/></r:RDF>'),
        'wrong crs namespace': ('<rdf:RDF ' + RDF + '><rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/2.0/" crs:Exposure2012="1"/></rdf:RDF>'),
        'default namespace crs': ('<rdf:RDF ' + RDF + '><rdf:Description xmlns="http://ns.adobe.com/camera-raw-settings/1.0/" ' + 'Exposure2012="1"/></rdf:RDF>'),
        'no xpacket': xmp('crs:Exposure2012="1"', head=False),
        'xml declaration': '<?xml version="1.0" encoding="UTF-8"?>\n' + xmp('crs:Exposure2012="1"', head=False),
        'xml declaration other encoding': '<?xml version="1.0" encoding="ISO-8859-1"?>\n' + xmp('crs:Exposure2012="1"', head=False),
        'leading whitespace': '\n  ' + xmp('crs:Exposure2012="1"', head=False),
        'leading whitespace before declaration': '\n<?xml version="1.0"?>' + xmp('crs:Exposure2012="1"', head=False),
        'trailing garbage': xmp('crs:Exposure2012="1"', head=False) + 'junk',
        'trailing whitespace': xmp('crs:Exposure2012="1"', head=False) + '\n\n  ',
        'trailing comment': xmp('crs:Exposure2012="1"', head=False) + '<!-- c -->',
        'single quotes': xmp("crs:Exposure2012='1'", head=False),
        'doctype': '<!DOCTYPE foo>' + xmp('crs:Exposure2012="1"', head=False),
        'doctype lower': '<!doctype foo>' + xmp('crs:Exposure2012="1"', head=False),
        'doctype spaced': '<! DOCTYPE foo>' + xmp('crs:Exposure2012="1"', head=False),
        'entity': '<!DOCTYPE foo [<!ENTITY x "y">]>' + xmp('crs:Exposure2012="&x;"', head=False),
        'entity lower': '<!entity x "y">' + xmp('crs:Exposure2012="1"', head=False),
        'doctype in comment': xmp('crs:Exposure2012="1"', head=False) + '<!-- <!DOCTYPE x> -->',
        'doctype word prefix': xmp('crs:Exposure2012="1"', '<crs:Name>x</crs:Name>', head=False).replace('<crs:Name>x', '<crs:Name>&lt;!DOCTYPES ok'),
        'unknown entity': xmp(children='<crs:Name>&foo;</crs:Name>'),
        'unclosed': '<rdf:RDF ' + RDF + '><rdf:Description ' + CRS + ' crs:Exposure2012="1">',
        'mismatched': '<a><b></a></b>',
        'two roots': '<a/><b/>',
        'empty': '',
        'only text': 'hello',
        'only whitespace': '  \n',
        'unbound prefix': '<a><b:c/></a>',
        'duplicate attribute': '<rdf:RDF ' + RDF + '><rdf:Description ' + CRS + ' crs:Exposure2012="1" crs:Exposure2012="1"/></rdf:RDF>',
        'less-than in attribute': '<rdf:RDF ' + RDF + '><rdf:Description ' + CRS + ' crs:Exposure2012="<"/></rdf:RDF>',
        'unquoted attribute': '<rdf:RDF ' + RDF + '><rdf:Description ' + CRS + ' crs:Exposure2012=1/></rdf:RDF>',
        'bare ampersand': xmp(children='<crs:Name>a & b</crs:Name>'),
        'control character': xmp(children='<crs:Name>a\x01b</crs:Name>'),
        'json instead': '{"a": 1}',
        'lua instead': 's = { value = { settings = { Exposure2012 = 1 } } }',
        'many seq items': xmp(children='<crs:T>' + seq(*range(300)) + '</crs:T>'),
    }
    cases = []

    def add(label, data):
        def parsed():
            return enc(develop.from_xmp_bytes(data), xml=True)
        def root():
            develop.xmp_root(data)
            return True
        cases.append({'label': label, 'hex': data.hex(), 'values': run(parsed), 'root': run(root)})

    for label, text in docs.items():
        add(label, text.encode('utf-8'))
    base = docs['attributes']
    add('utf-8 bom', b'\xef\xbb\xbf' + base.encode('utf-8'))
    add('utf-16 le bom', base.encode('utf-16'))
    add('utf-16 be bom', b'\xfe\xff' + base.encode('utf-16-be'))
    add('utf-16 le bom xml declaration', ('<?xml version="1.0" encoding="UTF-16"?>' + docs['attributes']).encode('utf-16'))
    add('utf-16 odd length', base.encode('utf-16') + b'\x00')
    add('utf-16 lone surrogate', b'\xff\xfe' + b'\x00\xd8' + base.encode('utf-16-le'))
    add('invalid utf-8', base.encode('utf-8') + b'\xff\xfe')
    add('invalid utf-8 truncated', base.encode('utf-8')[:-3] + '한'.encode('utf-8')[:2])
    add('latin-1 bytes', '<a>é</a>'.encode('latin-1'))
    add('bom only', b'\xef\xbb\xbf')
    add('double bom', b'\xef\xbb\xbf\xef\xbb\xbf' + base.encode('utf-8'))
    write('xmp', cases=cases)


def from_lua_cases():
    texts = [
        'return { Exposure2012 = 0.5, Contrast2012 = 5 }', 'return { settings = { Exposure2012 = 1 } }', 's = { value = { settings = { Exposure2012 = 2 } } }',
        's = { value = { settings = 5 } }', 's = { value = { notsettings = { a = 1 } } }', 's = { value = 5, settings = { Exposure2012 = 3 } }',
        'return { settings = 5 }', 'return { settings = { } }', 'return { value = { settings = { }, x = 1 }, settings = { y = 1 } }',
        's = { id = "00000000", internalName = "My preset", title = "My preset", type = "Develop", version = 0, value = { settings = '
        + lua({k: v for k, v in lr_settings().items() if not isinstance(v, dict)}) + ' } }',
        'return { 1, 2, 3 }', 'return { a = 1, [2] = 3 }', '', 'garbage', '{', '\ufeffreturn { Exposure2012 = 1 }', 'return { value = { settings = { Exposure2012 = 1, Exposure2012 = 2 } } }',
        'return { settings = { value = { settings = { a = 1 } } } }', 's = { value = { settings = { a = 1 } } } extra',
    ]
    write('from_lua', cases=[{'text': t, **run(lambda t=t: enc(develop.from_lua(t)))} for t in texts])


# -------------------------------------------------------------------------------------------- presets
def record(fmt, raw, **overrides):
    value = {'version': develop.VERSION, 'format': fmt, 'data': base64.b64encode(raw).decode('ascii'), 'sha256': hashlib.sha256(raw).hexdigest()}
    value.update(overrides)
    return value


def preset_cases():
    xmp_data = xmp('crs:Exposure2012="+0.50" crs:Contrast2012="+10" crs:WhiteBalance="As Shot" crs:HasCrop="True" crs:CropLeft="0.1" crs:CropTop="0.1" '
                   'crs:CropRight="0.9" crs:CropBottom="0.9" crs:CropAngle="0" crs:Temperature="5200" crs:Tint="3" crs:Foo="1"').encode()
    temp_xmp = xmp('crs:Temperature="4300" crs:Tint="-5" crs:WhiteBalance="Custom" crs:Vibrance="+20"').encode()
    lrt = ('s = { id = "x", title = "T", type = "Develop", version = 0, value = { settings = '
           + lua({k: v for k, v in lr_settings().items() if not isinstance(v, dict)}) + ' } }').encode('utf-8')
    lrt_bom = b'\xef\xbb\xbf' + b'return { Exposure2012 = 0.25, Dehaze = 12 }'
    unknown = xmp('crs:Foo="1" crs:Bar="2"').encode()
    crop_only = xmp('crs:HasCrop="True" crs:CropLeft="0.1" crs:CropTop="0.1" crs:CropRight="0.9" crs:CropBottom="0.9" crs:CropAngle="0"').encode()
    info_only = xmp('crs:Version="15.0" crs:ProcessVersion="11.0" crs:HasSettings="True"').encode()
    many_unknown = xmp(' '.join(f'crs:Unknown{i:02d}="{i}"' for i in range(8)) + ' crs:Exposure2012="1"').encode()

    base = {**defaults(), 'crop': [0.2, 0.2, 0.8, 0.8], 'rotation': 270, 'straighten': 1.5, 'flip': True, 'exposure': 0.1, 'film_look': 'kodak-portra-400'}
    records = {
        'xmp': record('xmp', xmp_data), 'xmp temp': record('xmp', temp_xmp), 'lrtemplate': record('lrtemplate', lrt), 'lrtemplate bom': record('lrtemplate', lrt_bom),
        'unknown only': record('xmp', unknown), 'crop only': record('xmp', crop_only), 'info only': record('xmp', info_only),
        'bad version': record('xmp', xmp_data, version=2), 'version float': record('xmp', xmp_data, version=1.0), 'version true': record('xmp', xmp_data, version=True),
        'version text': record('xmp', xmp_data, version='1'), 'version missing': {k: v for k, v in record('xmp', xmp_data).items() if k != 'version'},
        'bad format': record('xmp', xmp_data, format='txt'), 'format missing': {k: v for k, v in record('xmp', xmp_data).items() if k != 'format'},
        'data number': record('xmp', xmp_data, data=5), 'data missing': {k: v for k, v in record('xmp', xmp_data).items() if k != 'data'},
        'data bad base64': record('xmp', xmp_data, data='***not base64***'), 'data bad padding': record('xmp', xmp_data, data='QUJD='),
        'data unpadded': record('xmp', b'ABCD', data='QUJDRA'), 'data non-ascii': record('xmp', xmp_data, data='한글'),
        'data empty': record('xmp', b'', data=''), 'data whitespace': record('xmp', xmp_data, data=record('xmp', xmp_data)['data'][:20] + '\n' + record('xmp', xmp_data)['data'][20:]),
        'bad sha': record('xmp', xmp_data, sha256='0' * 64), 'sha missing': {k: v for k, v in record('xmp', xmp_data).items() if k != 'sha256'},
        'sha upper': record('xmp', xmp_data, sha256=hashlib.sha256(xmp_data).hexdigest().upper()),
        'sha number': record('xmp', xmp_data, sha256=5),
        'invalid utf-8 lrtemplate': record('lrtemplate', b'return { a = 1 }\xff'), 'garbage lrtemplate': record('lrtemplate', b'not lua'),
        'garbage xmp': record('xmp', b'<a'), 'many unknown': record('xmp', many_unknown),
        'empty record': {}, 'extra keys': {**record('xmp', xmp_data), 'report': 'x', 'extra': 1},
    }
    values_cases, translate_cases = [], []
    for label, rec in records.items():
        values_cases.append({'label': label, 'record': rec, **run(lambda r=rec: enc(preset.values(r), xml=True))})
        for tag, b, raw in (('none raw', None, True), ('none not raw', None, False), ('base raw', base, True), ('base not raw', base, False), ('empty base', {}, True)):
            def call(r=rec, b=b, raw=raw):
                result = preset.translate(r, base=b, is_raw=raw)
                return {'settings': result['settings'], 'mapped': result['mapped'], 'omitted': [[k, v] for k, v in result['omitted'].items()]}
            translate_cases.append({'label': f'{label} / {tag}', 'record': rec, 'base': b, 'is_raw': raw, **run(call)})
    # default arguments of translate: base=None, is_raw=True
    translate_cases.append({'label': 'defaults', 'record': records['xmp'], 'base': None, 'is_raw': True,
                            **run(lambda: (lambda r: {'settings': r['settings'], 'mapped': r['mapped'], 'omitted': [[k, v] for k, v in r['omitted'].items()]})(preset.translate(records['xmp'])))})
    write('preset_values', cases=values_cases)
    write('preset_translate', cases=translate_cases)

    folder = GOLDEN / MODULE / 'preset_read'
    folder.mkdir(parents=True, exist_ok=True)
    files = {'a.xmp': xmp_data, 'b.lrtemplate': lrt, 'c.xmp': temp_xmp, 'd.XMP': xmp_data, 'e.lrtemplate': lrt_bom, 'unknown.xmp': unknown, 'crop.xmp': crop_only,
             'info.xmp': info_only, 'f.txt': xmp_data, 'g': xmp_data, 'empty.xmp': b'', 'junk.lrtemplate': b'junk', 'many.xmp': many_unknown,
             'h.XMP.bak': xmp_data, 'i.lrtemplate': b'return { Exposure2012 = 9 }', 'j.xmp': xmp('crs:Exposure2012="0.1" crs:Temperature="5000"').encode(),
             'k.xmp': b'\xef\xbb\xbf' + xmp_data, 'l.xmp': xmp_data.decode().encode('utf-16')}
    cases = []
    for name, data in files.items():
        (folder / name).write_bytes(data)
        result = run(lambda n=name: preset.read(folder / n))
        cases.append({'file': name, **result})
    # keep the generated settings maps as inputs of resolve
    resolve_cases = []
    for name in ('a.xmp', 'b.lrtemplate', 'c.xmp'):
        stored = preset.read(folder / name)
        for path in ('C:\\photos\\a.CR2', 'photo.jpg', 'x.dng', 'y.DNG', '/p/z.tiff', 'noext', '', 'w.NEF', 'v.arw', 'v.mos', 'dir.cr2/file.png'):
            for b in (base, {}):
                resolve_cases.append({'file': name, 'path': path, 'base': b, **run(lambda s=stored, p=path, b=b: preset.resolve(s, b, p))})
    plain = {'exposure': 1.5, 'unknown_key': 3, 'contrast': 10.0}
    for path in ('a.jpg', 'a.cr2'):
        resolve_cases.append({'file': None, 'preset': plain, 'path': path, 'base': base, **run(lambda p=path: preset.resolve(plain, base, p))})
    resolve_cases.append({'file': None, 'preset': {preset.KEY: 5}, 'path': 'a.jpg', 'base': base, **run(lambda: preset.resolve({preset.KEY: 5}, base, 'a.jpg'))})
    resolve_cases.append({'file': None, 'preset': {preset.KEY: records['bad sha']}, 'path': 'a.jpg', 'base': base,
                          **run(lambda: preset.resolve({preset.KEY: records['bad sha']}, base, 'a.jpg'))})
    write('preset_read', cases=cases)
    write('preset_resolve', cases=resolve_cases)


# ---------------------------------------------------------------------------------------------- history
def history_cases():
    times = [0, 1, -1, 0.5, 0.25, 1.2345678, 1e-7, 5e-7, 1.5e-6, 2.5e-6, 7.3e8, 7.3e8 + 0.123456, 7.9e8, 1e9, -1e9, -1e10, -1e11, 1e11, 1e12, 1e20, -1e20,
             252423993599, 252423993600, 252423993599.5, -62135596800 - 978307200, -62135596800 - 978307200 - 1, -62135596800 - 978307200 + 1, 2 ** 53, 2 ** 70,
             float('inf'), float('-inf'), float('nan'), True, False, None, '7.3e8', '', [], {}, 0.0, -0.0, 86400, 31536000.75]
    write('adobe_time', cases=[{'value': enc(t) if not isinstance(t, float) or math.isfinite(t) else {'float': repr(t)}, **run(lambda t=t: history.adobe_time(t))} for t in times])

    develop_text = lua({k: v for k, v in lr_settings().items() if not isinstance(v, dict)})
    rows = {
        'full': {'text': 'return ' + develop_text}, 'settings wrapper': {'text': 's = { value = { settings = ' + develop_text + ' } }'},
        'partial': {'text': 'return { Exposure2012 = 0.5, Foo = 1 }'}, 'only unknown': {'text': 'return { Foo = 1, Bar = 2 }'}, 'only information': {'text': 'return { Version = "15.0" }'},
        'already applied': {'text': 'return { AlreadyApplied = true, Exposure2012 = 1 }'}, 'already applied bad': {'text': 'return { AlreadyApplied = "x" }'},
        'empty text': {'text': ''}, 'missing text': {}, 'null text': {'text': None}, 'number text': {'text': 5}, 'list text': {'text': ['a']}, 'false text': {'text': False},
        'garbage': {'text': 'garbage'}, 'int keys': {'text': 'return { 1, 2 }'}, 'empty table': {'text': 'return { }'},
        'curve attribute error': {'text': 'return { ToneCurvePV2012 = { "0,0", 5 } }'}, 'curve error': {'text': 'return { ToneCurvePV2012 = { 0, 0, 0, 0 } }'},
        'omitted fields': {'text': 'return { Exposure2012 = 99, Contrast2012 = 5, Foo = 1, Bar = 2 }'},
        'rotated crop': {'text': 'return { HasCrop = true, CropLeft = 0.1, CropTop = 0.1, CropRight = 0.9, CropBottom = 0.9, CropAngle = 3, Exposure2012 = 1 }'},
        'overflow hex': {'text': 'return { Exposure2012 = 0x1p99999 }'}, 'bom': {'text': '\ufeffreturn { Exposure2012 = 1 }'},
        'with name': {'name': 'Import', 'dateCreated': 1.5, 'text': 'return { Exposure2012 = 1 }'},
        'row is list': ['x'], 'row is string': 'x', 'row is none': None,
    }
    cases = []
    for label, row in rows.items():
        for raw in (False, True):
            for orientation in (None, 'BC'):
                def call(row=row, raw=raw, orientation=orientation):
                    from collections import Counter
                    warnings = Counter()
                    result = history.decode_row(row, raw, orientation, warnings)
                    shaped = None if result is None else {'settings': result['settings'], 'mapped': result['mapped'], 'omitted': [[k, v] for k, v in result['omitted'].items()]}
                    return {'result': shaped, 'warnings': dict(warnings)}
                cases.append({'label': label, 'row': row, 'is_raw': raw, 'orientation': enc(orientation), **run(call)})
    # warnings keep adding up
    def twice():
        from collections import Counter
        warnings = Counter()
        history.decode_row({'text': 'return { Foo = 1 }'}, False, None, warnings)
        history.decode_row({'text': 'return { Foo = 1, Exposure2012 = 1 }'}, False, None, warnings)
        history.decode_row({'text': 'bad'}, False, None, warnings)
        history.decode_row({'text': 'bad'}, False, None, warnings)
        return dict(warnings)
    write('decode_row', cases=cases, accumulate=run(twice))


# -------------------------------------------------------------------------------------- smart rules
def tree(*children, combine=None):
    node = {i + 1: c for i, c in enumerate(children)}
    if combine:
        node['combine'] = combine
    return node


def crit(field, op, value=None, **extra):
    node = {'criteria': field, 'operation': op}
    if value is not None:
        node['value'] = value
    node.update(extra)
    return node


def rule_trees():
    """(label, lua value) pairs that the oracle's translate accepts or rejects."""
    t = []
    for field in ('rating', 'pick', 'isoSpeedRating'):
        for op in ('==', '!=', '>', '<', '>=', '<='):
            t.append((f'{field} {op}', crit(field, op, 3 if field != 'isoSpeedRating' else 800)))
        t.append((f'{field} in', crit(field, 'in', 1, value2=4)))
        t.append((f'{field} in float', crit(field, 'in', 1.5, value2=3.5)))
        t.append((f'{field} in reversed', crit(field, 'in', 4, value2=1)))
        t.append((f'{field} in equal', crit(field, 'in', 2, value2=2)))
        t.append((f'{field} in no value2', crit(field, 'in', 1)))
        t.append((f'{field} in bad value2', crit(field, 'in', 1, value2='x')))
        t.append((f'{field} bad op', crit(field, 'contains', 1)))
        t.append((f'{field} text op', crit(field, 'any', 1)))
        t.append((f'{field} string value', crit(field, '==', '3')))
        t.append((f'{field} bool value', crit(field, '==', True)))
        t.append((f'{field} no value', crit(field, '==')))
    t.append(('rating negative', crit('rating', '>', -1)))
    t.append(('rating zero', crit('rating', '==', 0)))
    t.append(('rating float', crit('rating', '>=', 2.5)))
    t.append(('pick flag -1', crit('pick', '==', -1)))
    t.append(('iso big', crit('isoSpeedRating', '<=', 51200)))
    t.append(('rating hex', crit('rating', '==', 0x3)))
    t.append(('rating exp', crit('rating', '<', 1e1)))
    for field in ('filename', 'copyname', 'folder', 'labelText', 'title', 'caption', 'keywords', 'camera', 'cameraSN', 'lens', 'country', 'state', 'city',
                  'location', 'creator', 'jobIdentifier'):
        t.append((f'{field} any', crit(field, 'any', 'sunset beach')))
    for op in ('any', 'all', 'words', 'noneOf', 'beginsWith', 'endsWith', '==', '!='):
        t.append((f'keywords {op}', crit('keywords', op, 'Sunset, Beach')))
        t.append((f'title {op}', crit('title', op, 'Summer')))
        t.append((f'filename {op}', crit('filename', op, 'img_0001.cr2')))
        t.append((f'camera {op}', crit('camera', op, 'canon eos')))
        t.append((f'caption {op}', crit('caption', op, '  ')))
        t.append((f'folder {op}', crit('folder', op, 'C:\\Photos\\2024')))
    t.append(('title empty', crit('title', 'empty')))
    t.append(('title notEmpty', crit('title', 'notEmpty')))
    t.append(('title empty with value', crit('title', 'empty', 'x')))
    t.append(('title any no value', crit('title', 'any')))
    t.append(('title any number', crit('title', 'any', 5)))
    t.append(('title bad op', crit('title', 'regex', 'x')))
    t.append(('title >', crit('title', '>', 'x')))
    t.append(('labelText red', crit('labelText', '==', 'Red')))
    t.append(('labelText 빨강', crit('labelText', '==', '빨강')))
    t.append(('labelText yellow any', crit('labelText', 'any', 'yellow green')))
    t.append(('labelText blue words', crit('labelText', 'words', 'BLUE')))
    t.append(('labelText purple !=', crit('labelText', '!=', 'purple')))
    t.append(('labelText custom', crit('labelText', 'all', 'custom')))
    t.append(('keywords unicode', crit('keywords', 'words', 'café 한글')))
    t.append(('keywords underscore word', crit('keywords', 'words', 'a_b')))
    t.append(('keywords comma only', crit('keywords', 'any', ' , ,')))
    t.append(('keywords ß', crit('keywords', '==', 'STRASSE')))
    t.append(('keywords İ', crit('keywords', 'any', 'İstanbul')))
    for op in ('==', '!=', '>', '<', 'in', 'inLast', 'notInLast', 'today', 'yesterday', 'thisWeek', 'thisMonth', 'thisYear'):
        for field in ('captureTime', 'touchTime'):
            if op in ('inLast', 'notInLast'):
                for unit in ('hours', 'days', 'weeks', 'months', 'years'):
                    for n in (0, 1, 2, 36):
                        t.append((f'{field} {op} {n} {unit}', crit(field, op, n, value_units=unit)))
            elif op in ('today', 'yesterday', 'thisWeek', 'thisMonth', 'thisYear'):
                t.append((f'{field} {op}', crit(field, op)))
                t.append((f'{field} {op} with junk value', crit(field, op, 'x')))
            elif op == 'in':
                t.append((f'{field} in', crit(field, 'in', '2024-03-01', value2='2024-03-31')))
                t.append((f'{field} in reversed', crit(field, 'in', '2024-03-31', value2='2024-03-01')))
                t.append((f'{field} in one day', crit(field, 'in', '2024-03-15', value2='2024-03-15')))
                t.append((f'{field} in basic format', crit(field, 'in', '20240301', value2='20240331')))
                t.append((f'{field} in week format', crit(field, 'in', '2024-W10-1', value2='2024-W11')))
                t.append((f'{field} in bad value2', crit(field, 'in', '2024-03-01', value2='x')))
                t.append((f'{field} in no value2', crit(field, 'in', '2024-03-01')))
            else:
                t.append((f'{field} {op}', crit(field, op, '2024-03-15')))
    t.append(('capture bad date', crit('captureTime', '==', '2024-13-01')))
    t.append(('capture bad date text', crit('captureTime', '==', 'yesterday')))
    t.append(('capture time included', crit('captureTime', '==', '2024-03-15T10:00:00')))
    t.append(('capture number date', crit('captureTime', '==', 20240315)))
    t.append(('capture no value', crit('captureTime', '==')))
    t.append(('capture feb 30', crit('captureTime', '>', '2024-02-30')))
    t.append(('capture leap day', crit('captureTime', '==', '2024-02-29')))
    t.append(('capture year 1', crit('captureTime', '>', '0001-01-01')))
    t.append(('inLast negative', crit('captureTime', 'inLast', -1, value_units='days')))
    t.append(('inLast fraction', crit('captureTime', 'inLast', 1.5, value_units='days')))
    t.append(('inLast whole float', crit('captureTime', 'inLast', 2.0, value_units='days')))
    t.append(('inLast max', crit('captureTime', 'inLast', 10000, value_units='days')))
    t.append(('inLast max years', crit('captureTime', 'inLast', 10000, value_units='years')))
    t.append(('inLast max months', crit('captureTime', 'inLast', 10000, value_units='months')))
    t.append(('inLast max weeks', crit('captureTime', 'inLast', 10000, value_units='weeks')))
    t.append(('inLast max hours', crit('captureTime', 'inLast', 10000, value_units='hours')))
    t.append(('inLast over max', crit('captureTime', 'inLast', 10001, value_units='days')))
    t.append(('inLast bad unit', crit('captureTime', 'inLast', 1, value_units='minutes')))
    t.append(('inLast no unit', crit('captureTime', 'inLast', 1)))
    t.append(('inLast string', crit('captureTime', 'inLast', '5', value_units='days')))
    t.append(('inLast bool', crit('captureTime', 'inLast', True, value_units='days')))
    t.append(('date bad op', crit('captureTime', '>=', '2024-01-01')))
    t.append(('date contains', crit('captureTime', 'contains', '2024')))
    for value in (1, 2, 3, 4, 5, 'none', 'custom', 0, 6, 'red', 1.0, True, '1', 2 ** 70):
        t.append((f'labelColor == {value!r}', crit('labelColor', '==', value)))
    t.append(('labelColor !=', crit('labelColor', '!=', 3)))
    t.append(('labelColor != none', crit('labelColor', '!=', 'none')))
    t.append(('labelColor != custom', crit('labelColor', '!=', 'custom')))
    t.append(('labelColor >', crit('labelColor', '>', 3)))
    t.append(('labelColor no value', crit('labelColor', '==')))
    for value in ('DNG', 'RAW', 'JPG', 'TIFF', 'PNG', 'PSD', 'VIDEO', 'PSB', 'AVIF', 'JXL', 'jpg', 'HEIC', 5, ''):
        t.append((f'fileFormat == {value!r}', crit('fileFormat', '==', value)))
    t.append(('fileFormat !=', crit('fileFormat', '!=', 'RAW')))
    t.append(('fileFormat >', crit('fileFormat', '>', 'RAW')))
    t.append(('fileFormat no value', crit('fileFormat', '==')))
    t.append(('gps true', crit('hasGPSData', 'isTrue')))
    t.append(('gps false', crit('hasGPSData', 'isFalse')))
    t.append(('gps bad op', crit('hasGPSData', '==', True)))
    t.append(('gps with value', crit('hasGPSData', 'isTrue', True)))
    t.append(('unknown field', crit('exposureBias', '>', 1)))
    t.append(('field number', crit(5, '>', 1)))
    t.append(('op number', crit('rating', 5, 1)))
    t.append(('field nil', {'criteria': None, 'operation': '=='}))
    t.append(('extra option', {'criteria': 'rating', 'operation': '==', 'value': 3, 'comment': 'x'}))
    t.append(('extra int key', {'criteria': 'rating', 'operation': '==', 'value': 3, 1: 'x'}))
    t.append(('no operation', {'criteria': 'rating', 'value': 3}))
    # groups
    a, b, c = crit('rating', '>=', 3), crit('labelColor', '==', 1), crit('captureTime', 'inLast', 30, value_units='days')
    for combine in (None, 'intersect', 'union', 'exclude'):
        t.append((f'group {combine}', tree(a, b, combine=combine)))
        t.append((f'group {combine} single', tree(a, combine=combine)))
        t.append((f'group {combine} time', tree(a, c, combine=combine)))
        t.append((f'group {combine} nested', tree(tree(a, b, combine='union'), c, combine=combine)))
        t.append((f'group {combine} deep', tree(tree(tree(a, combine='exclude'), b, combine='intersect'), c, combine=combine)))
    t.append(('group empty', {}))
    t.append(('group bad combine', tree(a, combine='xor')))
    t.append(('group combine number', tree(a, combine=1)))
    t.append(('group gap', {1: a, 3: b}))
    t.append(('group zero key', {0: a}))
    t.append(('group string key', {'x': a}))
    t.append(('group float key', {1.5: a}))
    t.append(('group non table child', {1: 5}))
    t.append(('group child list', {1: tree()}))
    t.append(('group combine only', {'combine': 'union'}))
    t.append(('group 1001 children', {i + 1: a for i in range(1001)}))
    t.append(('group 1000 children', {i + 1: a for i in range(1000)}))
    nest = a
    for _ in range(24):
        nest = tree(nest)
    t.append(('nest 24', nest))
    t.append(('nest 25', tree(nest)))
    t.append(('nest 23', nest[1]))
    # SQL-friendly mixes
    t.append(('sql mix', tree(crit('rating', 'in', 2, value2=5), crit('pick', '!=', -1), crit('labelColor', '!=', 'custom'), combine='intersect')))
    t.append(('sql union mix', tree(crit('rating', '==', 5), crit('labelColor', '==', 4), combine='union')))
    t.append(('sql exclude mix', tree(crit('rating', '<', 2), crit('labelColor', '==', 'none'), combine='exclude')))
    t.append(('sql with iso', tree(crit('rating', '>', 2), crit('isoSpeedRating', '<', 800))))
    t.append(('sql with text', tree(crit('rating', '>', 2), crit('title', 'any', 'x'))))
    return t


def smart_cases():
    trees = rule_trees()
    translated = []
    for label, node in trees:
        text = lua(node)
        result = run(lambda text=text: smart_rules.translate(text))
        translated.append({'label': label, 'text': text, **result})
    # the lrtemplate-style wrapper and plain forms, and parser limits
    extras = ['return ' + lua(crit('rating', '>=', 3)), 's = {value = ' + lua(crit('rating', '>=', 3)) + '}', '{value = 5}', '{value = {}}', 'return {}', '', 'x',
              '{' * 33 + '}' * 33, '{' * 32 + '}' * 32, '{value = ' + lua(tree(crit('rating', '>', 1))) + '}', lua(tree(crit('rating', '>', 1))),
              '{' + ', '.join(['1'] * 10001) + '}', '{' + ', '.join(['{}'] * 4000) + '}', '"' + 'x' * (1024 * 1024) + '"']
    for text in extras:
        translated.append({'label': 'extra ' + text[:30].replace('\n', ' '), 'text': text if len(text) < 100000 else None, 'size': len(text),
                           **run(lambda text=text: smart_rules.translate(text))})
    write('smart_translate', cases=translated)

    # direct validate on parsed tables (depth argument 0) and finite()
    write('smart_finite', cases=[{'value': enc(v), **run(lambda v=v: enc(smart_rules.finite(v)))}
                                  for v in (0, 1, -1, 2.5, -0.0, 1e30, 10 ** 30, True, False, None, '1', [], {}, 10 ** 400)])

    valid = [(row['label'], row['ok']) for row in translated if 'ok' in row and isinstance(row.get('ok'), dict) and smart_rules.KEY in row['ok']]
    nodes = [(label, r[smart_rules.KEY]) for label, r in valid]
    hand = [
        ('hand: no criteria no combine', {}), ('hand: combine no children', {'combine': 'union'}), ('hand: children not list', {'combine': 'union', 'children': 5}),
        ('hand: text no value', {'criteria': 'title', 'operation': 'any'}), ('hand: text number', {'criteria': 'title', 'operation': 'any', 'value': 5}),
        ('hand: rating no value', {'criteria': 'rating', 'operation': '=='}), ('hand: rating string', {'criteria': 'rating', 'operation': '==', 'value': 'x'}),
        ('hand: rating in no value2', {'criteria': 'rating', 'operation': 'in', 'value': 1}), ('hand: unknown field', {'criteria': 'nope', 'operation': '=='}),
        ('hand: inLast no units', {'criteria': 'captureTime', 'operation': 'inLast', 'value': 3}),
        ('hand: inLast float', {'criteria': 'captureTime', 'operation': 'inLast', 'value': 2.5, 'value_units': 'days'}),
        ('hand: inLast unknown unit', {'criteria': 'captureTime', 'operation': 'inLast', 'value': 2, 'value_units': 'fortnights'}),
        ('hand: date no value', {'criteria': 'captureTime', 'operation': '=='}), ('hand: date number value', {'criteria': 'captureTime', 'operation': '==', 'value': 5}),
        ('hand: label unknown', {'criteria': 'labelColor', 'operation': '==', 'value': 9}), ('hand: label no value', {'criteria': 'labelColor', 'operation': '=='}),
        ('hand: label custom', {'criteria': 'labelColor', 'operation': '==', 'value': 'custom'}), ('hand: label list value', {'criteria': 'labelColor', 'operation': '==', 'value': [1]}),
        ('hand: format number', {'criteria': 'fileFormat', 'operation': '==', 'value': 5}), ('hand: format no value', {'criteria': 'fileFormat', 'operation': '=='}),
        ('hand: gps bad op', {'criteria': 'hasGPSData', 'operation': '=='}), ('hand: unknown op date', {'criteria': 'captureTime', 'operation': 'weird', 'value': 'x'}),
        ('hand: combine unknown', {'combine': 'xor', 'children': []}), ('hand: empty union', {'combine': 'union', 'children': []}),
        ('hand: empty intersect', {'combine': 'intersect', 'children': []}), ('hand: empty exclude', {'combine': 'exclude', 'children': []}),
        ('hand: missing operation', {'criteria': 'rating'}), ('hand: missing criteria with op', {'operation': '=='}),
        ('hand: sql rating no value2', {'criteria': 'rating', 'operation': 'in', 'value': 1}),
        ('hand: sql labelColor unknown', {'criteria': 'labelColor', 'operation': '==', 'value': 'zzz'}),
        ('hand: sql rating op unknown', {'criteria': 'rating', 'operation': 'weird', 'value': 1}),
        ('hand: window hours no value', {'criteria': 'captureTime', 'operation': 'inLast', 'value_units': 'hours'}),
        ('hand: relative with units days', {'criteria': 'touchTime', 'operation': 'notInLast', 'value': 3, 'value_units': 'days'}),
        ('hand: describe float', {'criteria': 'rating', 'operation': '>', 'value': 2.5}),
        ('hand: describe string with quotes', {'criteria': 'title', 'operation': 'any', 'value': "it's \"q\""}),
        ('hand: describe unknown field', {'criteria': 'weird', 'operation': '==', 'value': None}),
        ('hand: describe value null', {'criteria': 'rating', 'operation': '==', 'value': None}),
        ('hand: describe value bool', {'criteria': 'rating', 'operation': '==', 'value': True}),
        ('hand: describe value list', {'criteria': 'rating', 'operation': '==', 'value': [1, 'a', None, 2.5]}),
        ('hand: describe value trailing space', {'criteria': 'title', 'operation': 'any', 'value': 'x '}),
        ('hand: describe tab units', {'criteria': 'title', 'operation': 'any', 'value': '', 'value_units': '\t'}),
        ('hand: describe big float', {'criteria': 'rating', 'operation': '>', 'value': 1e16}),
        ('hand: describe small float', {'criteria': 'rating', 'operation': '>', 'value': 1.5e-7}),
        ('hand: describe float repr', {'criteria': 'rating', 'operation': '>', 'value': 0.1 + 0.2}),
    ]
    all_nodes = nodes + hand

    photos = [
        dict(path='C:\\Photos\\2024\\IMG_0001.CR2', rating=4, flag=1, iso=800, label='빨강', title='Summer Sunset', caption='A sunset at the beach', keywords='sunset, beach, café',
             camera='Canon EOS R5', serial='123', lens='RF 24-70', country='Korea', state='', city='Seoul', location='Han River', creator='Kim', job='J1',
             metadata=json.dumps({'DateTimeOriginal': '2024:03:15 10:30:00', 'latitude': 37.5, 'longitude': 127.0, '_last_edit_time': '2024-03-30T08:00:00'}),
             extras=json.dumps({'_copy_name': 'Copy 1'})),
        dict(path='D:/pics/sub/holiday.jpg', rating=0, flag=0, iso=100, label='', title='', caption='', keywords='', camera='NIKON Z6', metadata=json.dumps({'date': '2024-03-31T23:59:59'}),
             extras=''),
        dict(path='relative/dir/photo.JPEG', rating=5, flag=-1, iso='3200', label='파랑', title='  ', caption=None, keywords='Sunset Beach, sun_set', camera='', metadata='{}', extras='{}'),
        dict(path='scan.tif', rating=2, flag=0, iso=None, label='초록', title='Winter', metadata=json.dumps({'DateTimeOriginal': '2024-02-29T00:00:00', 'latitude': 91, 'longitude': 0})),
        dict(path='C:\\Raw\\shot.DNG', rating=3.0, flag=1, iso=1600.5, label='노랑', title='Autumn leaves', keywords='leaf; tree',
             metadata=json.dumps({'DateTimeOriginal': '2023-12-31 23:59:59', 'latitude': '37.1', 'longitude': '127.1', '_last_edit_time': '2024-03-15T12:00:00Z'}),
             extras=json.dumps({'_copy_name': '', 'title_extra': 1})),
        dict(path='C:\\Video\\clip.MP4', rating=1, flag=0, iso=400, label='보라', title='Clip', metadata=json.dumps({'DateTimeOriginal': '2024-03-31T11:30:00'}), extras='{}'),
        dict(path='noext', rating='x', flag='1', iso='abc', label='custom label', title='x', metadata=json.dumps({'DateTimeOriginal': 'not a date'}), extras='{}'),
        dict(path='/photos/a.png', rating=True, flag=False, iso=float('inf') if False else 6400, label=5, title='PNG', keywords=['a', 'b'],
             metadata=json.dumps({'DateTimeOriginal': '2025-01-01 00:00:00', '_last_edit_time': '2024-12-31T23:59:59'}), extras='{}'),
        dict(path='C:\\Photos\\2024\\sub\\x.NEF', rating=3, flag=0, iso=200, label='빨강', title='Red thing', keywords='red', metadata=json.dumps({'DateTimeOriginal': '2024-03-15'}),
             extras=json.dumps({'DateTimeOriginal': '2024-03-14T10:00:00', '_copy_name': 'Virtual copy'})),
        dict(path='C:\\x\\y.heic', rating=2, flag=1, iso=50, label='', title='Heic', metadata=json.dumps({'DateTimeOriginal': '20240331', 'latitude': -90, 'longitude': 180}), extras='{}'),
        dict(path='E:\\x\\y.avif', rating=2, flag=1, iso=50, label='', title='Avif', metadata=json.dumps({'DateTimeOriginal': '2024-03-31T00:00:00'}), extras='{}'),
        dict(path='', rating=2, metadata='{}', extras='{}'), dict(path='C:\\', rating=2, metadata=None, extras=None),
        dict(path='C:\\Photos\\a.jpg', rating=2, metadata='{"DateTimeOriginal": null, "date": "2024-03-20"}', extras='{}'),
        dict(path='C:\\Photos\\b.jpg', rating=2, metadata='{"DateTimeOriginal": "2024-03-20T10:00:00+14:00"}', extras='{}'),
        dict(path='C:\\Photos\\c.jpg', rating=2, metadata='{"DateTimeOriginal": "2024-03-20T12:00:00Z", "_last_edit_time": "2024-03-20T12:00:00.5Z"}', extras='{}'),
        dict(path='C:\\Photos\\d.jpg', rating=2, metadata='[1]', extras='{}'), dict(path='C:\\Photos\\e.jpg', rating=2, metadata='not json', extras='{}'),
        dict(path=5, rating=2), dict(rating=2),
    ]
    nows = [datetime(2024, 3, 31, 12, 0), datetime(2024, 2, 29, 0, 0), datetime(2024, 12, 31, 23, 59, 59), datetime(2025, 1, 1, 0, 0), datetime(2024, 1, 31, 8, 30),
            datetime(2024, 3, 15, 10, 30), datetime(2024, 5, 31, 12, 0), datetime(2023, 12, 31, 23, 0, 1)]
    matrix = []
    for label, node in all_nodes:
        rows = []
        for photo in photos:
            row = []
            for now in nows:
                try:
                    row.append(smart_rules.evaluate(node, photo, now=now))
                except Exception as error:
                    row.append('!' + kind(error))
            rows.append(row)
        matrix.append(rows)
    # with an explicit info argument
    info_cases = []
    indexed = list(enumerate(all_nodes))
    for index, (label, node) in indexed[:40] + indexed[-10:]:
        for info in ({}, {'DateTimeOriginal': '2024-03-30 12:00:00', 'latitude': 10, 'longitude': 20}, {'date': '2024-03-31', '_copy_name': 'x', 'label': 'lbl'}):
            photo = photos[0]
            try:
                value = smart_rules.evaluate(node, photo, now=nows[0], info=info)
            except Exception as error:
                value = '!' + kind(error)
            info_cases.append({'label': label, 'index': index, 'info': info, 'result': value})
    sql = [{'label': label, **run(lambda n=node: smart_rules.sql_condition(n))} for label, node in all_nodes]
    describe = [{'label': label, **run(lambda n=node: smart_rules.describe(n))} for label, node in all_nodes]
    describe_indent = [{'label': label, **run(lambda n=node: smart_rules.describe(n, 3))} for label, node in all_nodes[:30]]
    time_nodes = [(label, node) for label, node in all_nodes] + [(label + ' (wrapped)', {smart_rules.KEY: node}) for label, node in all_nodes[:60]]
    time_nodes += [
        ('old any', {'any': [crit('captureTime', 'today')]}), ('old all', {'all': [crit('rating', '>', 1), crit('touchTime', 'thisWeek')]}),
        ('old any plain', {'any': [crit('rating', '>', 1)]}), ('old any nested', {'any': [{'all': [{'children': [crit('captureTime', 'inLast', 1, value_units='days')]}]}]}),
        ('children not list: string', {'children': 'abc'}), ('children not list: dict', {'children': {'a': 1}}), ('children null', {'children': None}),
        ('children number', {'children': 5}), ('children true', {'children': True}), ('any null', {'any': None}), ('all string', {'all': 'x'}),
        ('children with scalars', {'children': [1, 'x', None, [crit('captureTime', 'today')]]}), ('not a dict', [1]), ('empty', {}),
        ('criteria list', {'criteria': ['captureTime'], 'operation': 'today'}), ('operation list', {'criteria': 'captureTime', 'operation': ['today']}),
        ('criteria dict', {'criteria': {'a': 1}}), ('criteria number', {'criteria': 5, 'operation': 'today'}),
        ('touch time == only', crit('touchTime', '==', '2024-01-01')), ('KEY wrapped bad', {smart_rules.KEY: 5}), ('KEY wrapped list', {smart_rules.KEY: [crit('captureTime', 'today')]}),
        ('capture today with other keys', {'criteria': 'captureTime', 'operation': 'today', 'children': [1]}),
    ]
    timed = [{'label': label, **run(lambda n=node: smart_rules.time_dependent(n))} for label, node in time_nodes]
    # _compare, _date, _window direct
    compares = []
    for actual in (None, 0, 1, 2.5, -1, 3, 'a', 'b', '2024-03-15', ''):
        for op in ('==', '!=', '>', '<', '>=', '<=', 'in', 'weird', ''):
            for value, value2 in ((2, 4), (2.5, None), ('a', 'c'), ('2024-03-01', '2024-03-31'), (None, None), (3, 3), ('b', 'a')):
                compares.append({'actual': enc(actual), 'op': op, 'value': enc(value), 'value2': enc(value2), **run(lambda a=actual, o=op, v=value, v2=value2: smart_rules._compare(a, o, v, v2))})
    dates = ['2024-03-15', '2024-03-15T10:30:00', '2024-03-15 10:30:00', '2024:03:15 10:30:00', '2024:03:15', '2024:03:15 10:30:00.250', '20240315', '20240315T103000',
             '2024-W11-5', '2024-W11', '2024W115', '2024-03-15T10:30', '2024-03-15T10', '2024-03-15T10:30:00Z', '2024-03-15T12:00:00Z', '2024-03-15T12:00:00+00:00',
             '2024-03-15T12:00:00.123456Z', '2024-03-15T12:00:00.1234567Z', '2024-03-15T12:00:00,5', '2024-03-15x10:00', '2024-03-15T', '2024-13-15', '2024-02-30',
             '2023-02-29', '2024-03-15T25:00:00', '2024-03-15T10:60:00', '2024-03-15T10:30:61', 'not a date', '', None, 0, 5, 20240315, True, 1.5, [], {}, '2024',
             '2024-03', '24-03-15', ' 2024-03-15', '2024-03-15 ', '2024-3-15', '2024-03-15T1030', '2024-03-15T103000.5', '0001-01-01', '9999-12-31T23:59:59',
             '2024-03-15T10:30:00 +09:00', '2024-03-15T10:30:00+9', '２０２４-03-15', '2024:03:15T10:30:00', '2024:3:15', '1234:56:78', '2024-03-15T10:30:00.']
    parsed_dates = []
    for value in dates:
        def call(v=value):
            r = smart_rules._date(v)
            return None if r is None else r.isoformat()
        parsed_dates.append({'value': enc(value), **run(call)})
    windows = []
    for label, node in all_nodes:
        if node.get('criteria') in smart_rules.DATE and node.get('operation') in smart_rules.DATE_OPS - {'==', '!=', '>', '<', 'in'}:
            for now in nows:
                def call(n=node, now=now):
                    s, e = smart_rules._window(n, now)
                    k = 'datetime' if isinstance(s, datetime) else 'date'
                    return {'kind': k, 'start': s.isoformat(), 'end': e.isoformat()}
                windows.append({'label': label, 'node': node, 'now': now.isoformat(), **run(call)})
    # year-1 and month-end edge windows
    for n, unit, now in ((1, 'months', datetime(2024, 3, 31)), (1, 'months', datetime(2024, 1, 31)), (13, 'months', datetime(2024, 1, 31)), (12, 'months', datetime(2024, 2, 29)),
                         (1, 'years', datetime(2024, 2, 29)), (4, 'years', datetime(2024, 2, 29)), (10000, 'years', datetime(2024, 2, 29)), (10000, 'months', datetime(2024, 2, 29)),
                         (24289, 'months', datetime(2024, 3, 1)), (24290, 'months', datetime(2024, 3, 1)), (0, 'months', datetime(2024, 3, 31)), (3, 'weeks', datetime(2024, 3, 31)),
                         (36, 'hours', datetime(2024, 3, 1, 5)), (1, 'months', datetime(2024, 12, 31)), (11, 'months', datetime(2024, 12, 31)), (12, 'months', datetime(2024, 12, 31)),
                         (2, 'years', datetime(2025, 1, 1)), (1, 'days', datetime(2024, 3, 1)), (366, 'days', datetime(2024, 12, 31)), (0, 'hours', datetime(2024, 1, 1)),
                         (5, 'fortnights', datetime(2024, 3, 31))):
        node = {'criteria': 'captureTime', 'operation': 'inLast', 'value': n, 'value_units': unit}
        def call(n=node, now=now):
            s, e = smart_rules._window(n, now)
            return {'kind': 'datetime' if isinstance(s, datetime) else 'date', 'start': s.isoformat(), 'end': e.isoformat()}
        windows.append({'label': f'edge {n} {unit}', 'node': node, 'now': now.isoformat(), **run(call)})
    for label, node in hand[-12:-6]:
        windows.append({'label': label, 'node': node, 'now': nows[0].isoformat(), **run(lambda n=node: (lambda se: {'kind': 'date', 'start': se[0].isoformat(), 'end': se[1].isoformat()})(smart_rules._window(n, nows[0])))})

    write('smart_rules', nodes=[{'label': label, 'node': node} for label, node in all_nodes], photos=photos, nows=[n.isoformat() for n in nows], matrix=matrix,
          info_cases=info_cases, sql=sql, describe=describe, describe_indent=describe_indent, time_nodes=[{'label': label, 'node': node} for label, node in time_nodes], time=timed,
          compares=compares, dates=parsed_dates, windows=windows)


def main():
    lua_cases()
    number_cases()
    curve_cases()
    from_lua_cases()
    xmp_cases()
    convert_cases()
    preset_cases()
    history_cases()
    smart_cases()
    print('adobe goldens written')


if __name__ == '__main__':
    main()
