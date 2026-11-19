"""Goldens for grainy-effects: film grain (the seeded random field, its blur and its character) and the post-crop
vignette, from luma/engine.py. Also numpy's random generator itself (PCG64 + ziggurat normals) and cv2's
GaussianBlur, which the crate ports privately."""
import cv2

from common import Case, inputs, np, settings

from luma import engine

U64 = np.uint64


def bits_sum(a):
    """Wrapping sum of the float64 bit patterns: one number that changes when any value changes by one ulp."""
    return int(np.ascontiguousarray(a, np.float64).view(U64).sum(dtype=U64))


# --- numpy's generator: default_rng(seed).standard_normal / random, raw values, compared with tolerance 0.
for seed, n in [(2026, 4000), (0, 1000), (1, 1), (12345, 3000), (2**40 + 7, 1000), (2**70 + 3, 500)]:
    with Case('effects', f'rng_normal_{seed}_{n}') as case:
        case.json(seed=str(seed), n=n)
        case.array('values', np.random.default_rng(seed).standard_normal(n))
for seed in [2026, 99]:
    # A million draws: covers the wedge and tail branches of the ziggurat many times over.
    big = np.random.default_rng(seed).standard_normal(1_000_000)
    with Case('effects', f'rng_normal_{seed}_million') as case:
        case.json(seed=str(seed), n=1_000_000, bits_sum=str(bits_sum(big)),
                  last_bits=[str(v) for v in big[-5:].view(U64)])
for seed, n in [(7, 1000), (2026, 100)]:
    with Case('effects', f'rng_random_{seed}_{n}') as case:
        case.json(seed=str(seed), n=n)
        case.array('values', np.random.default_rng(seed).random(n))

# --- cv2.GaussianBlur((0, 0), sigma, BORDER_REFLECT_101) on float32, and _normalized_blur.
rng = np.random.default_rng(5)
for shape in [(37, 53), (5, 7), (64, 96), (1, 9)]:
    field = rng.standard_normal(shape).astype(np.float32)
    for sigma in [0.3, 0.7, 1.125, 1.5, 4.5, 15.5]:
        name = f'blur_{shape[0]}x{shape[1]}_{sigma}'
        with Case('effects', name) as case:
            case.json(sigma=sigma)
            case.array('input', field)
            case.array('output', cv2.GaussianBlur(field, (0, 0), sigma, borderType=cv2.BORDER_REFLECT_101))
            case.array('normalized', engine._normalized_blur(field, sigma))

# --- grain_pattern / settings_grain: every branch, small odd shapes, a larger one (stored subsampled).
CHARACTERS = [(size, rough, ps) for size in (10., 100.) for rough in (20., 90.) for ps in (1., .25)] + [
    (25., 50., 1.), (27., 50., 1.), (27., 80., 1.), (27., 30., 1.), (40., 50., .25), (60., 50., 1.),
    (60., 100., .25), (100., 0., 1.), (25., 100., 1.), (25., 0., 1.), (35., 65., .5)]
LARGE = (300, 451)
SUB = (slice(None, None, 5), slice(None, None, 7))


def store(case, name, a, large):
    case.json(**{f'{name}_dtype': str(a.dtype)})
    if large:
        case.array(name, a[SUB])
        case.json(**{f'{name}_sum': float(np.sum(a, dtype=np.float64)), f'{name}_bits': str(bits_sum(a))})
    else:
        case.array(name, a)


for shape in [(37, 53), (64, 96), LARGE]:
    large = shape == LARGE
    chars = CHARACTERS if not large else [c for c in CHARACTERS if c[0] in (10., 100., 25., 27.)]
    for size, rough, ps in chars:
        with Case('effects', f'pattern_{shape[0]}x{shape[1]}_{size:g}_{rough:g}_{ps:g}') as case:
            case.json(shape=shape, size=size, roughness=rough, pixel_scale=ps, large=large)
            store(case, 'pattern', engine.grain_pattern(shape, size, rough, ps), large)
            for grain in (30., 100.):
                s = settings(grain=grain, grain_size=size, grain_roughness=rough)
                store(case, f'grain_{grain:g}', np.array(engine.settings_grain(shape, s, ps)), large)
                case.json(**{f'key_{grain:g}': list(engine.grain_key(shape, s, ps))})

# --- _add_grain on images.
images = inputs()
for name in ('ramp', 'colours', 'garden'):
    for grain, size, rough, ps in [(0., 25., 50., 1.), (30., 25., 50., 1.), (100., 25., 50., 1.),
                                   (30., 60., 80., 1.), (100., 100., 20., .25)]:
        if name == 'garden' and size != 25.:
            continue
        s = settings(grain=grain, grain_size=size, grain_roughness=rough)
        with Case('effects', f'add_grain_{name}_{grain:g}_{size:g}_{rough:g}_{ps:g}') as case:
            case.json(image=name, settings=s, pixel_scale=ps)
            case.array('output', engine._add_grain(images[name].copy(), s, pixel_scale=ps))

# --- vignette_shape and post_crop_vignette.
VIGNETTES = {
    'defaults': {},
    'dark': dict(vignette=-60.),
    'light': dict(vignette=40.),
    'dark_full': dict(vignette=-100.),
    'light_full': dict(vignette=100.),
    'mid_0': dict(vignette=-50., vignette_midpoint=0.),
    'mid_100': dict(vignette=-50., vignette_midpoint=100.),
    'round_100': dict(vignette=-70., vignette_roundness=100.),
    'round_m100': dict(vignette=-70., vignette_roundness=-100.),
    'round_m33': dict(vignette=-70., vignette_roundness=-100. / 3),
    'round_40': dict(vignette=60., vignette_roundness=40.),
    'feather_0': dict(vignette=-80., vignette_feather=0.),
    'feather_100': dict(vignette=-80., vignette_feather=100.),
    'feather_75': dict(vignette=-80., vignette_feather=75.),
    'highlights_100': dict(vignette=-90., vignette_highlights=100.),
    'mixed': dict(vignette=-65., vignette_midpoint=30., vignette_roundness=-55., vignette_feather=20.,
                  vignette_highlights=60.),
    'mixed_light': dict(vignette=75., vignette_midpoint=80., vignette_roundness=70., vignette_feather=90.,
                        vignette_highlights=35.),
}
GARDEN_VIGNETTES = ('defaults', 'mixed', 'highlights_100', 'round_m100')   # the large photo: a few cases only
for name in ('garden', 'colours'):
    for label, change in VIGNETTES.items():
        if name == 'garden' and label not in GARDEN_VIGNETTES:
            continue
        s = settings(**change)
        h, w = images[name].shape[:2]
        with Case('effects', f'vignette_{name}_{label}') as case:
            case.json(image=name, settings=s, shape=list(engine.vignette_shape(s, w, h)))
            case.array('output', engine.post_crop_vignette(images[name].copy(), s))
# A one-pixel-wide and one-row image (ogrid with a single sample).
for shape in [(1, 9), (7, 1)]:
    a = np.random.default_rng(3).random((*shape, 3)).astype(np.float32)
    s = settings(vignette=-50., vignette_roundness=-30., vignette_highlights=50.)
    with Case('effects', f'vignette_thin_{shape[0]}x{shape[1]}') as case:
        case.json(settings=s, shape=list(engine.vignette_shape(s, shape[1], shape[0])))
        case.array('input', a)
        case.array('output', engine.post_crop_vignette(a.copy(), s))
print('effects goldens written')
