"""Golden cases for grainy-geometry: engine.geometry, processing.optical_geometry, straighten, upright,
scan_border, rasterio.orient_pixels and the pixel path of optics (lens_shading, lens_geometry).

Run with the oracle's interpreter from the worktree root:
    "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_geometry.py

Inputs are the standard images (common.inputs) or `synth(h, w, c, seed)`, a float32 formula the Rust tests
rebuild exactly, so only results are stored. The CPU path is the reference (GRAINY_GPU=0): the generator checks
that native_gpu.rotate_any/optical_any return None, so straighten is Pillow's bicubic rotation.

Two oracle internals are observed, never changed on disk:
* upright.estimate's LSD detector (cv2.createLineSegmentDetector) is wrapped to record the detected segments,
  or replaced by a fake that returns given segments (branch coverage of the estimation maths);
* optics._modifier is wrapped to record every lensfunpy Modifier call (arguments, coordinate maps, colour
  results), so the Rust tests replay lensfun through their LensModel; optics.MAP_BUDGET is lowered for the
  strip cases so that the 128-row strip path runs on small frames.
"""
import shutil
import sys

sys.dont_write_bytecode = True

from common import Case, GOLDEN, inputs, np, settings as full_settings, write_json  # noqa: E402
import cv2  # noqa: E402
from luma import engine, native_gpu, optics, processing, rasterio, scan_border, straighten, upright  # noqa: E402

MODULE = 'geometry'
STANDARD = inputs()
NAMES = set()

assert not native_gpu.enabled()
assert native_gpu.rotate_any(np.zeros((4, 5, 3), np.float32), 3.0) is None
assert native_gpu.rotate_any(np.zeros((4, 5, 1), np.float32), 3.0) is None
assert native_gpu.optical_any(np.zeros((4, 5, 3), np.float32), {'distortion': 5}) is None
assert native_gpu.optical_any(np.zeros((4, 5, 1), np.float32), {'distortion': 5}) is None


def synth(h, w, c, seed=0):
    """Deterministic float32 test frame (rebuilt by the Rust tests): a fine pattern plus a ramp."""
    yy, xx, cc = np.mgrid[:h, :w, :c]
    k = ((7 * xx + 13 * yy + 5 * cc + 3 * seed) % 101).astype(np.float32)
    return (k * np.float32(.015) + xx.astype(np.float32) * np.float32(.002)).astype(np.float32)


def source(spec):
    if spec['kind'] == 'synth':
        return synth(spec['h'], spec['w'], spec['c'], spec.get('seed', 0))
    a = STANDARD[spec['name']]
    return np.ascontiguousarray(a[..., 1:2]) if spec.get('channels') == 1 else a


def S(name, channels=3):
    return {'kind': 'standard', 'name': name, 'channels': channels}


def Y(h, w, c, seed=0):
    return {'kind': 'synth', 'h': h, 'w': w, 'c': c, 'seed': seed}


def case(name):
    assert name not in NAMES, name
    NAMES.add(name)
    return Case(MODULE, name)


def error_of(function):
    try:
        return function(), None
    except (ValueError, upright.AlignmentError) as error:
        return None, {'type': type(error).__name__, 'message': str(error)}


def run_geometry(name, spec, changes, use_crop=True, function='geometry'):
    """One engine.geometry (or processing.optical_geometry) case: settings are the oracle defaults + changes."""
    a = source(spec)
    s = full_settings(**changes)
    if function == 'geometry':
        result, error = error_of(lambda: engine.geometry(a.copy(), s, use_crop))
    else:
        result, error = error_of(lambda: processing.optical_geometry(a.copy(), s))
    with case(name) as c:
        c.json(input=spec, changes=changes, use_crop=use_crop, function=function, error=error)
        if result is not None:
            result = np.asarray(result)
            assert result.dtype == np.float32 and result.ndim == 3, (name, result.dtype, result.shape)
            c.array('output', result)


# ---------------------------------------------------------------------------------------------------------
# Quarter turns, flip and crop: pixels only move (tolerance 0).

CROPS = {'none': None, 'inner': [.1, .2, .8, .9], 'full': [0, 0, 1, 1], 'border': [-.2, .5, 1.3, 1.1],
         'point': [.5, .5, .5, .5], 'corner': [1, 1, 1, 1], 'sliver': [.95, 0, 1, .02], 'half': [0.25, 0.25, 0.75, 0.75]}


