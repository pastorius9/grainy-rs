"""Goldens for grainy-local: mask layers, apply_local, apply_retouch (luma/processing.py), luma/healing.py and
luma/range_mask.py on the oracle's CPU path.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_local.py [section ...]
Sections: masks, layers, local, retouch, healing, range (default: all).

Functions the Rust port takes as traits (cv2.inpaint, cv2.distanceTransform, dust.repair, sky_mask) are recorded
while the oracle runs: every call's inputs and outputs are stored so the Rust tests replay them through stand-ins
that also check the inputs the port computed.
"""
import sys
from contextlib import contextmanager

import cv2

from common import Case, inputs, np

from luma import engine, healing, native_gpu, processing as P, range_mask
import luma.dust as dust_module
import luma.sky_mask as sky_module

MODULE = 'local'
SECTIONS = set(sys.argv[1:]) or {'masks', 'layers', 'local', 'retouch', 'healing', 'range'}
IN = inputs()
U64 = np.uint64

# The CPU path is the reference: the GPU local stage must be off.
assert not native_gpu.enabled()
assert native_gpu.local(np.zeros((4, 4, 3), np.float32), np.ones((4, 4), np.float32), {'exposure': 1}) is None


def bits_sum(a):
    """Wrapping sum of the float64 bit patterns of a float32 array: changes when any value changes by one ulp."""
    return int(np.ascontiguousarray(a, np.float64).view(U64).sum(dtype=U64))


def exact(case, name, a, step=None):
    """An output the Rust port reproduces bit for bit: its full bit sum, shape and a subsample (every third row
    and column, every fourth on the 160x240 frames)."""
    a = np.asarray(a)
    step = step or (4 if a.shape[0] >= 100 else 3)
    case.json(**{f'{name}_bits': str(bits_sum(a)), f'{name}_shape': list(a.shape), f'{name}_step': step})
    case.array(f'{name}_sub', np.ascontiguousarray(a[::step, ::step]))


def srgb(name):
    """apply_local works on the display-encoded developed frame: to_srgb of the scene-linear inputs."""
    return np.ascontiguousarray(engine.to_srgb(IN[name]), np.float32)


def identity(m):
    return m


def cropflip(m):
    """A simple geometry: crop 3 rows top, 2 bottom, 1 column left, 4 right, then mirror horizontally."""
    return np.ascontiguousarray(m[3:m.shape[0] - 2, 1:m.shape[1] - 4][:, ::-1])


TRANSFORMS = {'identity': identity, 'cropflip': cropflip}


# ---------------------------------------------------------------- recording the trait-backed functions
REC = None
_SUSPEND = [0]
_inpaint = cv2.inpaint
_distance = cv2.distanceTransform
_repair = dust_module.repair
_sky = sky_module.sky_mask


def _inpaint_rec(src, mask, radius, flags):
    out = _inpaint(src, mask, radius, flags)
    if REC is not None and not _SUSPEND[0]:
        assert flags == cv2.INPAINT_NS and src.dtype == np.float32 and src.ndim == 2
        REC['inpaint'].append((np.array(src), np.array(mask), float(radius), np.array(out)))
    return out


def _distance_rec(src, kind, size, *args, **kw):
    out = _distance(src, kind, size, *args, **kw)
    if REC is not None and not _SUSPEND[0]:
        assert kind == cv2.DIST_L2 and size == 3
        REC['distance'].append((np.array(src), np.array(out)))
    return out


def _repair_rec(image, spots, *, cancel=None, copy=True, method='smooth'):
    before = np.array(image)
    _SUSPEND[0] += 1
    try:
        out = _repair(image, spots, cancel=cancel, copy=copy, method=method)
    finally:
        _SUSPEND[0] -= 1
    if REC is not None:
        REC['dust'].append((before, spots, method, np.array(out)))
    return out


def _sky_rec(source, component, transform=None):
    _SUSPEND[0] += 1
    try:
        out = _sky(source, component, transform)
    finally:
        _SUSPEND[0] -= 1
    if REC is not None:
        REC['sky'].append((np.array(source), dict(component), transform is not None, np.array(out)))
    return out


cv2.inpaint = _inpaint_rec
cv2.distanceTransform = _distance_rec
dust_module.repair = _repair_rec
sky_module.sky_mask = _sky_rec


@contextmanager
def recording(case):
    """Records the trait-backed calls made inside the block and stores them in the case."""
    global REC
    REC = {'inpaint': [], 'distance': [], 'dust': [], 'sky': []}
    try:
        yield
    finally:
        rec, REC = REC, None
        for k, (src, mask, radius, out) in enumerate(rec['inpaint']):
            case.array(f'inpaint_{k}_in', src)
            case.array(f'inpaint_{k}_mask', mask)
            case.array(f'inpaint_{k}_out', out)
        distance = []
        for k, (src, out) in enumerate(rec['distance']):
            # automatic_offset reads only the largest distance: the binary input (packed) and that value.
            case.array(f'distance_{k}_in', np.packbits(src.astype(bool), axis=None))
            distance.append(dict(shape=list(src.shape), max=float(out.max())))
        dust = []
        for k, (before, spots, method, after) in enumerate(rec['dust']):
            # The repair changes a few pixels: the input's bit sum and the changed rectangle of the output.
            changed = np.argwhere(np.any(before != after, axis=-1))
            (y0, x0), (y1, x1) = (changed.min(0), changed.max(0) + 1) if len(changed) else ((0, 0), (0, 0))
            case.array(f'dust_{k}_patch', np.ascontiguousarray(after[y0:y1, x0:x1]))
            dust.append(dict(spots=spots, method=method, input_bits=str(bits_sum(before)), output_bits=str(bits_sum(after)),
                             window=[int(y0), int(y1), int(x0), int(x1)]))
        sky = []
        for k, (source, component, transformed, out) in enumerate(rec['sky']):
            case.array(f'sky_{k}_out', out)
            sky.append(dict(component=component, transformed=transformed, input_bits=str(bits_sum(source)),
                            input_shape=list(source.shape)))
        case.json(recorded=dict(inpaint=len(rec['inpaint']), distance=distance, dust=dust, sky=sky))


