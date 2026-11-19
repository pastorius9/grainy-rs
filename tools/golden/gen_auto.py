"""Goldens for grainy-engine's `auto` module: the analysis behind the editor's automatic buttons.

Ported functions: engine.auto_tone_settings, engine.auto_slider_value, auto_color.auto_color_settings,
workflow_data.automatic_white_balance, negative.measured/sample/converted/converted_together, upright.estimate,
scan_border.crop_for and photo_edits.automatic (its loader replaced by the source of the case).

Sources are made by recipes that the Rust test repeats exactly (float32 products of the standard inputs), the
synthetic colour negative and the camera frame of tools/golden/gen_engine.py (read back from golden/engine), and
line scenes stored as 8-bit grey (the linear image is grey/255 in float32). Results are JSON: the settings keys
an automatic button changes (against normalized(settings)) and the reports.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_auto.py
"""
from copy import deepcopy

import cv2

from common import Case, GOLDEN, ORACLE, inputs, np

from luma import engine, negative, rawcolor, upright, scan_border, workflow_data, auto_color
from luma.engine import normalized

MODULE = 'auto'
IN = inputs()
f = np.float32


def tiled(a, h, w):
    reps = (-(-h // a.shape[0]), -(-w // a.shape[1]), 1)
    return np.ascontiguousarray(np.tile(a, reps)[:h, :w])


def bordered(a, top, left, value):
    out = np.full((a.shape[0] + 2 * top, a.shape[1] + 2 * left, 3), f(value), np.float32)
    out[top:top + a.shape[0], left:left + a.shape[1]] = a
    return out


G = IN['garden']
SOURCES = dict(IN)
SOURCES['bright'] = G * f(4)
SOURCES['dark'] = G * f(.05)
SOURCES['flat'] = np.full((64, 96, 3), f(.18), np.float32)
SOURCES['low'] = G * f(.1) + f(.3)
SOURCES['cast'] = G * np.array([1.3, 1., .7], np.float32)
SOURCES['black'] = G * f(.001)
SOURCES['big'] = tiled(G, 1300, 900)
SOURCES['border'] = bordered(G, 12, 15, .002)
SOURCES['border_big'] = tiled(SOURCES['border'], 1500, 1100)


def colour_negative(scene, gamma=(.62, .66, .70)):
    base = np.array([.78, .52, .30], np.float32)
    return (base * np.power(np.maximum(scene, 1e-4) / .18, -np.array(gamma, np.float32)) * .35).astype(np.float32)


NEG = np.ascontiguousarray(np.clip(colour_negative(G[::2, ::2]), 0, 4))
assert np.array_equal(NEG, np.load(GOLDEN / 'engine' / 'negative' / 'input.npy'))
SOURCES['negative'] = NEG
SOURCES['negative_bright'] = NEG * f(1.5)
BASE, PEAK = negative.analyse(NEG)
NEGATIVE = dict(negative=True, negative_base=[float(x) for x in BASE], negative_peak=[float(x) for x in PEAK])

CAMERA, _ = engine.load_image(ORACLE / 'assets' / 'test-triple-illuminant.dng', raw_options=dict(raw_mode='as_shot'))
assert np.array_equal(np.asarray(CAMERA), np.load(GOLDEN / 'engine' / 'camera_as_shot' / 'camera.npy'))
SOURCES['camera'] = CAMERA
CAMERA_RAW = dict(raw_mode='as_shot')


# Line scenes for the automatic alignment: 8-bit grey, linear frame = grey / 255.
def scene(kind, h=240, w=320):
    img = np.full((h, w), 60, np.uint8)
    if kind == 'building':
        # A facade turned by 4 degrees: uprights and floors.
        m = cv2.getRotationMatrix2D((w / 2, h / 2), 4, 1)
        canvas = np.full((h * 2, w * 2), 70, np.uint8)
        for x in range(40, 2 * w, 50):
            cv2.rectangle(canvas, (x, 30), (x + 22, 2 * h - 30), 200, -1)
        for y in range(60, 2 * h, 70):
            cv2.line(canvas, (0, y), (2 * w, y), 140, 3)
        m2 = cv2.getRotationMatrix2D((w, h), 4, 1)
        img = cv2.warpAffine(canvas, m2, (2 * w, 2 * h))[h // 2:h // 2 + h, w // 2:w // 2 + w].copy()
        del m
    elif kind == 'horizon':
        # Sea and sky: one long boundary tilted by 3 degrees.
        yy, xx = np.mgrid[:h, :w]
        line = h * .55 + (xx - w / 2) * np.tan(np.deg2rad(3))
        img = np.where(yy < line, 190, 50).astype(np.uint8)
        img = cv2.GaussianBlur(img, (3, 3), 0)
    elif kind == 'keystone':
        # Uprights converging upwards (camera tilted up) and level floors.
        for i in range(7):
            xb = 30 + i * 43
            xt = w / 2 + (xb - w / 2) * .75
            cv2.line(img, (int(xb), h - 10), (int(xt), 10), 220, 4, cv2.LINE_AA)
        for y in range(30, h, 40):
            half = (w / 2 - 30) * (.75 + .25 * y / h)
            cv2.line(img, (int(w / 2 - half), y), (int(w / 2 + half), y), 160, 3, cv2.LINE_AA)
    elif kind == 'box':
        # A frame in two-point perspective.
        pts = np.array([[40, 50], [290, 20], [300, 220], [30, 200]], np.int32)
        cv2.polylines(img, [pts], True, 230, 4, cv2.LINE_AA)
        cv2.line(img, (165, 35), (165, 210), 230, 3, cv2.LINE_AA)
        cv2.line(img, (35, 125), (295, 120), 230, 3, cv2.LINE_AA)
        for x in range(60, 290, 40):
            cv2.line(img, (x, 45), (x + 3, 205), 180, 2, cv2.LINE_AA)
    elif kind == 'plain':
        rng = np.random.default_rng(5)
        img = (cv2.GaussianBlur(rng.random((h, w)).astype(np.float32), (0, 0), 6) * 600 % 255).astype(np.uint8)
    elif kind == 'empty':
        pass
    elif kind == 'tiny':
        img = np.full((30, 50), 100, np.uint8)
    return img


SCENES = {k: scene(k) for k in ('building', 'horizon', 'keystone', 'box', 'plain', 'empty', 'tiny')}


def scene_image(grey):
    return np.repeat((grey.astype(np.float32) / f(255))[..., None], 3, axis=2)


for k, v in SCENES.items():
    SOURCES['scene_' + k] = scene_image(v)


def source(name):
    a = SOURCES[name]
    return a if isinstance(a, rawcolor.CameraSource) else a.copy()


def py_ne(a, b):
    return a != b


def changed(before, after):
    """Keys of `after` whose value differs from `before`, and keys `before` has that `after` lacks."""
    diff = {k: v for k, v in after.items() if k not in before or py_ne(before[k], v)}
    return diff, sorted(k for k in before if k not in after)


def store(name, **values):
    with Case(MODULE, name) as c:
        c.json(**values)


def error_text(error):
    return dict(kind=type(error).__name__, message=str(error))


index = {}


def add(group, name):
    index.setdefault(group, []).append(name)


I = [[0., 0.], [.25, .25], [.5, .5], [.75, .75], [1., 1.]]
S = [[0., 0.], [.25, .18], [.5, .5], [.75, .83], [1., 1.]]
UPRIGHT = dict(matrix=[[1.02, .04, -.01], [-.03, 1.01, .005], [.02, -.015, 1.]], crop_scale=1.12, mode='full')
EDITS = {
    'defaults': {},
    'exposure': dict(exposure=1.2, contrast=15.),
    'wb': dict(temperature=30., tint=-15.),
    'kelvin': dict(kelvin_enabled=True, kelvin=3800., wb_reference=5600., tint=6.),
    'crop': dict(crop=[.1, .15, .85, .9]),
    'rotation': dict(rotation=1, flip=True),
    'straighten': dict(straighten=4.5),
    'film': dict(film_look='kodak-portra-400', film_strength=80.),
    'prophoto': dict(working_space='ProPhoto', saturation=20.),
    'panels_off': dict(exposure=.8, contrast=20., saturation=30., curve=S, panels_off=['basic', 'curve', 'mixer']),
    'hdr': dict(hdr=True, exposure=.5),
    'tone': dict(contrast=30., highlights=-40., shadows=30., curve=S, whites=20., blacks=-10., tone_version=2),
    'lens': dict(lens_vignette=40., distortion=10., perspective_v=12.),
    'upright': dict(upright=UPRIGHT),
    'colour': dict(saturation=25., vibrance=-20., hsl=[[20., -30., 10.], [0., 0., 0.], [-40., 25., 0.], [15., 0., -20.],
                                                     [0., 50., 0.], [-25., -10., 30.], [0., 0., 0.], [35., 20., -15.]]),
    'monochrome': dict(monochrome=True),
}


def settings_for(source_name, edit):
    s = dict(EDITS[edit])
    if source_name.startswith('negative'):
        s.update(NEGATIVE)
    if source_name == 'camera':
        s.update(CAMERA_RAW)
    return s


# ---- auto tone ---------------------------------------------------------------------------------------------
TONE_SOURCES = ['ramp', 'wide', 'colours', 'patches', 'noise', 'edges', 'garden', 'bright', 'dark', 'flat', 'low',
                'cast', 'big', 'negative', 'camera', 'black']
TONE_EDITS = ['defaults', 'exposure', 'wb', 'crop', 'rotation', 'straighten', 'film', 'prophoto', 'panels_off', 'hdr',
              'tone', 'lens', 'upright', 'kelvin']
for src in TONE_SOURCES:
    edits = TONE_EDITS if src in ('garden', 'camera', 'negative', 'cast') else ['defaults', 'exposure', 'crop', 'hdr']
    for edit in edits:
        name = f'tone_{src}_{edit}'
        s = settings_for(src, edit)
        adjusted, report = engine.auto_tone_settings(source(src), s)
        diff, removed = changed(normalized(s), adjusted)
        store(name, source=src, settings=s, changed=diff, removed=removed, report=report)
        add('tone', name)

# ---- auto slider value -------------------------------------------------------------------------------------
for src in ['garden', 'bright', 'dark', 'flat', 'colours', 'camera', 'big', 'wide', 'negative', 'low']:
    edits = ['defaults', 'tone', 'film', 'crop'] if src in ('garden', 'camera') else ['defaults']
    for edit in edits:
        for key in ('exposure', 'whites', 'blacks', 'contrast'):
            name = f'slider_{src}_{edit}_{key}'
            s = settings_for(src, edit)
            value = engine.auto_slider_value(source(src), s, key)
            store(name, source=src, settings=s, key=key, value=value)
            add('slider', name)

# ---- auto colour -------------------------------------------------------------------------------------------
for src in ['garden', 'cast', 'colours', 'patches', 'noise', 'flat', 'dark', 'bright', 'big', 'negative', 'camera', 'black']:
    edits = ['defaults', 'exposure', 'film', 'crop', 'colour', 'monochrome', 'hdr', 'prophoto', 'panels_off', 'tone', 'wb'] \
        if src in ('garden', 'cast') else ['defaults']
    for edit in edits:
        for amount in ((1., .5, 0., 1.7) if edit == 'defaults' else (1.,)):
            name = f'colour_{src}_{edit}_{amount:g}'
            s = settings_for(src, edit)
            adjusted, report = auto_color.auto_color_settings(source(src), s, amount)
            diff, removed = changed(normalized(s), adjusted)
            store(name, source=src, settings=s, amount=amount, changed=diff, removed=removed, report=report)
            add('colour', name)

# ---- automatic white balance (the workflow's batch button: stored, normalized settings) ----------------------
for src in ['garden', 'cast', 'colours', 'patches', 'noise', 'bright', 'big', 'negative', 'camera', 'flat', 'black', 'wide']:
    edits = ['defaults', 'wb', 'kelvin', 'crop', 'rotation', 'straighten', 'prophoto', 'lens'] if src in ('garden', 'cast', 'camera') \
        else ['defaults']
    for edit in edits:
        name = f'wb_{src}_{edit}'
        s = normalized(settings_for(src, edit))
        try:
            values = workflow_data.automatic_white_balance(source(src), deepcopy(s))
            result = dict(values={k: values[k] for k in ('kelvin_enabled', 'temperature', 'tint')})
            assert changed(s, values)[0].keys() <= {'kelvin_enabled', 'temperature', 'tint'}
        except ValueError as error:
            result = dict(error=error_text(error))
        store(name, source=src, settings=s, **result)
        add('wb', name)

# ---- negatives -----------------------------------------------------------------------------------------------
NEG_EDITS = {
    'defaults': {},
    'crop': dict(crop=[.1, .05, .9, .8]),
    'rotation': dict(rotation=3, flip=True, crop=[.2, .1, .9, 1.]),
    'straighten': dict(straighten=-3.),
    'perspective': dict(perspective_v=15., distortion=-8.),
    'upright': dict(upright=UPRIGHT),
    'raw_defaults': dict(exposure=1.5, blacks=-30.),
    'linear_raw_defaults': dict(exposure=.75, blacks=-30., contrast=10.),
    'stored': dict(NEGATIVE, negative_density=1., negative_range=5.),
    'lens': dict(lens_vignette=30.),
}
for src in ['negative', 'negative_bright', 'camera', 'garden']:
    for edit, s in NEG_EDITS.items():
        if src != 'negative' and edit not in ('defaults', 'crop', 'straighten', 'raw_defaults'):
            continue
        name = f'negative_{src}_{edit}'
        part = negative.measured(source(src), s)
        sample = negative.sample(source(src), s)
        converted = negative.converted(source(src), s)
        with Case(MODULE, name) as c:
            c.json(source=src, settings=s, measured_shape=list(part.shape),
                   measured_sum=part.astype(np.float64).sum(axis=(0, 1)).tolist(),
                   sample_shape=list(sample.shape), sample_sum=sample.astype(np.float64).sum(axis=0).tolist(),
                   converted=changed(s, converted)[0])
            if src == 'negative' and edit in ('defaults', 'straighten', 'perspective'):
                c.array('measured', part)
        add('negative', name)

ROLLS = {
    'roll': [('negative', 'defaults'), ('negative_bright', 'crop'), ('negative', 'straighten')],
    'roll_raw': [('negative', 'raw_defaults'), ('camera', 'defaults'), ('negative_bright', 'rotation')],
    'roll_one': [('negative', 'perspective')],
}
for roll, members in ROLLS.items():
    samples = [negative.sample(source(src), NEG_EDITS[edit]) for src, edit in members]
    results = negative.converted_together(samples, [NEG_EDITS[edit] for _, edit in members])
    store(f'together_{roll}', members=[dict(source=src, settings=NEG_EDITS[edit]) for src, edit in members],
          converted=[changed(NEG_EDITS[edit], r)[0] for (_, edit), r in zip(members, results)])
    add('together', f'together_{roll}')

# ---- automatic alignment -------------------------------------------------------------------------------------
for k, grey in SCENES.items():
    with Case(MODULE, f'scene_{k}') as c:
        c.array('grey', grey)
for src in ['scene_' + k for k in SCENES] + ['patches', 'edges', 'garden', 'colours']:
    for mode in ('level', 'vertical', 'full', 'auto', 'bogus'):
        name = f'upright_{src}_{mode}'
        try:
            result = dict(estimate=upright.estimate(SOURCES[src].copy(), mode))
        except ValueError as error:
            result = dict(error=error_text(error))
        store(name, source=src, mode=mode, **result)
        add('upright', name)

# ---- scan borders --------------------------------------------------------------------------------------------
for src in ['border', 'border_big', 'garden', 'dark', 'flat', 'negative', 'camera', 'scene_building']:
    edits = ['defaults', 'crop', 'rotation', 'straighten', 'exposure'] if src == 'border' else ['defaults']
    for edit in edits:
        for longest in ((1200, 100) if src in ('border', 'border_big') and edit == 'defaults' else (1200,)):
            name = f'border_{src}_{edit}_{longest}'
            s = settings_for(src, edit)
            crop = scan_border.crop_for(source(src), s, longest)
            store(name, source=src, settings=s, longest=longest, crop=None if crop is None else list(crop))
            add('border', name)

# ---- photo_edits.automatic (the AI batch's auto level / auto crop) -------------------------------------------
from luma import photo_edits, file_access  # noqa: E402

CURRENT = {}
real_load, real_present = engine.load_image, file_access.present
engine.load_image = lambda path, size, space, raw_options=None: (CURRENT['source'], {})
file_access.present = lambda path: True
try:
    for src in ['scene_building', 'scene_horizon', 'border', 'scene_empty', 'garden', 'camera']:
        for flags in ((True, False), (False, True), (True, True)):
            edits = ['defaults', 'crop', 'straighten', 'upright'] if src in ('scene_building', 'border') else ['defaults']
            for edit in edits:
                name = f'automatic_{src}_{edit}_{int(flags[0])}{int(flags[1])}'
                s = normalized(settings_for(src, edit))
                if src == 'scene_building' and edit == 'defaults':
                    s['panels_off'] = ['transform', 'basic']
                CURRENT['source'] = source(src)
                entry = dict(auto_straighten=flags[0], auto_crop=flags[1])
                try:
                    out = photo_edits.automatic(dict(path='x', id=1), deepcopy(s), entry, None)
                    result = dict(changed=changed(s, out)[0], removed=changed(s, out)[1])
                except ValueError as error:
                    result = dict(error=error_text(error))
                store(name, source=src, settings=s, auto_straighten=flags[0], auto_crop=flags[1], **result)
                add('automatic', name)
finally:
    engine.load_image, file_access.present = real_load, real_present

with Case(MODULE, '_index') as c:
    c.json(**index)
print({k: len(v) for k, v in index.items()})