def orientation_cases():
    for channels in (1, 3):
        spec = Y(13, 21, channels, 1)
        for rotation in (0, 1, 2, 3, -1, 6):
            for flip in (False, True):
                for label, crop in CROPS.items():
                    run_geometry(f'orient_c{channels}_r{rotation}_f{int(flip)}_{label}', spec,
                                 {'rotation': rotation, 'flip': flip, 'crop': crop})
    run_geometry('orient_garden_r1_flip_crop', S('garden'), {'rotation': 1, 'flip': True, 'crop': [.2, .1, .7, .6]})
    run_geometry('orient_odd_r3_crop', Y(77, 123, 1, 4), {'rotation': 3, 'crop': [.013, .5, .999, .77]})
    run_geometry('orient_nocrop_tool', Y(13, 21, 3, 1), {'rotation': 1, 'crop': [.1, .2, .8, .9]}, use_crop=False)
    run_geometry('orient_all_tool', Y(13, 21, 3, 1), {'rotation': 2, 'flip': True, 'crop': [.1, .2, .8, .9]}, use_crop='all')
    run_geometry('noop_defaults', S('edges'), {})


# ---------------------------------------------------------------------------------------------------------
# Straighten: Pillow's bicubic affine rotation per channel ('F' images), with the crop tool's padding.

def straighten_cases():
    small = Y(37, 53, 1, 2)
    for angle in (0.3, -2.7, 12.5, -44.9, 45.0, 90.0, -90.0, 180.0, 360.0, 1e-4, 270.0, -137.25):
        for use_crop in (True, 'all'):
            tag = 'all' if use_crop == 'all' else 'crop'
            run_geometry(f'straighten_small_{angle:g}_{tag}', small, {'straighten': angle}, use_crop)
    square = Y(40, 40, 1, 3)
    for angle in (90.0, -90.0, 180.0, 270.0, 360.0, 33.0):
        run_geometry(f'straighten_square_{angle:g}', square, {'straighten': angle})
    for angle in (1.5, -7.25):
        for use_crop in (True, 'all'):
            tag = 'all' if use_crop == 'all' else 'crop'
            run_geometry(f'straighten_edges_{angle:g}_{tag}', S('edges'), {'straighten': angle}, use_crop)
            run_geometry(f'straighten_odd_{angle:g}_{tag}', Y(123, 77, 3, 5), {'straighten': angle}, use_crop)
    run_geometry('straighten_large_frame', Y(200, 301, 1, 6), {'straighten': -3.3})
    run_geometry('straighten_garden_crop', S('garden'), {'straighten': 4.0, 'crop': [.25, .2, .75, .7]})
    run_geometry('straighten_turn_flip_crop', Y(37, 53, 3, 7), {'straighten': -6.0, 'rotation': 1, 'flip': True,
                                                              'crop': [.1, .05, .9, .8]})
    run_geometry('straighten_turn_flip_all', Y(37, 53, 3, 7), {'straighten': 8.0, 'rotation': 3, 'flip': True,
                                                             'crop': [.1, .05, .9, .8]}, 'all')
    run_geometry('straighten_wide_values', S('wide'), {'straighten': 3.0})


# ---------------------------------------------------------------------------------------------------------
# Manual transform (processing.optical_geometry) and guides.

TRANSFORMS = {
    'distortion_pos': {'distortion': 12.0}, 'distortion_neg': {'distortion': -30.0},
    'k2': {'distortion_k2': 20.0}, 'k3': {'distortion_k3': -15.0},
    'persp_h': {'perspective_h': 40.0}, 'persp_v': {'perspective_v': -60.0}, 'persp_clamp': {'perspective_h': 150.0, 'perspective_v': 120.0},
    'aspect': {'aspect_scale': 25.0}, 'shift_x': {'shift_x': 10.0}, 'shift_y': {'shift_y': -7.0},
    'scale_80': {'transform_scale': 80.0}, 'scale_clamp': {'transform_scale': 10.0}, 'scale_int': {'transform_scale': 130},
    'ca_red': {'ca_red': 30.0}, 'ca_blue': {'ca_blue': -25.0}, 'ca_both': {'ca_red': 40.0, 'ca_blue': 40.0},
    'combined': {'distortion': -8.0, 'distortion_k2': 3.0, 'distortion_k3': 1.0, 'perspective_h': -20.0, 'perspective_v': 15.0,
                 'aspect_scale': -10.0, 'shift_x': -3.0, 'shift_y': 4.0, 'transform_scale': 110.0},
    'combined_ca': {'distortion': 6.0, 'perspective_v': 25.0, 'transform_scale': 95.0, 'ca_red': -15.0, 'ca_blue': 20.0},
}
GUIDES = [[.05, .1], [.9, .02], [.97, .95], [.02, .9]]