def error_of(function):
    try:
        function()
    except Exception as error:   # noqa: BLE001 - the message is the golden
        return f'{type(error).__name__}: {error}'
    return None


# ---------------------------------------------------------------- frames
FRAMES = {name: srgb(name) for name in ('garden', 'patches', 'colours', 'wide', 'noise', 'edges')}
FRAMES['odd'] = np.ascontiguousarray(FRAMES['garden'][3:40, 5:58])
LINEAR = {name: np.ascontiguousarray(IN[name]) for name in ('garden', 'noise', 'patches', 'edges')}
red = LINEAR['patches'].copy()
yy, xx = np.mgrid[:64, :96]
red[(yy - 30) ** 2 + (xx - 40) ** 2 < 49] = [.8, .1, .12]
red[(yy - 20) ** 2 + (xx - 70) ** 2 < 25] = [.5, .3, .32]
LINEAR['red'] = red
if SECTIONS - {'masks'}:
    # The frames are rebuilt in Rust from the standard inputs (to_srgb; the red discs); their bit sums check them.
    with Case(MODULE, 'inputs') as case:
        case.json(frames={name: str(bits_sum(a)) for name, a in FRAMES.items()},
                  linear={name: str(bits_sum(a)) for name, a in LINEAR.items()})


# ---------------------------------------------------------------- masks
if 'masks' in SECTIONS:
    rng = np.random.default_rng(7)
    walk = np.clip(np.cumsum(rng.normal(0, .05, (30, 2)), axis=0) + .5, -.1, 1.1).tolist()
    BRUSHES = [
        ('dot', (64, 96), [[.5, .5]], .04, 70),
        ('stroke', (64, 96), [[.1, .2], [.4, .6], [.8, .3]], .05, 50),
        ('horizontal', (64, 96), [[.1, .5], [.9, .5]], .03, 70),
        ('vertical', (64, 96), [[.3, .1], [.3, .9]], .03, 70),
        ('offframe', (64, 96), [[-.5, -.5], [1.5, .5]], .04, 70),
        ('outside', (64, 96), [[2., 2.]], .04, 70),
        ('hard', (64, 96), [[.5, .5], [.6, .55]], .1, 0),
        ('soft', (64, 96), [[.5, .5]], .2, 100),
        ('feather_tiny', (64, 96), [[.45, .55]], .08, .5),
        ('radius_tiny', (64, 96), [[.25, .75], [.3, .7]], .001, 70),
        ('radius_huge', (64, 96), [[.5, .5]], 2., 70),
        ('ints', (64, 96), [[0, 1], [1, 0]], .04, 70),
        ('long', (64, 96), walk, .02, 60),
        ('odd', (37, 53), [[.2, .3], [.7, .8]], .06, 40),
        ('tiny_frame', (1, 1), [[.5, .5]], .04, 70),
        ('strip', (2, 5), [[0, 0], [1, 1]], .3, 70),
        ('empty', (64, 96), [], .04, 70),
        ('repeat', (64, 96), [[.4, .4], [.4, .4], [.42, .4]], .05, 70),
        ('large', (160, 240), [[.05, .1], [.5, .9], [.95, .2]], .03, 70),
    ]
    with Case(MODULE, 'brush') as case:
        rows = []
        for name, shape, points, radius, feather in BRUSHES:
            mask, bounds = P.brush_mask(shape, points, radius, feather, with_bounds=True)
            assert mask.dtype == np.float32
            rows.append(dict(name=name, shape=shape, points=points, radius=radius, feather=feather,
                             bounds=None if bounds is None else [int(b) for b in bounds]))
            exact(case, name, mask)
        case.json(cases=rows)

    COMPONENTS = [
        ('linear', (64, 96), dict(type='linear', points=[[.2, .3], [.7, .8]])),
        ('linear_rev', (64, 96), dict(type='linear', points=[[.9, .1], [.5, .5], [.1, .9]])),
        ('linear_zero', (64, 96), dict(type='linear', points=[[.5, .5], [.5, .5]])),
        ('linear_default', (64, 96), dict(type='linear')),
        ('linear_ints', (64, 96), dict(type='linear', points=[[0, 0], [1, 1]])),
        ('linear_odd', (37, 53), dict(type='linear', points=[[.1, .9], [.6, .2]])),
        ('radial', (64, 96), dict(type='radial', points=[[.3, .3], [.7, .8]])),
        ('radial_feather0', (64, 96), dict(type='radial', points=[[.3, .3], [.7, .8]], feather=0)),
        ('radial_feather100', (64, 96), dict(type='radial', points=[[.2, .1], [.6, .9]], feather=100)),
        ('radial_angle30', (64, 96), dict(type='radial', points=[[.2, .3], [.8, .6]], angle=30)),
        ('radial_angle_neg', (37, 53), dict(type='radial', points=[[.1, .3], [.7, .6]], angle=-90, feather=40)),
        ('radial_angle_big', (64, 96), dict(type='radial', points=[[.3, .2], [.6, .9]], angle=400.5, feather=20)),
        ('radial_tiny', (64, 96), dict(type='radial', points=[[.5, .5], [.5, .5]])),
        ('radial_offframe', (64, 96), dict(type='radial', points=[[-.5, .2], [.3, 1.4]], angle=12)),
        ('radial_1px', (1, 1), dict(type='radial', points=[[0, 0], [1, 1]])),
        ('radial_large', (160, 240), dict(type='radial', points=[[.1, .2], [.9, .7]], angle=-33, feather=55)),
        ('brush', (64, 96), dict(type='brush', points=[[.2, .2], [.8, .8]], radius=.05, feather=30)),
        ('brush_default', (64, 96), dict()),
        ('unknown', (64, 96), dict(type='other', points=[[.1, .1]])),
        ('null_type', (64, 96), dict(type=None)),
    ]
    with Case(MODULE, 'component') as case:
        rows = []
        for name, shape, component in COMPONENTS:
            raw = P.component_mask(shape, component)
            rows.append(dict(name=name, shape=shape, component=component, dtype=str(raw.dtype)))
            exact(case, name, np.asarray(raw, np.float32))
        case.json(cases=rows)


