"""Goldens for grainy-sky: luma/sky_mask.py (Select Sky), every function, with its intermediates.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_sky.py

Scenes are drawn here from seeded generators and stored as uint8 (the input is `u8 / 255` in float32, times an
optional float32 gain), at a base size; each scene is also used at half size (2x2 integer means, rounded half up)
and at double size (integer upsampling, see `up2`) so the detection copy is reduced (long side above 640). The Rust
test rebuilds the same arrays from the stored base.

Each case stores, for the detection copy and each tested sensitivity: the view box and turns, bit checksums of the
copy, of every float32 feature (and a strided subsample of each), hashes of boolean maps, the horizon row, the
detection, the guided-filter coefficients and the final masks (bit checksum and subsample). A bit checksum is the
wrapping sum of the float64 bit patterns: it changes when any value changes by one ulp. A bool hash is
(count, sum of flat indices of true pixels).
"""
import math

import cv2

from common import Case, inputs, np

from luma import sky_mask as S

MODULE = 'sky'
U64 = np.uint64


def bits(a):
    return str(int(np.ascontiguousarray(a, np.float64).view(U64).sum(dtype=U64)))


def bool_hash(a):
    flat = np.flatnonzero(np.asarray(a).ravel())
    return [int(flat.size), int(flat.sum(dtype=np.int64))]


def stride(shape, target=600):
    return max(1, math.ceil(math.sqrt(shape[0] * shape[1] / target)))


def floats(case, name, a, target=80):
    """A float32 array: bit checksum, shape and a strided subsample."""
    a = np.ascontiguousarray(a)
    assert a.dtype == np.float32, (name, a.dtype)
    k = stride(a.shape, target)
    case.json(**{f'{name}_bits': bits(a), f'{name}_shape': list(a.shape), f'{name}_stride': k})
    case.array(f'{name}_sub', np.ascontiguousarray(a[::k, ::k]))


# ------------------------------------------------------------------------------------------------- scenes
def u8(image):
    return np.uint8(np.clip(np.round(np.asarray(image, np.float64) * 255), 0, 255))


def blue_sky(t):
    return np.stack([.30 + .45 * t, .50 + .38 * t, .88 + .08 * t], -1)