def optical_cases():
    for channels in (1, 3):
        spec = Y(37, 53, channels, 8)
        for label, changes in TRANSFORMS.items():
            run_geometry(f'optical_c{channels}_{label}', spec, changes, function='optical')
        run_geometry(f'optical_c{channels}_guides', spec, {'guides': GUIDES}, function='optical')
        run_geometry(f'optical_c{channels}_guides_bad', spec, {'guides': GUIDES[:3]}, function='optical')
        run_geometry(f'optical_c{channels}_guides_distortion', spec, {'guides': GUIDES, 'distortion': 10.0, 'ca_red': 12.0},
                     function='optical')
    run_geometry('optical_noop', Y(37, 53, 3, 8), {'transform_scale': 100.0, 'distortion': 0}, function='optical')
    run_geometry('optical_edges_combined', S('edges'), TRANSFORMS['combined_ca'], function='optical')
    run_geometry('optical_odd_combined', Y(77, 123, 3, 9), TRANSFORMS['combined'], function='optical')
    run_geometry('geometry_optical_turn_straighten_crop', Y(37, 53, 3, 10),
                 dict(TRANSFORMS['combined_ca'], rotation=1, flip=True, straighten=2.5, crop=[.1, .1, .9, .85]))
    run_geometry('geometry_guides_mask', Y(53, 37, 1, 11), {'guides': GUIDES, 'straighten': -1.25, 'crop': [0, .1, .95, 1]})


# ---------------------------------------------------------------------------------------------------------
# Upright: apply (through geometry and directly) and the stored matrix checks.

def keystone_matrix(angle=2.0, k=.15):
    t = np.radians(angle)
    rotation = np.array([[np.cos(t), -np.sin(t), .02], [np.sin(t), np.cos(t), -.01], [0, 0, 1]])
    keystone = np.array([[1, 0, 0], [0, 1, 0], [0, k, 1]])
    return (keystone @ rotation).tolist()


def upright_cases():
    good = {'matrix': keystone_matrix(), 'crop_scale': 1.3}
    spec = Y(37, 53, 3, 12)
    for label, changes in {
        'crop': {'upright': good}, 'nocrop': {'upright': good, 'upright_crop': False},
        'zoom_max': {'upright': {'matrix': keystone_matrix(-3, -.2), 'crop_scale': 2.6}},
        'zoom_default': {'upright': {'matrix': keystone_matrix(1, .05)}},
        'straighten_no_pad': {'upright': good, 'straighten': 3.0},
        'bad_shape': {'upright': {'matrix': [[1, 0, 0], [0, 1, 0]], 'crop_scale': 1}},
        'bad_null': {'upright': {'matrix': [[1, 0, None], [0, 1, 0], [0, 0, 1]], 'crop_scale': 1}},
        'bad_singular': {'upright': {'matrix': [[1, 2, 0], [2, 4, 0], [0, 0, 1]], 'crop_scale': 1}},
        'bad_zoom_low': {'upright': {'matrix': keystone_matrix(), 'crop_scale': .9}},
        'bad_zoom_high': {'upright': {'matrix': keystone_matrix(), 'crop_scale': 3.0}},
        'bad_zoom_ignored': {'upright': {'matrix': keystone_matrix(), 'crop_scale': 3.0}, 'upright_crop': False},
    }.items():
        run_geometry(f'upright_{label}', spec, changes)
        if label == 'straighten_no_pad':
            run_geometry(f'upright_{label}_all', spec, changes, 'all')
    run_geometry('upright_mask', Y(41, 29, 1, 13), {'upright': good})
    run_geometry('upright_tiny', Y(1, 1, 3, 13), {'upright': good})
    run_geometry('upright_edges_all', S('edges'), {'upright': good, 'rotation': 1, 'crop': [.1, .1, .9, .9]})


# ---------------------------------------------------------------------------------------------------------
# Upright estimation. Real LSD segments on synthetic line images, and fake detectors on chosen segments.

class Recorder:
    """Wraps cv2.createLineSegmentDetector: returns the real detector and remembers what it found."""

    def __init__(self, fake=None):
        self.found = None
        self.fake = fake
        self.called = False

    def __call__(self, *args):
        real = ORIGINAL_LSD(*args)
        recorder = self

        class Detector:
            def detect(self, gray):
                recorder.called = True
                if recorder.fake is not None:
                    recorder.found = recorder.fake
                    return recorder.fake, None, None, None
                result = real.detect(gray)
                recorder.found = result[0]
                return result
        return Detector()


ORIGINAL_LSD = cv2.createLineSegmentDetector