# ---------------------------------------------------------------- layer components and layer masks
RADIAL = dict(type='radial', points=[[.25, .3], [.7, .85]], feather=60)
STROKE = dict(type='brush', points=[[.15, .2], [.5, .4], [.85, .25]], radius=.06, feather=50)
LINEAR_C = dict(type='linear', points=[[.1, .9], [.6, .3]])
if 'layers' in SECTIONS:
    COMPONENT_CASES = [
        ('luma_default', 'patches', dict(type='luma')),
        ('luma_range', 'garden', dict(type='luma', range=[.1, .4], softness=.05)),
        ('luma_hard', 'patches', dict(type='luma', range=[.3, .5], softness=0)),
        ('luma_high', 'wide', dict(type='luma', range=[.6, 1.2], softness=.3)),
        ('color_rgb', 'colours', dict(type='color', rgb=[.8, .3, .2], tolerance=.2)),
        ('color_default', 'garden', dict(type='color')),
        ('color_tol0', 'patches', dict(type='color', rgb=[.4, .5, .6], tolerance=0)),
        ('color_tol2', 'noise', dict(type='color', rgb=[.1, .1, .1], tolerance=2)),
        ('color_samples1', 'garden', dict(type='color', samples=[[.3, .4, .2]], tolerance=.15)),
        ('color_samples3', 'colours', dict(type='color', samples=[[.2, .2, .8], [.9, .5, .1], [.5, .5, .5]])),
        ('color_samples5', 'patches', dict(type='color', samples=[[.1, .2, .3], [.4, .5, .6], [.7, .8, .9], [.2, .9, .1], [1, 0, 0]], tolerance=.05)),
        ('auto_default', 'colours', dict(type='brush', points=[[.3, .4], [.6, .5]], radius=.12, auto_mask=True)),
        ('auto_seed', 'garden', dict(type='brush', points=[[.2, .3], [.7, .6]], radius=.1, auto_mask=True, seed=[.5, .5], auto_tolerance=.08)),
        ('auto_tol_tiny', 'patches', dict(type='brush', points=[[.3, .3], [.6, .6]], radius=.15, auto_mask=True, auto_tolerance=0)),
        ('auto_source', 'patches', dict(type='brush', points=[[.3, .4], [.6, .5]], radius=.12, auto_mask=True)),
        ('auto_off', 'patches', dict(type='brush', points=[[3, 3]], radius=.12, auto_mask=True)),
        ('brush_typed', 'patches', STROKE),
        ('untyped', 'patches', dict(points=[[.3, .3]], radius=.1, auto_mask=True)),
        ('radial', 'patches', RADIAL),
    ]
    for tname, transform in TRANSFORMS.items():
        with Case(MODULE, f'layer_component_{tname}') as case:
            rows = []
            for name, frame, component in COMPONENT_CASES:
                source = FRAMES[frame]
                image = transform(source)
                # Auto-masked brushes need a colour source in source pixels once the frame is transformed.
                source_name = 'colours' if name == 'auto_source' else (
                    frame if tname == 'cropflip' and component.get('auto_mask') else None)
                source_image = FRAMES[source_name] if source_name else None
                mask = P._layer_component(image, source.shape, component, transform, source_image)
                assert mask.dtype == np.float32, name
                rows.append(dict(name=name, frame=frame, component=component, source_image=source_name))
                exact(case, name, mask)
            case.json(cases=rows)

    # Sky components through the recorded sky detection, without and with the developed colour source.
    for tname, transform in TRANSFORMS.items():
        with Case(MODULE, f'layer_sky_{tname}') as case:
            component = dict(type='sky', sensitivity=60, feather=40)
            source = FRAMES['garden']
            image = transform(source)
            with recording(case):
                exact(case, 'frame', P._layer_component(image, source.shape, component, transform, None))
                exact(case, 'source', P._layer_component(image, source.shape, component, transform, source))
            case.json(component=component)

    # Errors.
    with Case(MODULE, 'layer_errors') as case:
        patches = FRAMES['patches']
        errors = dict(
            auto_shape=error_of(lambda: P._layer_component(cropflip(patches), patches.shape,
                                                           dict(type='brush', points=[[.5, .5]], auto_mask=True),
                                                           cropflip, None)),
            samples6=error_of(lambda: P._layer_component(patches, patches.shape, dict(type='color', samples=[[0, 0, 0]] * 6), identity, None)),
            samples0=error_of(lambda: P._layer_component(patches, patches.shape, dict(type='color', samples=[]), identity, None)),
            samples_inf=error_of(lambda: P._layer_component(patches, patches.shape, dict(type='color', samples=[[1e39, 0, 0]]), identity, None)),
            samples_shape=error_of(lambda: P._layer_component(patches, patches.shape, dict(type='color', samples=[[1, 2]]), identity, None)),
            tolerance0=error_of(lambda: P._layer_component(patches, patches.shape, dict(type='color', samples=[[1, 0, 0]], tolerance=0), identity, None)),
            tolerance_neg=error_of(lambda: P._layer_component(patches, patches.shape, dict(type='color', samples=[[1, 0, 0]], tolerance=-1), identity, None)),
        )
        case.json(errors=errors)

    LAYERS = [
        ('add3', dict(components=[RADIAL, STROKE, LINEAR_C])),
        ('subtract', dict(components=[RADIAL, {**STROKE, 'operation': 'subtract'}])),
        ('intersect', dict(components=[LINEAR_C, {**RADIAL, 'operation': 'intersect'}])),
        ('first_disabled', dict(components=[{**STROKE, 'enabled': False}, {**RADIAL, 'operation': 'subtract'}, LINEAR_C])),
        ('invert_component', dict(components=[{**RADIAL, 'invert': True}, STROKE])),
        ('layer_invert', dict(components=[RADIAL, STROKE], invert=True, opacity=.6)),
        ('opacity2', dict(components=[RADIAL], opacity=2)),
        ('pv2_add', dict(components=[{**STROKE, 'paint_version': 2, 'flow': 40, 'density': 80},
                                     dict(type='brush', points=[[.3, .8], [.6, .1]], radius=.08, paint_version=2, flow=60)])),
        ('pv2_ops', dict(components=[RADIAL, {**STROKE, 'paint_version': 2, 'operation': 'subtract', 'flow': 50, 'density': 70},
                                     dict(type='brush', points=[[.5, .5]], radius=.2, paint_version=2, operation='intersect', flow=120, density=-10)])),
        ('pv2_invert', dict(components=[{**STROKE, 'paint_version': 2, 'invert': True, 'flow': 75}])),
        ('pv1_brush', dict(components=[{**STROKE, 'paint_version': 1}, {**RADIAL, 'operation': 'intersect'}])),
        ('range_combo', dict(components=[dict(type='luma', range=[.2, .6]), dict(type='color', rgb=[.5, .4, .3], tolerance=.4, operation='intersect'),
                                         {**RADIAL, 'operation': 'subtract', 'invert': True}])),
        ('empty', dict(components=[])),
        ('none', dict()),
        ('all_disabled', dict(components=[{**RADIAL, 'enabled': False}], opacity=.5)),
        ('unknown_op', dict(components=[RADIAL, {**LINEAR_C, 'operation': 'xor'}])),
    ]
    for tname, transform in TRANSFORMS.items():
        for frame in ('garden', 'patches'):
            with Case(MODULE, f'layer_mask_{frame}_{tname}') as case:
                source = FRAMES[frame]
                image = transform(source)
                rows = []
                for name, layer in LAYERS:
                    mask = P.layer_mask(image, source.shape, layer, transform)
                    assert mask.dtype == np.float32
                    rows.append(dict(name=name, layer=layer, image_dependent=P.image_dependent(layer)))
                    exact(case, name, mask)
                case.json(cases=rows, frame=frame)