def scene(kind, h, w, seed):
    rng = np.random.default_rng(seed)
    y, x = np.mgrid[:h, :w].astype(np.float64)
    t = y / h
    ground = np.array([.22, .30, .12]) + rng.normal(0, .07, (h, w, 3))
    sky = blue_sky(t)
    if kind in ('clear', 'portrait'):
        line = (.45 + .04 * np.sin(x[0] / (w / 15)) + .02 * np.sin(x[0] / (w / 50))) * h
        return np.where(y[..., None] < line[None, :, None], sky, ground)
    if kind == 'cloudy':
        image = np.where((t < .55)[..., None], sky, ground)
        for _ in range(7):
            cy, cx = rng.uniform(.05, .4) * h, rng.uniform(0, 1) * w
            ry, rx = rng.uniform(.03, .08) * h, rng.uniform(.06, .15) * w
            d = ((y - cy) / ry) ** 2 + ((x - cx) / rx) ** 2
            cloud = np.clip(1.4 - d, 0, 1)[..., None]
            image = np.where((t < .55)[..., None], image * (1 - cloud) + np.array([.95, .95, .97]) * cloud, image)
        return image
    if kind == 'trees':
        leaves = cv2.GaussianBlur(rng.normal(0, 1, (h, w)).astype(np.float32), (0, 0), max(1., w / 90))
        foliage = (leaves > -.1) | (t > .7) | (np.abs(x - w * .3) < w * .02)
        dark = np.array([.10, .18, .07]) + rng.normal(0, .05, (h, w, 3))
        return np.where(foliage[..., None] & (t > .08)[..., None], dark, sky)
    if kind == 'bars':
        bars = (np.mod(x, w / 8) < w / 40) | (t > .75) | (np.abs(y - h * .4) < h * .02)
        return np.where(bars[..., None], np.array([.08, .07, .06]) + rng.normal(0, .02, (h, w, 3)), sky)
    if kind == 'sunset':
        warm = np.stack([.98 - .1 * t, .80 - .2 * t, .58 - .15 * t], -1)
        hills = (.6 + .05 * np.sin(x[0] / (w / 9))) * h
        silhouette = np.array([.05, .03, .04]) + rng.normal(0, .02, (h, w, 3))
        return np.where(y[..., None] < hills[None, :, None], warm, silhouette)
    if kind == 'bluewall':
        wall = np.array([.32, .52, .86]) + rng.normal(0, .004, (h, w, 3))
        windows = (np.mod(x, w / 5) > w / 10) & (np.mod(y, h / 4) > h / 8) & (y < h * .9)
        image = np.where(windows[..., None], np.array([.15, .17, .2]), wall)
        return np.where((t > .9)[..., None], ground, image)
    if kind == 'water':
        mirror = blue_sky(1 - t) * .8
        return np.where((t < .35)[..., None], sky, np.where((t < .5)[..., None], ground * .6, mirror))
    if kind == 'sea':
        level = (.42 + .03 * x / w) * h
        sea = blue_sky(np.full_like(t, .42 * .6)) * .75 + rng.normal(0, .004, (h, w, 3))
        sea = np.where((t > .85)[..., None], ground, sea)
        return np.where((y < level)[..., None], blue_sky(t * .6), sea)
    if kind == 'sea2':
        # Water that looks like sky only at high sensitivity: its horizon is found by the second pass.
        sea = np.array([.16, .28, .52]) + rng.normal(0, .003, (h, w, 3))
        return np.where((t < .45)[..., None], blue_sky(t * .6), np.where((t > .85)[..., None], ground, sea))
    if kind == 'palewall':
        # A pale facade with dark windows from the top: pale and smooth, but with many enclosed gaps.
        wall = np.array([.85, .84, .82]) + rng.normal(0, .003, (h, w, 3))
        windows = (np.mod(x, w / 6) > w / 14) & (np.mod(y, h / 6) > h / 14) & (y > h * .05) & (y < h * .6)
        image = np.where(windows[..., None], np.array([.2, .2, .22]), wall)
        return np.where((t > .65)[..., None], ground, image)
    if kind == 'foliage':
        # Open sky on the left; a canopy on the right with blue gaps that do not touch the open sky.
        leaves = cv2.GaussianBlur(rng.normal(0, 1, (h, w)).astype(np.float32), (0, 0), 4.)
        gaps = (leaves < -.9 * leaves.std()) & (t < .5)
        canopy = ((x > w * .45) & (t > .06)) | (t > .75)
        dark = np.array([.10, .18, .07]) + rng.normal(0, .05, (h, w, 3))
        return np.where((canopy & ~gaps)[..., None], dark, blue_sky(t))
    if kind == 'nosky':
        image = np.empty((h, w, 3))
        image[:] = [.55, .42, .30]
        image[:h // 4] = [.30, .24, .18]
        image[h * 5 // 8:h * 5 // 6, w * 6 // 10:w * 5 // 6] = [.85, .9, .95]
        return image + rng.normal(0, .01, (h, w, 3))
    if kind == 'texture':
        return rng.uniform(.1, .9, (h, w, 3))
    if kind == 'full':
        image = blue_sky(t * .8)
        cy, cx = h * .5, w * .45
        d = ((y - cy) / (h * .12)) ** 2 + ((x - cx) / (w * .1)) ** 2
        cloud = np.clip(1.3 - d, 0, 1)[..., None]
        return image * (1 - cloud) + np.array([.93, .93, .95]) * cloud + rng.normal(0, .003, (h, w, 3))
    if kind == 'overcast':
        grey = np.stack([.82 - .1 * t, .83 - .1 * t, .86 - .1 * t], -1)
        line = (.5 + .06 * np.sin(x[0] / (w / 7))) * h
        return np.where(y[..., None] < line[None, :, None], grey, ground * .8)
    if kind == 'hazy':
        line = (.45 + .04 * np.sin(x[0] / (w / 15))) * h
        image = np.where(y[..., None] < line[None, :, None], sky, ground)
        band = (y > h * .375) & (y < h * .44)
        return np.where(band[..., None], image * .5 + np.array([.36, .38, .40]), image)
    raise ValueError(kind)


def to_float(raw, gain):
    a = raw.astype(np.float32) / np.float32(255)
    return a * np.float32(gain) if gain != 1 else a


def down2(raw):
    """Half size: 2x2 integer means rounded half up (odd edges dropped)."""
    h, w = raw.shape[0] // 2 * 2, raw.shape[1] // 2 * 2
    r = raw[:h, :w].astype(np.uint32)
    return ((r[0::2, 0::2] + r[0::2, 1::2] + r[1::2, 0::2] + r[1::2, 1::2] + 2) // 4).astype(np.uint8)


def up2(raw):
    """Double size: each pixel and the mean (rounded half up) with its right/lower/diagonal neighbour."""
    r = raw.astype(np.uint32)
    right = np.concatenate([r[:, 1:], r[:, -1:]], 1)
    down = np.concatenate([r[1:], r[-1:]], 0)
    diag = np.concatenate([right[1:], right[-1:]], 0)
    h, w = r.shape[:2]
    out = np.empty((2 * h, 2 * w) + r.shape[2:], np.uint32)
    out[0::2, 0::2] = r
    out[0::2, 1::2] = (r + right + 1) // 2
    out[1::2, 0::2] = (r + down + 1) // 2
    out[1::2, 1::2] = (r + right + down + diag + 2) // 4
    return out.astype(np.uint8)


# ------------------------------------------------------------------------------------------------- transforms
def make_transform(name):
    """Numpy closures on HxWxC arrays; the Rust test has the same ones by name."""
    if name == 'none':
        return None
    if name == 'identity':
        return lambda m: m
    if name == 'crop':
        return lambda m: m[m.shape[0] // 10:m.shape[0] * 9 // 10, m.shape[1] * 2 // 10:]
    if name == 'flip':
        return lambda m: m[:, ::-1]
    if name == 'flipv':
        return lambda m: m[::-1]
    if name.startswith('rot'):
        k = int(name[3:])
        return lambda m: np.rot90(m, k)
    if name == 'croprot1':
        return lambda m: np.rot90(m[:m.shape[0] * 3 // 4, m.shape[1] // 8:], 1)
    if name == 'pad':
        # Rows of invalid (zero) pixels on top, as a straightened frame has outside its photo.
        return lambda m: np.pad(m, ((m.shape[0] // 6, 0), (0, 0), (0, 0)))
    if name == 'tiny':
        # Almost nothing valid: the view falls back to the whole frame.
        return lambda m: np.zeros_like(m)
    raise ValueError(name)


# ------------------------------------------------------------------------------------------------- one case
COMPONENTS = [
    {},
    {'type': 'sky', 'operation': 'add', 'enabled': True, 'invert': False, 'sensitivity': 50, 'feather': 30},
    {'sensitivity': 0}, {'sensitivity': 20}, {'sensitivity': 80}, {'sensitivity': 100},
    {'feather': 0}, {'feather': 70}, {'feather': 100},
    {'sensitivity': 65.5, 'feather': 12.25}, {'sensitivity': -20, 'feather': 140},
]
SENSITIVITIES = [50, 0, 20, 80, 100, 65.5, -20]


def features_into(case, prefix, small, sensitivity):
    f = S.features(small, sensitivity)
    for key in ('score', 'blue', 'L', 'texture'):
        floats(case, f'{prefix}_{key}', f[key])
    case.json(**{f'{prefix}_edges': bool_hash(f['edges'])})
    return f


def run_case(name, source, transforms=('none',), components=COMPONENTS, sensitivities=SENSITIVITIES, base=None):
    with Case(MODULE, name) as case:
        case.json(**(base or {}))
        h, w = source.shape[:2]
        case.json(shape=[h, w], transforms=list(transforms), components=components, sensitivities=sensitivities)
        for tname in transforms:
            transform = make_transform(tname)
            p = f't_{tname}'
            box, turns = S.view(source.shape, transform)
            case.json(**{f'{p}_view': [int(v) for v in box] + [int(turns)]})
            region, small, _ = S.detection_copy(source, transform)
            floats(case, f'{p}_small', small)
            if min(small.shape[:2]) < 16:
                case.json(**{f'{p}_tooSmall': True})
            elif tname == transforms[0]:
                # Features, horizon and detection for each sensitivity (once per case: the first transform).
                f = features_into(case, f'{p}_f50', small, 50)
                candidates = ((f['score'] > .5) & ~f['edges']).astype(np.uint8)
                case.json(**{f'{p}_candidates': bool_hash(candidates)})
                line = S.horizon(f['L'], candidates)
                case.json(**{f'{p}_horizon': None if line is None else line.tolist()})
                for s in sensitivities:
                    if s != 50:
                        features_into(case, f'{p}_f{s}', small, s)
                    sky = S.detect(small, s)
                    case.json(**{f'{p}_detect{s}': bool_hash(sky)})
                    if s == 50 and sky.size <= 160000:
                        case.array(f'{p}_detect50_bits', np.packbits(sky.ravel()))
            else:
                sky = S.detect(small, 50)
                case.json(**{f'{p}_detect50': bool_hash(sky)})
            for i, component in enumerate(components):
                if i and tname != transforms[0] and i not in (2, 6):
                    continue                    # other transforms: default, sensitivity 0, feather 0
                q = f'{p}_c{i}'
                if min(small.shape[:2]) >= 16:
                    guided_into(case, q, small, component)
                floats(case, f'{q}_mask', S.sky_mask(source, component, transform), 200)


def guided_into(case, q, small, component):
    """The middle of sky_mask: guide, p, r, eps, reach and the coefficients (as sky_mask computes them)."""
    sky = S.detect(small, component.get('sensitivity', S.DEFAULTS['sensitivity']))
    case.json(**{f'{q}_any': bool(sky.any())})
    if not sky.any():
        return
    feather = float(np.clip(component.get('feather', S.DEFAULTS['feather']), 0, 100)) / 100
    sh, sw = small.shape[:2]
    size = (max(1, sw // 2), max(1, sh // 2))
    guide = cv2.resize(small, size, interpolation=cv2.INTER_AREA)
    p = cv2.resize(sky.astype(np.float32), size, interpolation=cv2.INTER_AREA)
    unit = max(size) / (S.DETECT_SIZE / 2)
    r = max(1, round((1 + 5 * feather) * unit))
    eps = np.float32(1e-4 + 3e-3 * feather)
    grow = 1 + r // 2
    reach = S._box(cv2.dilate(p, cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (2 * grow + 1, 2 * grow + 1))), max(1, r // 2))
    coefficients = S._guided_coefficients(guide, p, r, eps)
    case.json(**{f'{q}_r': r, f'{q}_eps': float(eps)})
    floats(case, f'{q}_guide', guide, 20)
    floats(case, f'{q}_p', p, 20)
    floats(case, f'{q}_reach', reach, 20)
    floats(case, f'{q}_coef', coefficients, 20)


# ------------------------------------------------------------------------------------------------- cases
SCENES = [
    # kind, base (h, w), seed, gain
    ('clear', (216, 324), 0, 1),
    ('cloudy', (216, 324), 1, 1),
    ('trees', (216, 324), 2, 1),
    ('bars', (216, 324), 3, 1),
    ('sunset', (216, 324), 4, 1),
    ('bluewall', (216, 324), 5, 1),
    ('water', (216, 324), 6, 1),
    ('sea', (216, 324), 7, 1),
    ('nosky', (216, 324), 8, 1),
    ('texture', (120, 180), 9, 1),
    ('full', (216, 324), 10, 1),
    ('overcast', (216, 324), 11, 1),
    ('hazy', (216, 324), 12, 1),
    ('portrait', (324, 216), 13, 1.6),
    ('sea2', (216, 324), 20, 1),
    ('palewall', (216, 324), 20, 1),
    ('foliage', (216, 324), 21, 1),
]
ALL_TRANSFORMS = ('none', 'identity', 'crop', 'flip', 'flipv', 'rot1', 'rot2', 'rot3', 'croprot1', 'pad', 'tiny')


def primitives(name, small, base, raw=None):
    """Bit checksums of each OpenCV/numpy step of features, horizon and the guided filter, with the oracle's
    arguments, so a mismatch is found at its source."""
    with Case(MODULE, f'cv_{name}') as case:
        h, w = small.shape[:2]
        unit = max(h, w) / S.DETECT_SIZE
        white = max(.05, float(np.percentile(small[::2, ::2].max(-1), 99.5)))
        case.json(white=white, shape=[h, w], **base)
        if raw is not None:
            case.array('raw', raw)
        n = np.minimum(small * np.float32(1 / white), 1); np.maximum(n, 0, out=n)
        floats(case, 'n', n)
        hsv = cv2.cvtColor(n, cv2.COLOR_RGB2HSV)
        floats(case, 'hsv', hsv)
        L = n @ S.LUMA
        floats(case, 'L', L)
        sigma = max(.5, .8 * unit); scale = max(unit, .25) / 8
        case.json(sigma=sigma, scale=scale)
        bl = cv2.GaussianBlur(L, (0, 0), sigma)
        floats(case, 'blurL', bl)
        bn = cv2.GaussianBlur(n, (0, 0), sigma)
        floats(case, 'blurN', bn)
        for tag, a in (('L', bl), ('N', bn)):
            gx = cv2.Sobel(a, cv2.CV_32F, 1, 0, ksize=3, scale=scale)
            gy = cv2.Sobel(a, cv2.CV_32F, 0, 1, ksize=3, scale=scale)
            floats(case, f'gx{tag}', gx)
            floats(case, f'gy{tag}', gy)
            m = cv2.magnitude(gx.reshape(h, -1), gy.reshape(h, -1)).reshape(a.shape)
            floats(case, f'mag{tag}', m)
        smooth = cv2.GaussianBlur(L, (0, 0), max(.5, unit))
        floats(case, 'smooth', smooth)
        rr = max(1, round(2 * unit))
        floats(case, 'texture', S._box(cv2.magnitude(*(cv2.Sobel(bl, cv2.CV_32F, d, 1 - d, ksize=3, scale=scale) for d in (1, 0))), rr))
        hh, ww = max(1, w // 2), max(1, h // 2)
        floats(case, 'area_half', cv2.resize(small, (hh, ww), interpolation=cv2.INTER_AREA))
        floats(case, 'area_half1', cv2.resize(np.ascontiguousarray(small[..., 1]), (hh, ww), interpolation=cv2.INTER_AREA))
        for k in (1, 2, 4, 9):
            floats(case, f'box3_{k}', S._box(small, k))
            floats(case, f'box1_{k}', S._box(np.ascontiguousarray(small[..., 0]), k))


if __name__ == '__main__':
    import sys
    sections = set(sys.argv[1:]) or {'cases', 'cv'}
    if 'cv' in sections:
        seeds = {kind: seed for kind, _, seed, _ in SCENES}
        for kind in ('clear', 'sunset', 'trees'):
            raw = u8(scene(kind, 216, 324, seeds[kind]))
            primitives(f'{kind}_base', S.detection_copy(to_float(raw, 1))[1], {'scene': kind, 'size': 'base'})
            primitives(f'{kind}_double', S.detection_copy(to_float(up2(raw), 1))[1], {'scene': kind, 'size': 'double'})
        primitives('garden', inputs()['garden'], {'scene': 'garden', 'size': 'input'})
        odd = u8(scene('cloudy', 37, 53, 3))
        primitives('odd', to_float(odd, 1), {'scene': 'odd', 'size': 'base'}, odd)
    if 'cases' not in sections:
        sys.exit()
    for kind, (h, w), seed, gain in SCENES:
        raw = u8(scene(kind, h, w, seed))
        with Case(MODULE, f'input_{kind}') as case:
            case.array('raw', raw)
            case.json(gain=gain)
        for size, data in (('base', raw), ('half', down2(raw)), ('double', up2(raw))):
            transforms = ALL_TRANSFORMS if (size == 'base' and kind in ('clear', 'cloudy', 'portrait')) else \
                ('none', 'rot1') if size == 'base' else ('none',)
            base = {'scene': kind, 'size': size, 'gain': gain}
            run_case(f'{kind}_{size}', to_float(data, gain), transforms, base=base)
    # Very small frames: below the detector's 16 pixels, and just above it.
    for h, w in ((12, 20), (15, 40), (16, 24), (20, 30), (40, 18)):
        raw = u8(scene('clear', h, w, 30 + h))
        with Case(MODULE, f'input_tiny_{h}x{w}') as case:
            case.array('raw', raw)
            case.json(gain=1)
        run_case(f'tiny_{h}x{w}', to_float(raw, 1), ('none', 'rot1'), base={'scene': f'tiny_{h}x{w}', 'size': 'base', 'gain': 1})
    # The standard garden photograph (scene-linear, values above 1 possible).
    run_case('garden', inputs()['garden'], ('none', 'identity', 'flip', 'rot3'), base={'scene': 'garden', 'size': 'input', 'gain': 1})
    # Edge-of-range values: a frame brighter than white, and a black frame (white floor .05).
    raw = u8(scene('clear', 120, 180, 40))
    with Case(MODULE, 'input_bright') as case:
        case.array('raw', raw)
        case.json(gain=3.5)
    run_case('bright_base', to_float(raw, 3.5), ('none',), base={'scene': 'bright', 'size': 'base', 'gain': 3.5})
    with Case(MODULE, 'input_black') as case:
        case.array('raw', np.zeros((64, 96, 3), np.uint8))
        case.json(gain=1)
    run_case('black_base', np.zeros((64, 96, 3), np.float32), ('none',), base={'scene': 'black', 'size': 'base', 'gain': 1})