def scene(h, w, segments, angle=0., keystone=0., thickness=2, background=210, seed=0):
    """A grey line drawing, turned by `angle` degrees and keystoned, as a linear float32 RGB frame."""
    image = np.full((h, w), background, np.uint8)
    for x0, y0, x1, y1, value in segments:
        cv2.line(image, (int(x0), int(y0)), (int(x1), int(y1)), int(value), thickness, cv2.LINE_AA)
    src = np.float32([[0, 0], [w - 1, 0], [w - 1, h - 1], [0, h - 1]])
    inset = keystone * w
    dst = np.float32([[inset, 0], [w - 1 - inset, 0], [w - 1, h - 1], [0, h - 1]])
    matrix = cv2.getPerspectiveTransform(src, dst)
    image = cv2.warpPerspective(image, matrix, (w, h), flags=cv2.INTER_LINEAR, borderMode=cv2.BORDER_REPLICATE)
    turn = cv2.getRotationMatrix2D(((w - 1) / 2, (h - 1) / 2), angle, 1.0)
    image = cv2.warpAffine(image, turn, (w, h), flags=cv2.INTER_LINEAR, borderMode=cv2.BORDER_REPLICATE)
    rng = np.random.default_rng(seed)
    noisy = np.clip(image.astype(np.float32) / 255 + rng.normal(0, .004, image.shape).astype(np.float32), 0, 1)
    return np.repeat(engine.to_linear(noisy)[..., None], 3, axis=2).astype(np.float32)


def facade(h=240, w=320):
    segments = []
    for x in range(40, w - 30, 30):
        segments.append((x, 25, x, h - 25, 40))
    for y in range(35, h - 30, 24):
        segments.append((25, y, w - 25, y, 60))
    return segments


def horizon(h=240, w=320, tilt=2.0, dark=40):
    image = np.full((h, w), 225, np.uint8)
    t = np.tan(np.radians(tilt))
    yy, xx = np.mgrid[:h, :w]
    image[yy > h * .55 + (xx - w / 2) * t] = dark
    return np.repeat(engine.to_linear(image.astype(np.float32) / 255)[..., None], 3, axis=2).astype(np.float32)


def stripes(h=480, w=640, count=200):
    segments = [(int(10 + i * (w - 20) / count), 20, int(10 + i * (w - 20) / count + 6), h - 20, 30 if i % 2 else 90)
                for i in range(count)]
    return segments


def filtered(found, h, w):
    """estimate's segment selection (the input of the estimation maths)."""
    lines = found.reshape(-1, 2, 2).astype(float)
    lengths = np.linalg.norm(lines[:, 1] - lines[:, 0], axis=1)
    keep = lengths >= max(20, min(h, w) * .06)
    lines = lines[keep]; lengths = lengths[keep]
    if len(lines) > 160:
        order = np.argsort(lengths)[-160:]; lines = lines[order]; lengths = lengths[order]
    return lines, lengths


def record_estimate(name, image=None, shape=None, fake=None, modes=('auto', 'level', 'vertical', 'full')):
    if image is None:
        image = np.zeros(shape + (3,), np.float32)
    h, w = image.shape[:2]
    for mode in modes:
        recorder = Recorder(fake)
        cv2.createLineSegmentDetector = recorder
        try:
            result, error = error_of(lambda: upright.estimate(image, mode))
        finally:
            cv2.createLineSegmentDetector = ORIGINAL_LSD
        with case(f'estimate_{name}_{mode}') as c:
            c.json(width=w, height=h, mode=mode, result=result, error=error, detected=recorder.called,
                   lines_none=recorder.called and recorder.found is None)
            if recorder.found is not None:
                c.array('lines', np.asarray(recorder.found, np.float32).reshape(-1, 4))
    return recorder.found


def internals(name, found, h, w):
    """Direct results of the estimation helpers on estimate's own selection of segments."""
    lines, lengths = filtered(np.asarray(found, np.float32), h, w)
    out = {}
    out['roll'] = error_of(lambda: list(upright._roll(lines, lengths)))
    out['horizon_roll'] = error_of(lambda: list(upright._horizon_roll(lines, lengths, w, h)))
    out['plumb_rolls'] = error_of(lambda: upright._plumb_rolls(lines, lengths, w, h))
    out['wide_roll'] = error_of(lambda: (lambda r: None if r is None else list(r))(upright._wide_roll(lines, lengths, w, h)))
    out['level'] = error_of(lambda: list(upright._level(lines, lengths, w, h)))
    for vertical in (True, False):
        delta = lines[:, 1] - lines[:, 0]
        if vertical:
            select = np.abs(delta[:, 1]) > np.abs(delta[:, 0]) * 1.5
        else:
            select = np.abs(delta[:, 0]) > np.abs(delta[:, 1]) * 1.5
        scale = max(w, h) / 2
        normal = np.array([[1 / scale, 0, -(w - 1) / 2 / scale], [0, 1 / scale, -(h - 1) / 2 / scale], [0, 0, 1]], float)
        leveled = upright._points(normal, lines)

        def vanishing():
            point, count, confidence = upright._vanishing(leveled[select], lengths[select], vertical)
            return [point.tolist(), count, confidence]
        out[f'vanishing_{"v" if vertical else "h"}'] = error_of(vanishing)
    with case(f'internals_{name}') as c:
        c.json(width=w, height=h, results={k: {'value': v[0], 'error': v[1]} for k, v in out.items()})
        c.array('lines', np.asarray(found, np.float32).reshape(-1, 4))