# ---------------------------------------------------------------- apply_local
def local_case(name, frame, layers, *, transform='identity', pixel_scale=1., detail_version=1, source_image=None,
               callbacks=False, step=None):
    """One apply_local case; with callbacks, the arguments on_mask, on_layer_input and on_point received."""
    source = FRAMES[frame]
    t = TRANSFORMS[transform]
    image = t(source)
    big = image.shape[0] >= 100
    step = step or (4 if big else 3)
    with Case(MODULE, f'local_{name}') as case:
        case.json(frame=frame, layers=layers, transform=transform, pixel_scale=pixel_scale,
                  detail_version=detail_version, source_image=source_image)
        kw = {}
        seen = dict(masks=[], inputs=[], points=[])
        if callbacks:
            def on_mask(index, mask):
                k = len(seen['masks'])
                seen['masks'].append(index)
                exact(case, f'cb_mask_{k}', mask, step)

            def on_layer_input(index, value):
                seen['inputs'].append([index, str(bits_sum(value))])

            def on_point(layer, i, value, mask, origin, shape):
                seen['points'].append(dict(layer=layer, point=i, image=str(bits_sum(value)), image_shape=list(value.shape),
                                           mask=str(bits_sum(mask)), mask_shape=list(mask.shape),
                                           origin=[int(origin[0]), int(origin[1])], frame=list(shape)))
            kw = dict(on_mask=on_mask, on_layer_input=on_layer_input)
            if callbacks == 'points':
                kw['on_point'] = on_point
        src = FRAMES[source_image] if source_image else None
        out = P.apply_local(image, source.shape, layers, t, source_image=src, pixel_scale=pixel_scale,
                            detail_version=detail_version, **kw)
        assert out.dtype == np.float32
        tolerant = any(l.get('enabled', True) and l.get('adjustments', {}).get('noise_luma') and
                       max(detail_version, l.get('adjustments', {}).get('detail_version', 1)) < 3 for l in layers)
        if tolerant:
            # Luminance noise reduction of detail_version 1-2 runs through the oracle's IPP bilateral filter:
            # compared with a tolerance on a subsample.
            case.json(output_shape=list(out.shape), output_step=step, tolerant=True)
            case.array('output_sub', np.ascontiguousarray(out[::step, ::step]))
        else:
            exact(case, 'output', out, step)
        case.json(callbacks=seen, callback_mode=callbacks or None)


