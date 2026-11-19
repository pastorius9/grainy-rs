"""Array goldens of gen_library.py: folder_recovery descriptors, `compare`, the 5x5 Gaussian blur it uses and
workflow_data.automatic_white_balance. Small arrays only."""
from common import GOLDEN, Case, inputs, np, write_json
import cv2
from unittest.mock import patch
from luma import folder_recovery, workflow_data, engine


def seed_image(k):
    rng = np.random.default_rng(1000 + k)
    yy, xx = np.mgrid[:64, :64] / 63.0
    img = np.zeros((64, 64, 3))
    for c in range(3):
        fx, fy = rng.uniform(0.5, 4.0, 2)
        p = rng.uniform(0, 6.28)
        img[..., c] = 0.5 + 0.35 * np.sin(2 * np.pi * (fx * xx + fy * yy) + p)
    for _ in range(4):
        x0, y0 = rng.integers(0, 48, 2)
        w, h = rng.integers(6, 16, 2)
        img[y0:y0 + h, x0:x0 + w] = rng.uniform(0.05, 0.95, 3)
    return np.uint8(np.clip(img, 0, 1) * 255 + .5)


def seed_images():
    return np.stack([seed_image(k) for k in range(8)])


def stats(x, y):
    """The quantities `compare` thresholds, computed with the oracle's own numpy/cv2 calls."""
    error = np.abs(cv2.GaussianBlur(x, (5, 5), .8) - cv2.GaussianBlur(y, (5, 5), .8))
    return [float(x.std()), float(np.abs(np.diff(x, axis=0)).mean() + np.abs(np.diff(x, axis=1)).mean()),
            float(error.mean()), float(np.quantile(error, .99))]


def main():
    seeds = seed_images()
    with Case('library', '_seeds') as case:
        case.array('seeds', seeds)
    rng = np.random.default_rng(77)
    pairs, ratios, kinds = [], [], []

    def add(a, b, ra=1.0, rb=1.0, kind=''):
        pairs.append(np.stack([a, b]))
        ratios.append([ra, rb])
        kinds.append(kind)
    for k in range(4):
        add(seeds[k], seeds[k], kind='same')
    for k in range(4):
        add(seeds[k], seeds[(k + 1) % 8], kind='different')
    for k in range(4):
        noise = rng.integers(-3, 4, seeds[k].shape)
        add(seeds[k], np.uint8(np.clip(seeds[k].astype(int) + noise, 0, 255)), kind='jpeg-like noise')
    for k in range(3):
        noise = rng.integers(-40, 41, seeds[k].shape)
        add(seeds[k], np.uint8(np.clip(seeds[k].astype(int) + noise, 0, 255)), kind='heavy noise')
    for k in range(3):
        add(seeds[k], cv2.GaussianBlur(seeds[k], (3, 3), .6), kind='softened')
    for k in range(2):
        add(seeds[k], np.roll(seeds[k], 3, axis=1), kind='shifted')
    flat = np.full((64, 64, 3), 128, np.uint8)
    add(flat, flat, kind='flat')
    gentle = np.uint8(np.repeat((100 + np.arange(64) * 0.25)[None, :, None], 64, axis=0).repeat(3, axis=2))
    add(gentle, gentle, kind='low gradient')
    add(seeds[0], seeds[0], 1.0, 1.5, kind='ratio mismatch')
    add(seeds[0], seeds[0], 1.5, 1.5 * 1.01, kind='ratio close')
    add(seeds[0], seeds[0], 1.5, 1.5 * 1.02, kind='ratio far')
    expected, numbers = [], []
    for (a, b), (ra, rb) in zip(pairs, ratios):
        x, y = a.astype(np.float32) / 255, b.astype(np.float32) / 255
        expected.append(bool(folder_recovery.compare((x, ra), (y, rb))))
        numbers.append(stats(x, y))
    with Case('library', 'compare') as case:
        case.array('pairs', np.stack(pairs))
        case.json(ratios=ratios, expected=expected, kinds=kinds, stats=numbers)
    # descriptor of a 64x64 picture (resize to the same size is the identity)
    from PIL import Image
    img = Image.fromarray(seeds[2])
    value, ratio = folder_recovery.descriptor(img)
    with Case('library', 'descriptor') as case:
        case.array('pixels', seeds[2])
        case.array('value', value)
        case.json(ratio=ratio)
    # the 5x5 Gaussian blur against cv2
    src = rng.random((24, 20, 3)).astype(np.float32)
    with Case('library', 'blur') as case:
        case.array('input', src)
        case.array('output', cv2.GaussianBlur(src, (5, 5), .8))
        case.json(kernel=[float(v) for v in cv2.getGaussianKernel(5, .8, cv2.CV_32F).ravel()])
    # automatic white balance on the standard inputs with colour casts
    ins = inputs()
    plans = [('garden', [1.0, 1.0, 1.0]), ('garden', [1.35, 1.0, 0.7]), ('garden', [0.75, 1.0, 1.4]), ('colours', [1.0, 1.0, 1.0]),
             ('patches', [1.2, 1.0, 0.9]), ('ramp', [1.1, 1.0, 0.95]), ('noise', [1.0, 1.0, 1.0]), ('edges', [1.0, 1.0, 1.0]),
             ('wide', [1.0, 1.0, 1.0]), ('garden', [0.01, 0.01, 0.01])]
    with patch.object(engine, 'geometry', lambda a, s: a), patch.object(engine, 'resize_float', lambda a, n: a), \
            patch('luma.rawcolor.linear_frame', lambda a, s: a):
        for n, (name, gains) in enumerate(plans):
            source = (ins[name] * np.array(gains, np.float32)).astype(np.float32)
            settings = {'exposure': 0.25, 'temperature': 3.0, 'tint': -2.0, 'kelvin_enabled': True, 'contrast': 7}
            with Case('library', f'awb_{n}') as case:
                try:
                    r = workflow_data.automatic_white_balance(source, settings)
                    case.json(input=name, gains=gains, settings=settings, result={'temperature': r['temperature'], 'tint': r['tint'],
                                                                                  'kelvin_enabled': r['kelvin_enabled'], 'keys': list(r)})
                except Exception as error:
                    case.json(input=name, gains=gains, settings=settings, error={'type': type(error).__name__, 'message': str(error)})