def fake_lines(seed, groups, h, w):
    """Segments in N×1×4 float32 (as LSD returns them): groups of (count, angle°, spread°, x-range, y-range, length)."""
    rng = np.random.default_rng(seed)
    result = []
    for count, angle, spread, xr, yr, length in groups:
        for _ in range(count):
            a = np.radians(angle + rng.normal(0, spread))
            cx, cy = rng.uniform(*xr) * w, rng.uniform(*yr) * h
            half = rng.uniform(*length) / 2
            dx, dy = np.cos(a) * half, np.sin(a) * half
            result.append([cx - dx, cy - dy, cx + dx, cy + dy])
    return np.asarray(result, np.float32).reshape(-1, 1, 4)


def estimate_cases():
    image = scene(240, 320, facade(), angle=2.5, keystone=.12, seed=1)
    found = record_estimate('facade', image)
    internals('facade', found, 240, 320)
    image = scene(240, 320, facade(), angle=-4.0, seed=2)
    found = record_estimate('grid', image)
    internals('grid', found, 240, 320)
    image = scene(240, 320, facade(), keystone=.3, seed=3)
    found = record_estimate('strong_keystone', image)
    internals('strong_keystone', found, 240, 320)
    found = record_estimate('horizon', horizon())
    internals('horizon', found, 240, 320)
    found = record_estimate('horizon_steep', horizon(tilt=9.0))
    internals('horizon_steep', found, 240, 320)
    image = scene(480, 640, stripes(), angle=1.0, keystone=.05, thickness=2, seed=4)
    found = record_estimate('stripes', image, modes=('auto', 'vertical'))
    internals('stripes', found, 480, 640)
    record_estimate('blank', np.full((120, 160, 3), .2, np.float32), modes=('auto', 'level'))
    record_estimate('tiny', np.zeros((30, 50, 3), np.float32), modes=('auto',))
    record_estimate('badmode', np.zeros((60, 80, 3), np.float32), modes=('sideways',))
    # Chosen segments (fake detector) for the Level branches and the vanishing-point maths.
    h, w = 400, 600
    fakes = {
        'plumbs_strong': [(8, 91.0, .2, (.05, .95), (.3, .7), (150, 250))],
        'plumbs_slanted': [(6, 93.5, .3, (.05, .95), (.3, .7), (150, 250))],
        'plumbs_horizon': [(6, 91.2, .2, (.05, .95), (.3, .7), (150, 250)), (2, 1.0, .1, (.45, .55), (.45, .55), (400, 420))],
        'few_plumbs_wide': [(2, 90.5, .1, (.2, .8), (.3, .7), (100, 140)),
                            (6, .8, .2, (.1, .9), (.2, .9), (150, 260))],
        'horizon_only': [(1, -2.0, 0, (.5, .5), (.5, .5), (500, 500)), (3, 30.0, 5, (.2, .8), (.2, .8), (40, 60))],
        'horizon_conflict': [(1, -2.0, 0, (.5, .5), (.4, .4), (500, 500)), (1, 3.0, 0, (.5, .5), (.6, .6), (500, 500))],
        'contradicted': [(1, -3.5, 0, (.5, .5), (.5, .5), (500, 500)), (4, 92.0, .2, (.3, .7), (.4, .6), (60, 90))],
        'nothing': [(5, 45.0, 3, (.2, .8), (.2, .8), (60, 90))],
        'two_lines': [(2, 90.0, .5, (.3, .7), (.4, .6), (100, 150))],
        'one_sided': [(8, 91.0, .5, (.05, .2), (.3, .7), (150, 250)), (8, 0.5, .5, (.4, .6), (.05, .2), (150, 250))],
        'mixed': [(10, 92.0, .4, (.05, .95), (.2, .8), (60, 200)), (10, -1.0, .4, (.1, .9), (.1, .9), (80, 300)),
                  (6, 40.0, 10, (.1, .9), (.1, .9), (40, 80))],
        'many': [(60, 90.5, .6, (.02, .98), (.2, .8), (60, 300)), (50, 1.5, .4, (.05, .95), (.05, .95), (80, 400))],
    }
    for seed, (label, groups) in enumerate(fakes.items()):
        found = fake_lines(100 + seed, groups, h, w)
        record_estimate(f'fake_{label}', shape=(h, w), fake=found)
        internals(f'fake_{label}', found, h, w)
    found = fake_lines(200, [(200, 90.0, .3, (.02, .98), (.2, .8), (100, 300))], h, w)
    record_estimate('fake_tail_shuffle', shape=(h, w), fake=found, modes=('vertical', 'auto'))
    # Crop scale of chosen matrices.
    scales = []
    for angle, k in [(0, 0), (2, .1), (-5, -.2), (10, .4), (25, .9), (0, 2.0)]:
        m = np.asarray(keystone_matrix(angle, k))
        for (sw, sh) in [(320, 240), (101, 333)]:
            n = np.diag([1 / (sw - 1), 1 / (sh - 1), 1])
            matrix = np.linalg.inv(n) @ m @ n
            scales.append({'matrix': matrix.tolist(), 'width': sw, 'height': sh,
                           'value': error_of(lambda: upright._crop_scale(matrix, sw, sh))})
    with case('crop_scale') as c:
        c.json(cases=scales)
    # numpy's Generator.choice used by _vanishing to sample 500 pairs.
    with case('choice42') as c:
        c.json(cases=[{'n': n, 'indices': np.random.default_rng(42).choice(n, 500, replace=False).tolist()}
                      for n in (501, 528, 1000, 4950, 10000, 10001, 12720)])