def layer(adjustments, components=None, **more):
    return dict(components=components or [RADIAL, STROKE], adjustments=adjustments, **more)


if 'local' in SECTIONS:
    STRENGTHS = {
        'exposure': (-1.3, .7), 'contrast': (45, -60), 'temperature': (35, -50), 'tint': (30, -45),
        'saturation': (60, -100), 'shadows': (70, -40), 'highlights': (-80, 50), 'whites': (45, -35),
        'blacks': (-40, 30), 'hue': (30, -150), 'clarity': (60, -45), 'texture': (50, -60), 'dehaze': (40, -35),
        'noise_luma': (35, 90), 'noise_color': (40, 85), 'sharpen': (45, 120), 'defringe': (50, 100),
    }
    SPATIAL = {'clarity', 'texture', 'dehaze', 'noise_luma', 'noise_color', 'sharpen'}
    for key, values in STRENGTHS.items():
        for k, value in enumerate(values):
            frame = 'garden' if key in SPATIAL else ('patches' if k else 'colours')
            local_case(f'{key}_{k}', frame, [layer({key: value})], detail_version=3)
            if key in SPATIAL:
                local_case(f'{key}_{k}_ps025', frame, [layer({key: value})], pixel_scale=.25, detail_version=3)
    for key in ('noise_luma', 'sharpen'):
        for version in (1, 2, 3):
            local_case(f'{key}_v{version}', 'garden', [layer({key: 60})], detail_version=version)
            local_case(f'{key}_v{version}_ps025', 'garden', [layer({key: 60})], pixel_scale=.25, detail_version=version)
    local_case('noise_luma_own_version', 'garden', [layer({'noise_luma': 50, 'detail_version': 3})], detail_version=1)
    S_CURVE = [[0, 0], [.25, .18], [.5, .5], [.75, .82], [1, 1]]
    local_case('curves_master', 'garden', [layer({'local_curves': [S_CURVE]})])
    local_case('curves_channels', 'colours', [layer({'local_curves': [[[0, 0], [1, 1]], [[0, .1], [.6, .5], [1, 1]],
                                                                       [[0, 0], [.5, .6], [1, .9]], [[0, 0], [1, .8]]]})])
    local_case('curves_identity', 'colours', [layer({'local_curves': [[[0, 0], [.5, .5], [1, 1]]]})])
    HSL = [[0, 0, 0]] * 8
    HSL = [list(g) for g in HSL]
    HSL[0] = [20, -30, 10]
    HSL[3] = [-40, 50, -20]
    HSL[5] = [0, 0, 30]
    local_case('hsl', 'colours', [layer({'hsl': HSL})])
    local_case('hsl_odd', 'odd', [layer({'hsl': HSL})])
    local_case('hsl_zero', 'colours', [layer({'hsl': [[0, 0, 0]] * 8})])
    POINTS = [dict(rgb=[.8, .3, .2], hue=20, saturation=-30, lightness=10),
              dict(version=2, rgb=[.2, .5, .7], range=.2, saturation_range=.5, lightness_range=.6, hue=-40, saturation=30, lightness=-15)]
    local_case('point_colors', 'colours', [layer({'point_colors': POINTS})])
    local_case('point_colors_odd', 'odd', [layer({'point_colors': POINTS[1:]})])
    # HDR: highlights above 1 carried through the colour tools and the detail tools.
    for name, adj in [('exposure', {'exposure': 1}), ('contrast', {'contrast': 40, 'saturation': 30}),
                      ('texture', {'texture': 50, 'exposure': .5}), ('noise', {'noise_luma': 60, 'noise_color': 40}),
                      ('clarity', {'clarity': 50, 'hue': 40}), ('points', {'point_colors': POINTS, 'local_curves': [S_CURVE]}),
                      ('sharpen', {'sharpen': 80, 'dehaze': 30})]:
        local_case(f'hdr_{name}', 'wide', [layer({**adj, 'hdr': True})], detail_version=3)
    local_case('hdr_garden_ps025', 'garden', [layer({'hdr': True, 'texture': 40, 'exposure': 1.5})], pixel_scale=.25, detail_version=3)
    # Realistic combinations.
    PORTRAIT = {'exposure': .4, 'contrast': 20, 'saturation': -20, 'clarity': -30, 'sharpen': 30, 'noise_color': 20}
    SKY = {'temperature': -20, 'tint': 10, 'highlights': -60, 'dehaze': 25, 'hsl': HSL, 'local_curves': [S_CURVE]}
    ALL = {'exposure': -.3, 'contrast': 15, 'temperature': 10, 'tint': -5, 'saturation': 25, 'shadows': 30,
           'highlights': -20, 'whites': 10, 'blacks': -5, 'hue': 8, 'hsl': HSL, 'point_colors': POINTS,
           'local_curves': [S_CURVE], 'clarity': 20, 'texture': 15, 'dehaze': 10, 'noise_luma': 25,
           'noise_color': 30, 'sharpen': 40}
    for ps in (1., .25):
        for version in (1, 2, 3):
            tag = f'v{version}_ps{int(ps * 100):03d}'
            local_case(f'combo_portrait_{tag}', 'garden', [layer(PORTRAIT)], pixel_scale=ps, detail_version=version)
            local_case(f'combo_all_{tag}', 'garden', [layer(ALL)], pixel_scale=ps, detail_version=version)
        local_case(f'combo_sky_ps{int(ps * 100):03d}', 'garden', [layer(SKY, [LINEAR_C])], pixel_scale=ps)
    local_case('combo_odd', 'odd', [layer(PORTRAIT, [dict(type='radial', points=[[.1, .1], [.9, .8]])])])
    local_case('combo_cropflip', 'garden', [layer(ALL)], transform='cropflip', detail_version=3)
    local_case('combo_edges_full', 'edges', [layer({'clarity': 40, 'texture': 30, 'sharpen': 50}, [dict(type='all')])])
    # Several layers: a later range mask reads the earlier layers' result.
    SEQUENCE = [
        layer({'exposure': 1.2}, [RADIAL]),
        layer({'saturation': -80, 'contrast': 30}, [dict(type='luma', range=[.55, 1], softness=.15)]),
        layer({'texture': 40, 'temperature': 20}, [dict(type='color', samples=[[.6, .7, .5], [.3, .4, .2]], tolerance=.25), {**STROKE, 'operation': 'intersect', 'invert': True}]),
        layer({'clarity': -30}, [dict(type='brush', points=[[.5, .5]], radius=.1, auto_mask=True, auto_tolerance=.3)]),
    ]
    local_case('sequence', 'garden', SEQUENCE, detail_version=3, source_image='garden')
    local_case('sequence_cropflip', 'garden', SEQUENCE[:3], transform='cropflip', detail_version=3)
    # Disabled layers, with and without on_mask; the callbacks' arguments.
    DISABLED = [layer({'exposure': 1}, [RADIAL], enabled=False), layer({'contrast': 30}, [STROKE]),
                layer({'saturation': 50}, [LINEAR_C], enabled=False), layer({'shadows': 40}, [dict(type='luma', range=[0, .3])])]
    local_case('disabled', 'garden', DISABLED)
    local_case('disabled_on_mask', 'garden', DISABLED, callbacks=True)
    local_case('callbacks_points', 'colours', [layer({'point_colors': POINTS}), layer({'exposure': .5}, [LINEAR_C]),
                                               layer({'hue': 20}, [RADIAL], enabled=False)], callbacks='points')
    local_case('empty_mask', 'patches', [layer({'exposure': 1}, [dict(type='brush', points=[[4, 4]])])], callbacks='points')
    local_case('no_layers', 'patches', [])


# ---------------------------------------------------------------- apply_retouch
if 'retouch' in SECTIONS:
    SPOT = [dict(x=.3, y=.4, rx=.02, ry=.03), dict(x=.7, y=.6, rx=.015, ry=.01, scale=150)]
    RETOUCH = [
        ('red_eye', 'red', [dict(type='red_eye', points=[[40 / 95, 30 / 63]], radius=.12), dict(type='red_eye', points=[[70 / 95, 20 / 63]], radius=.1, feather=20)]),
        ('clone_inside', 'garden', [dict(type='clone', points=[[.3, .4], [.35, .45]], source=[.6, .5], radius=.03)]),
        ('clone_border', 'garden', [dict(type='clone', points=[[.1, .9]], source=[.98, .02], radius=.04)]),
        ('clone_offframe', 'noise', [dict(type='clone', points=[[.2, .3]], source=[1.2, -.1], radius=.05)]),
        ('clone_default_source', 'garden', [dict(type='clone', points=[[.5, .5], [.6, .5]], radius=.02)]),
        ('heal_v1', 'garden', [dict(type='heal', points=[[.4, .6], [.45, .62]], source=[.7, .3], radius=.03)]),
        ('heal_v1_border', 'noise', [dict(points=[[.02, .05]], source=[.5, .5], radius=.06, feather=80)]),
        ('heal_v2_donor', 'garden', [dict(type='heal', version=2, points=[[.4, .6], [.45, .62]], source=[.7, .3], radius=.03)]),
        ('heal_v2_border', 'garden', [dict(type='heal', version=2, points=[[.97, .03]], source=[.5, .5], radius=.04)]),
        ('heal_v2_auto', 'garden', [dict(type='heal', version=2, points=[[.5, .5], [.55, .52]], radius=.03)]),
        ('heal_v2_auto_corner', 'garden', [dict(type='heal', version=2, points=[[.01, .02]], radius=.05)]),
        ('heal_v2_auto_big', 'edges', [dict(type='heal', version=2, points=[[.5, .5]], radius=.45, feather=10)]),
        ('heal_v2_auto_all', 'noise', [dict(type='heal', version=2, points=[[.5, .5]], radius=2., feather=0)]),
        ('inpaint', 'garden', [dict(type='inpaint', points=[[.6, .3], [.65, .35]], radius=.03)]),
        ('inpaint_border', 'noise', [dict(type='inpaint', points=[[0, 1]], radius=.08)]),
        ('dust', 'garden', [dict(type='dust', spots=SPOT)]),
        ('dust_texture', 'garden', [dict(type='dust', spots=SPOT[:1], repair_method='texture'), dict(type='clone', points=[[.3, .4]], source=[.32, .4], radius=.02)]),
        ('disabled', 'garden', [dict(type='clone', points=[[.3, .4]], source=[.6, .5], enabled=False)]),
        ('offframe_stroke', 'garden', [dict(type='heal', points=[[3, 3]], radius=.03)]),
        ('stacked', 'garden', [dict(type='clone', points=[[.2, .2]], source=[.25, .3], radius=.04),
                               dict(type='heal', points=[[.22, .22], [.3, .25]], source=[.5, .6], radius=.02),
                               dict(type='heal', version=2, points=[[.6, .6]], source=[.65, .7], radius=.03),
                               dict(type='red_eye', points=[[.6, .6]], radius=.05),
                               dict(type='inpaint', points=[[.2, .2]], radius=.02),
                               dict(type='heal', version=2, points=[[.8, .3]], radius=.025)]),
        ('noise_negative', 'noise', [dict(type='heal', points=[[.5, .5]], source=[.3, .3], radius=.1)]),
        ('empty', 'noise', []),
    ]
    for name, frame, operations in RETOUCH:
        with Case(MODULE, f'retouch_{name}') as case:
            source = LINEAR[frame]
            if name == 'noise_negative':
                source = source - np.float32(.3)
            case.json(frame=frame, operations=operations, shift=-.3 if name == 'noise_negative' else 0)
            with recording(case):
                try:
                    out, error = P.apply_retouch(source, operations), None
                except Exception as e:   # noqa: BLE001 - the oracle's error is the golden
                    out, error = None, f'{type(e).__name__}: {e}'

            case.json(error=error)
            if out is not None:
                assert out.dtype == np.float32
                exact(case, 'output', out, 4 if out.shape[0] >= 100 else 3)