# ---------------------------------------------------------------------------------------------------------
# straighten.py over a grid.

def straighten_functions():
    records = []
    sizes = [(96, 64), (64, 96), (301, 200), (1, 1), (5, 3), (4000, 6000)]
    angles = [-45, -30, -7.5, -1, -1e-10, 0, 1e-10, .5, 3, 10, 44.9, 90, 135]
    aspects = [None, 1, 1.5, .5, 0]
    crops = [[0, 0, 1, 1], [.1, .1, .9, .9], [.3, .3, .7, .6], [0, .2, .5, .8], [-.01, 0, 1, 1], [.5, .5, .5, .6],
             [.02, .02, .98, .98], [.9, .9, 1, 1], [.45, .1, .55, .9]]
    for width, height in sizes:
        for angle in angles:
            r = {'width': width, 'height': height, 'angle': angle,
                 'padding': list(straighten.padding(angle, width, height)),
                 'largest': [straighten.largest(angle, width, height, aspect) for aspect in aspects],
                 'valid': [straighten.valid(c, angle, width, height) for c in crops],
                 'shrink': [straighten.shrink(c, angle, width, height) for c in crops],
                 'ratio': [straighten.ratio(c, width, height) for c in crops[:4]],
                 'limit': [straighten.limit(c, p, angle, width, height) for c in crops for p in ([.1, .1, .9, .9], [.4, .4, .6, .6])],
                 'refit': [straighten.refit(c, previous, angle, width, height, aspect)
                           for c in (None, [0, 0, 1, 1], [.1, .1, .9, .9], [.2, .3, .6, .7],
                                     straighten.largest(3, width, height))
                           for previous in (0, 3) for aspect in (None, 1.5)]}
            records.append(r)
    close = [[a, b, straighten.close(a, b)] for a, b in [([0, 0, 1, 1], [0, 0, 1, 1]), ([0, 0, 1, 1], [.002, 0, 1, 1]),
                                                         ([0, 0, 1, 1], [.0021, 0, 1, 1]), ([.1, .2, .3, .4], [.1, .2, .3])]]
    with case('straighten_functions') as c:
        c.json(aspects=aspects, crops=crops, records=records, close=close)


# ---------------------------------------------------------------------------------------------------------
# Scan borders.

def scan(h, w, sides, value=.03, picture=.45, blur=0, seed=0):
    """Synthetic sRGB scan: a picture with dark bands of given depths (fractions) on each side."""
    rng = np.random.default_rng(seed)
    image = (picture + rng.normal(0, .05, (h, w, 3))).astype(np.float32)
    left, right, top, bottom = sides
    border = np.zeros((h, w), bool)
    if left: border[:, :int(left * w)] = True
    if right: border[:, w - int(right * w):] = True
    if top: border[:int(top * h)] = True
    if bottom: border[h - int(bottom * h):] = True
    image[border] = value
    if blur:
        image = cv2.GaussianBlur(image, (0, 0), blur)
    return np.ascontiguousarray(image, np.float32)


def scan_cases():
    scans = {
        'all_sides': scan(80, 120, (.04, .05, .03, .06)),
        'left_right': scan(80, 120, (.06, .03, 0, 0), seed=1),
        'blurred': scan(90, 130, (.05, .05, .05, .05), blur=1.5, seed=2),
        'dark_photo': scan(80, 120, (.05, .05, .05, .05), picture=.09, seed=3),
        'too_wide': scan(80, 120, (.2, 0, 0, 0), seed=4),
        'none': scan(80, 120, (0, 0, 0, 0), seed=5),
        'tiny': scan(19, 40, (.1, .1, .1, .1), seed=6),
        'large': scan(1010, 20, (0, 0, .05, .02), seed=7),
        'narrow_result': scan(100, 100, (0, 0, .0, .0), seed=8),
        'bright_border': scan(80, 120, (.05, .05, .05, .05), value=.9, seed=9),
    }
    scans['narrow_result'][:, :] = .02
    scans['narrow_result'][40:60, 40:60] = .5  # noqa
    for label, image in scans.items():
        result = scan_border.detect(image)
        with case(f'scan_{label}') as c:
            c.json(crop=result[0], kinds=result[1], seed_label=label)
            c.array('input', image)
    # crop_for on a linear frame (what linear_frame(resize_float(source, longest), settings) returns).
    frame = engine.to_linear(np.clip(scan(80, 120, (.04, .05, .03, .06), seed=10), 0, 1)).astype(np.float32)
    for label, changes in {'plain': {}, 'turned': {'rotation': 1, 'straighten': 1.0, 'crop': [.1, .1, .9, .9]}}.items():
        s = full_settings(**changes)
        geometry_frame = engine.geometry(optics.lens_shading(frame, s), s, False)
        crop = scan_border.detect(engine.to_srgb(geometry_frame))[0]
        with case(f'scan_crop_for_{label}') as c:
            c.json(changes=changes, crop=crop)
            c.array('frame', frame)


# ---------------------------------------------------------------------------------------------------------
# EXIF orientation.

def orient_cases():
    for channels in (1, 3):
        a = synth(5, 7, channels, 14)
        for orientation in (0, 1, 2, 3, 4, 5, 6, 7, 8, 9):
            with case(f'orient_pixels_c{channels}_{orientation}') as c:
                c.json(orientation=orientation, input=Y(5, 7, channels, 14))
                c.array('output', np.ascontiguousarray(rasterio.orient_pixels(a, orientation)))


# ---------------------------------------------------------------------------------------------------------
# Lens (lensfun through a recorded Modifier).

class RecordedModifier:
    def __init__(self, modifier, log, request):
        self.modifier, self.log = modifier, log
        log.append({'kind': 'create', 'request': request})

    def apply_geometry_distortion(self, xu=0, yu=0, width=-1, height=-1):
        coords = self.modifier.apply_geometry_distortion(xu, yu, width, height)
        self.log.append({'kind': 'geometry', 'y': yu, 'rows': height if height > 0 else self.modifier.height,
                         'array': None if coords is None else np.asarray(coords, np.float32)})
        return coords

    def apply_subpixel_geometry_distortion(self, xu=0, yu=0, width=-1, height=-1):
        coords = self.modifier.apply_subpixel_geometry_distortion(xu, yu, width, height)
        self.log.append({'kind': 'subpixel', 'y': yu, 'rows': height if height > 0 else self.modifier.height,
                         'array': None if coords is None else np.asarray(coords, np.float32)})
        return coords

    def apply_color_modification(self, pixels):
        before = np.array(pixels, copy=True)
        done = self.modifier.apply_color_modification(pixels)
        self.log.append({'kind': 'color', 'before': before, 'array': np.array(pixels, copy=True), 'done': bool(done)})
        return done


ORIGINAL_MODIFIER = optics._modifier


def recording_modifier(log):
    def wrapped(settings, shape, color=False):
        modifier = ORIGINAL_MODIFIER(settings, shape, color)
        if modifier is None:
            log.append({'kind': 'none'})
            return None
        data = settings['lensfun']
        request = {'width': int(shape[1]), 'height': int(shape[0]), 'crop': float(data['crop']), 'focal': float(data['focal']),
                   'aperture': float(data.get('aperture', 0)), 'distance': float(data.get('distance', 1000)),
                   'scale': 0.0 if settings.get('lensfun_scale', True) else 1.0, 'color': bool(color),
                   'vignetting': bool(color), 'distortion': not color and bool(settings.get('lensfun_distortion', True)),
                   'tca': not color and bool(settings.get('lensfun_tca', True))}
        request['flag_scale'] = (not color) and (request['distortion'] or request['tca']) and bool(settings.get('lensfun_scale', True))
        return RecordedModifier(modifier, log, request)
    return wrapped


def lens_profile(orientation=1, aperture=4.0, focal=24.0):
    for record in optics.lens_records():
        if record['maker'] == 'Canon' and record['model'] == 'Canon EF 24-105mm f/4L IS USM' and record['crop'] == 1.0:
            return optics.make_profile(record, 1.0, focal, aperture, 1000, orientation, 'Canon EOS 5D Mark II')
    raise SystemExit('lens profile not found')