# ---------------------------------------------------------------- healing
if 'healing' in SECTIONS:
    garden = LINEAR['garden']
    big = np.ascontiguousarray(np.tile(garden, (4, 3, 1)))      # 640x720: the search runs on a 512-pixel copy
    OFFSETS = [
        ('centre', 'garden', [[.5, .5], [.55, .52]], .03),
        ('corner', 'garden', [[.01, .02]], .05),
        ('edge', 'garden', [[.5, .99]], .04),
        ('big', 'garden', [[.5, .5]], .4),
        ('all', 'garden', [[.5, .5]], 2.),
        ('long', 'garden', [[.1, .5], [.9, .5]], .02),
        ('tiled', 'big', [[.3, .6], [.32, .61]], .01),
        ('noise', 'noise', [[.4, .4]], .05),
    ]
    with Case(MODULE, 'healing_offsets') as case:
        rows = []
        with recording(case):
            for name, frame, points, radius in OFFSETS:
                image = big if frame == 'big' else LINEAR[frame]
                mask = P.brush_mask(image.shape, points, radius, 60)
                try:
                    offset, error = healing.automatic_offset(image, mask), None
                except Exception as e:   # noqa: BLE001 - the oracle's error is the golden
                    offset, error = None, f'{type(e).__name__}: {e}'
                rows.append(dict(name=name, frame=frame, points=points, radius=radius, error=error,
                                 offset=None if offset is None else [float(offset[0]), float(offset[1])]))
        case.json(cases=rows)
        t = np.linspace(0, 2 * np.pi, 24, endpoint=False)
        case.array('directions', np.stack([np.cos(t), np.sin(t)], -1))

    with Case(MODULE, 'healing_blend') as case:
        target = np.ascontiguousarray(garden[40:80, 60:120])
        donor = np.ascontiguousarray(garden[90:130, 150:210])
        weights = {
            'stroke': P.brush_mask(target.shape, [[.3, .4], [.7, .6]], .2, 60),
            'none': np.zeros(target.shape[:2], np.float32),
            'all': np.ones(target.shape[:2], np.float32),
            'speck': P.brush_mask(target.shape, [[.1, .9]], .05, 0),
        }
        with recording(case):
            for name, weight in weights.items():
                case.array(f'weight_{name}', weight)
                exact(case, f'blend_{name}', healing.blend_boundary(target, donor, weight[..., None]))
                exact(case, f'inpaint_{name}', healing.inpaint(target, weight[..., None]))
        case.json(weights=list(weights))


# ---------------------------------------------------------------- range masks
if 'range' in SECTIONS:
    with Case(MODULE, 'range_coverage') as case:
        CASES = [('one', 'garden', [[.3, .4, .2]], .15), ('three', 'colours', [[.2, .2, .8], [.9, .5, .1], [.5, .5, .5]], .3),
                 ('five', 'patches', [[.1, .2, .3], [.4, .5, .6], [.7, .8, .9], [.2, .9, .1], [1, 0, 0]], .05),
                 ('tiny_tol', 'patches', [[.4, .4, .4]], .001), ('wide', 'wide', [[1.5, 1.5, 1.5]], 1)]
        rows = []
        for name, frame, samples, tolerance in CASES:
            rows.append(dict(name=name, frame=frame, samples=samples, tolerance=tolerance))
            exact(case, name, range_mask.color_coverage(FRAMES[frame], samples, tolerance))
        case.json(cases=rows)

    _develop = engine.develop
    _geometry = engine.geometry

    def selection_case(name, settings, points, layer=None, *, use_crop=True):
        source = LINEAR['garden']
        seen = dict(inputs=[], geometry=[], settings=[])

        def develop(src, s, use_crop=True, output_space='sRGB', **kw):
            read = kw.get('on_layer_input')
            seen['settings'].append(dict(masks=len(s['masks']), panels_off=list(s.get('panels_off') or [])))

            def recorded(index, image):
                seen['inputs'].append((index, np.array(image)))
                seen['reading'] = True      # geometry calls from here on are the selection's valid area
                try:
                    read(index, image)
                finally:
                    seen['reading'] = False
            kw['on_layer_input'] = recorded
            return _develop(src, s, use_crop, output_space, **kw)

        def geometry(a, s, use_crop=True, copy=True):
            out = _geometry(a, s, use_crop, copy)
            if seen.get('reading'):
                seen['geometry'].append(np.array(out))
            return out
        engine.develop, engine.geometry = develop, geometry
        try:
            try:
                result = range_mask.sample_selection(source, settings, points, layer, use_crop=use_crop)
                error = None
            except Exception as e:   # noqa: BLE001
                result, error = None, f'{type(e).__name__}: {e}'
        finally:
            engine.develop, engine.geometry = _develop, _geometry
        with Case(MODULE, f'range_select_{name}') as case:
            # The selection reads only the rectangle of the frame: stored are that window of the selected layer's
            # input and of the valid-area map (the replay fills the rest of the frame with zeros) and bit sums.
            frames = []
            for k, (index, image) in enumerate(seen['inputs']):
                h, w = image.shape[:2]
                start, end = np.asarray(points[0], np.float64), np.asarray(points[-1], np.float64)
                x0, x1 = sorted(min(w - 1, int(p[0] * w)) for p in (start, end))
                y0, y1 = sorted(min(h - 1, int(p[1] * h)) for p in (start, end))
                window = [y0, y1 + 1, x0, x1 + 1]
                frames.append(dict(index=index, shape=list(image.shape), window=window, bits=str(bits_sum(image))))
                if index == (len(settings.get('masks', [])) if layer is None else layer):
                    case.array(f'input_{k}', np.ascontiguousarray(image[y0:y1 + 1, x0:x1 + 1]))
                    for j, valid in enumerate(seen['geometry']):
                        case.array(f'geometry_{j}', np.ascontiguousarray(valid[y0:y1 + 1, x0:x1 + 1]))
                        frames[-1]['geometry_shape'] = list(valid.shape)
            case.json(settings=settings, points=points, layer=layer, source_shape=list(source.shape[:2]),
                      inputs=frames, geometry=len(seen['geometry']),
                      develop_settings=seen['settings'], result=result, error=error)

    BASE = dict(engine.defaults())
    MASKED = {**BASE, 'masks': [layer({'exposure': 1}, [RADIAL]), layer({'saturation': -60}, [STROKE], enabled=False)],
              'panels_off': ['masks', 'detail']}
    selection_case('point_new', BASE, [[.5, .5]])
    selection_case('rect_new', MASKED, [[.2, .3], [.6, .7]])
    selection_case('rect_layer0', MASKED, [[.7, .8], [.1, .2]], 0)
    selection_case('rect_layer1', MASKED, [[.0, .0], [1., 1.]], 1)
    selection_case('flat_patch', {**BASE, 'exposure': -5}, [[.1, .1], [.3, .3]])
    ROTATED = {**BASE, 'straighten': 8}
    selection_case('rotated_inside', ROTATED, [[.4, .4], [.6, .6]])
    selection_case('rotated_corner', ROTATED, [[0, 0], [.02, .02]])
    selection_case('bad_layer', MASKED, [[.5, .5]], 5)
    selection_case('bad_points', BASE, [[.5, 1.5]])