def run_lens(name, spec, changes, function, budget=None):
    a = source(spec)
    s = full_settings(**changes)
    log = []
    optics._modifier = recording_modifier(log)
    old_budget = optics.MAP_BUDGET
    if budget is not None:
        optics.MAP_BUDGET = budget
    try:
        if function == 'geometry':
            result, error = error_of(lambda: engine.geometry(a.copy(), s))
        elif function == 'lens_geometry':
            result, error = error_of(lambda: optics.lens_geometry(a.copy(), s))
        else:
            result, error = error_of(lambda: optics.lens_shading(a.copy(), s))
    finally:
        optics._modifier = ORIGINAL_MODIFIER
        optics.MAP_BUDGET = old_budget
    with case(name) as c:
        stored = {k: v for k, v in changes.items() if k != 'lensfun'}
        c.json(input=spec, changes=stored, lensfun=changes.get('lensfun'), function=function, budget=budget, error=error,
               calls=[{k: v for k, v in call.items() if k not in ('array', 'before')} for call in log],
               unchanged=result is not None and np.array_equal(np.asarray(result), a) and result.shape == a.shape)
        for i, call in enumerate(log):
            if call.get('array') is not None:
                c.array(f'call{i}', call['array'])
            if call.get('before') is not None:
                c.array(f'call{i}_before', call['before'])
        if result is not None:
            c.array('output', np.asarray(result, np.float32))


def lens_cases():
    base = {'lensfun_enabled': True, 'lensfun': lens_profile()}
    spec3, spec1 = Y(37, 53, 3, 15), Y(37, 53, 1, 15)
    run_lens('lens_geometry_chromatic', spec3, base, 'lens_geometry')
    run_lens('lens_geometry_no_tca', spec3, dict(base, lensfun_tca=False), 'lens_geometry')
    run_lens('lens_geometry_mask', spec1, base, 'lens_geometry')
    run_lens('lens_geometry_no_scale', spec3, dict(base, lensfun_scale=False), 'lens_geometry')
    run_lens('lens_geometry_tca_only', spec3, dict(base, lensfun_distortion=False), 'lens_geometry')
    run_lens('lens_geometry_off', spec3, dict(base, lensfun_distortion=False, lensfun_tca=False), 'lens_geometry')
    run_lens('lens_geometry_orient6', Y(53, 37, 3, 16), dict(base, lensfun=lens_profile(6)), 'lens_geometry')
    run_lens('lens_geometry_orient3', spec3, dict(base, lensfun=lens_profile(3)), 'lens_geometry')
    run_lens('lens_geometry_mask_orient8', Y(53, 37, 1, 16), dict(base, lensfun=lens_profile(8)), 'lens_geometry')
    strip = Y(300, 20, 3, 17)
    run_lens('lens_strips_chromatic', strip, base, 'lens_geometry', budget=1000)
    run_lens('lens_strips_plain', strip, dict(base, lensfun_tca=False), 'lens_geometry', budget=1000)
    run_lens('lens_strips_mask', Y(300, 20, 1, 17), base, 'lens_geometry', budget=1000)
    run_lens('lens_strips_orient6', Y(20, 300, 3, 18), dict(base, lensfun=lens_profile(6)), 'lens_geometry', budget=1000)
    run_lens('lens_shading', spec3, base, 'lens_shading')
    run_lens('lens_shading_orient8', Y(53, 37, 3, 16), dict(base, lensfun=lens_profile(8)), 'lens_shading')
    run_lens('lens_shading_off', spec3, dict(base, lensfun_vignette=False), 'lens_shading')
    run_lens('lens_shading_no_aperture', spec3, dict(base, lensfun=lens_profile(aperture=0)), 'lens_shading')
    run_lens('lens_shading_disabled', spec3, dict(base, lensfun_enabled=False), 'lens_shading')
    bad = dict(lens_profile(), crop=50.0)
    run_lens('lens_bad_conditions', spec3, dict(base, lensfun=bad), 'lens_geometry')
    run_lens('lens_bad_orientation', spec3, dict(base, lensfun=dict(lens_profile(), orientation=9)), 'lens_geometry')
    run_lens('lens_no_profile', spec3, {'lensfun_enabled': True}, 'lens_geometry')
    run_lens('lens_in_geometry', spec3, dict(base, distortion=5.0, rotation=1, straighten=1.5, crop=[.05, .05, .95, .95]), 'geometry')
    run_lens('lens_in_geometry_mask', spec1, dict(base, ca_red=10.0, flip=True), 'geometry')


def main():
    folder = GOLDEN / MODULE
    if folder.exists():
        shutil.rmtree(folder)
    orientation_cases()
    straighten_cases()
    optical_cases()
    upright_cases()
    estimate_cases()
    straighten_functions()
    scan_cases()
    orient_cases()
    lens_cases()
    write_json(folder / 'index.json', sorted(NAMES))
    total = sum(p.stat().st_size for p in folder.rglob('*') if p.is_file())
    print(f'{len(NAMES)} cases, {total / 1e6:.2f} MB')


if __name__ == '__main__':
    main()
