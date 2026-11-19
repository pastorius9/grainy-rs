"""Goldens for grainy-color: transfer curves, HSV/HSL/Lab conversions, illuminants, output_rgb (sRGB path)."""
import struct

from common import Case, inputs, np

from luma import engine, illuminants, processing

ins = inputs()
ROUND_TRIP = ('colours', 'patches', 'garden', 'hand')
rng = np.random.default_rng(7)


def keep(case, name, a, derived=False):
    """Standard inputs are loaded by name in Rust (the case stores only the hand-made array)."""
    if name == 'hand':
        case.array('input', a)


def f32(x):
    return np.ascontiguousarray(np.asarray(x, np.float32))


# ---------------------------------------------------------------- hand-made colour arrays (H x W x 3)
def hand_rgb():
    vals = [-0.5, -1e-6, -0.0, 0.0, 1e-7, 1e-4, .0031308, .0031309, .02, .04045, .04046, .2, .5, .7354, 1.0,
            1.0000001, 1.5, 2.0, 6.0, 100.0]
    rows = [[v, v, v] for v in vals]                              # greys including negatives and extremes
    rows += [[1, 0, 0], [0, 1, 0], [0, 0, 1], [1, 1, 0], [0, 1, 1], [1, 0, 1], [0, 0, 0], [1, 1, 1]]
    rows += [[.5, .5, .5 + 1e-7], [.5, .5 + 2e-7, .5], [.3, .3 + 5e-8, .3], [1, 1, 1 - 1e-7]]   # d near FLT_EPSILON
    rows += [[1.2, -.3, .4], [-.2, -.1, -.3], [3, 2, 1], [0, .5, 1], [1, .5, 0], [.9, .1, .5]]
    rows += rng.uniform(-.3, 1.5, (32, 3)).tolist()
    rows += rng.random((32, 3)).tolist()
    a = np.array(rows, np.float32)
    n = (len(a) + 7) // 8 * 8
    a = np.concatenate([a, np.zeros((n - len(a), 3), np.float32)])
    return f32(a.reshape(-1, 8, 3))


hand = hand_rgb()
named = {'ramp': ins['ramp'], 'wide': ins['wide'], 'colours': ins['colours'], 'patches': ins['patches'],
         'noise': ins['noise'], 'edges': ins['edges'], 'hand': hand}
if 'garden' in ins:
    named['garden'] = f32(ins['garden'][::2, ::2])      # every other pixel, to keep the goldens small

# ---------------------------------------------------------------- transfer curves
for name, a in named.items():
    with Case('color', f'to_linear_{name}') as case:
        out = engine.to_linear(a)
        assert out.dtype == np.float32
        keep(case, name, a); case.array('output', out)
    with Case('color', f'to_srgb_{name}') as case:
        out = engine.to_srgb(a)
        assert out.dtype == np.float32
        keep(case, name, a); case.array('output', out)
    if name not in ('wide', 'hand', 'noise'):
        continue
    with Case('color', f'clip01_{name}') as case:
        keep(case, name, a); case.array('output', engine._clip01(a.copy()))
# round trip linear -> sRGB -> linear as the oracle does it
for name in ('colours', 'patches', 'garden', 'hand'):
    if name in named:
        with Case('color', f'srgb_round_trip_{name}') as case:
            a = np.clip(named[name], 0, None)
            keep(case, name, a); case.array('output', engine.to_linear(engine.to_srgb(a)))

# scalar use (app.py calls to_srgb on np.float32 scalars)
with Case('color', 'to_srgb_scalars') as case:
    xs = [0., 1e-4, .0031308, .004, .5, 1., 2., 4., 64.]
    case.json(inputs=xs, outputs=[float(engine.to_srgb(np.float32(x))) for x in xs])

# ---------------------------------------------------------------- HSV
for name, a in named.items():
    with Case('color', f'rgb_to_hsv_{name}') as case:
        h, s, v = engine.rgb_to_hsv(a)
        assert h.dtype == s.dtype == v.dtype == np.float32
        keep(case, name, a); case.array('h', h); case.array('s', s); case.array('v', v)
    # the oracle's own round trip (values clipped as the mixer does)
    if name in ROUND_TRIP:
      with Case('color', f'hsv_round_trip_{name}') as case:
        c = np.clip(a, 0, 1)
        h, s, v = engine.rgb_to_hsv(c)
        keep(case, name, c, True); case.array('output', engine.hsv_to_rgb(h, s, v))

with Case('color', 'hsv_to_rgb_hand') as case:
    h = rng.uniform(-1.5, 2.5, (8, 16)).astype(np.float32)
    s = rng.uniform(-.2, 1.3, (8, 16)).astype(np.float32)
    v = rng.uniform(-.2, 1.6, (8, 16)).astype(np.float32)
    h[0, :8] = [0, 1 / 6, 2 / 6, .5, 4 / 6, 5 / 6, 1.0, -1 / 6]          # exact sector boundaries, wrap, negative
    s[0, :8] = 1; v[0, :8] = 1
    h[1, :4] = [0.9999999, 1e-7, 2.0, -2.0]
    s[1, 4:8] = 0
    v[2, :4] = 0
    out = engine.hsv_to_rgb(h, s, v)
    assert out.dtype == np.float32
    case.array('h', h); case.array('s', s); case.array('v', v); case.array('output', out)

# ---------------------------------------------------------------- HSL (OpenCV HLS, float32)
for name, a in named.items():
    with Case('color', f'rgb_to_hsl_{name}') as case:
        h, s, l = processing.rgb_to_hsl(a)
        assert h.dtype == s.dtype == l.dtype == np.float32
        keep(case, name, a); case.array('h', h); case.array('s', s); case.array('l', l)
    if name in ROUND_TRIP:
      with Case('color', f'hsl_round_trip_{name}') as case:
        h, s, l = processing.rgb_to_hsl(a)
        keep(case, name, a, True); case.array('output', processing.hsl_to_rgb(h, s, l))

with Case('color', 'hsl_to_rgb_hand') as case:
    h = rng.uniform(-2.5, 3.5, (8, 16)).astype(np.float32)
    s = rng.uniform(-.3, 1.4, (8, 16)).astype(np.float32)
    l = rng.uniform(-.3, 1.4, (8, 16)).astype(np.float32)
    h[0, :8] = [0, 1 / 6, 2 / 6, .5, 4 / 6, 5 / 6, 1.0, -1 / 6]
    s[0, :8] = 1; l[0, :8] = .5
    s[1, :8] = 0; l[1, :8] = [0, .1, .3, .5, .7, .9, 1, .25]             # s = 0
    l[2, :4] = 0; s[2, :4] = [0, .5, 1, 1]                               # l = 0
    l[3, :4] = 1; s[3, :4] = [0, .5, 1, 1]                               # l = 1
    l[4, :4] = .5; s[4, :4] = [1, 1, 1, 1]; h[4, :4] = [0.9999999, 1e-7, 2.0, -2.0]
    s[5, :4] = 1e-9                                                       # tiny but non-zero saturation
    out = processing.hsl_to_rgb(h, s, l)
    assert out.dtype == np.float32
    case.array('h', h); case.array('s', s); case.array('l', l); case.array('output', out)


# ---------------------------------------------------------------- Lab
def lab_grid():
    L = np.linspace(-5, 110, 24, dtype=np.float32)
    A = np.linspace(-130, 130, 12, dtype=np.float32)
    B = np.linspace(-130, 130, 8, dtype=np.float32)
    g = np.stack(np.meshgrid(L, A, B, indexing='ij'), -1).reshape(-1, 16, 3)
    return f32(g)


for name, a in named.items():
    with Case('color', f'rgb_to_lab_{name}') as case:
        out = processing.rgb_to_lab(a)
        assert out.dtype == np.float32
        assert np.array_equal(out, processing._rgb_to_lab(a))
        keep(case, name, a); case.array('output', out)
    if name in ROUND_TRIP:
      with Case('color', f'lab_round_trip_{name}') as case:
        c = np.clip(a, 0, 1)
        keep(case, name, c, True); case.array('output', processing.lab_to_rgb(processing.rgb_to_lab(c)))
grid = lab_grid()
with Case('color', 'lab_to_rgb_grid') as case:
    out = processing.lab_to_rgb(grid)
    assert out.dtype == np.float32 and np.array_equal(out, processing._lab_to_rgb(grid))
    case.array('input', grid); case.array('output', out)
with Case('color', 'lab_to_rgb_neutral') as case:        # L only, a = b = 0 (neutral edits must round-trip)
    lab = np.zeros((1, 101, 3), np.float32); lab[0, :, 0] = np.linspace(0, 100, 101)
    case.array('input', lab); case.array('output', processing.lab_to_rgb(lab))

# ---------------------------------------------------------------- output_rgb: the pure part (sRGB working, sRGB out)
for name in ('wide', 'hand', 'colours'):
    a = named[name]
    with Case('color', f'output_rgb_srgb_{name}') as case:
        out = engine.output_rgb(a.copy(), 'sRGB', 'sRGB')
        keep(case, name, a); case.array('output', out)

# ProPhoto working space (profile 'Luma Wide', sRGB curve) to sRGB: LittleCMS in the oracle, matrices in Rust
for name in ('colours', 'patches', 'noise', 'wide', 'hand'):
    a = named[name]
    with Case('color', f'output_rgb_prophoto_{name}') as case:
        out = engine.output_rgb(a.copy(), 'ProPhoto', 'sRGB')
        keep(case, name, a); case.array('output', out)


# ---------------------------------------------------------------- illuminants
def run(fn, *args, **kwargs):
    try:
        r = fn(*args, **kwargs)
    except ValueError:
        return {'error': True}
    if r is None:
        return {'none': True}
    if isinstance(r, tuple):
        return {'value': [float(x) for x in r]}
    return {'value': np.asarray(r, np.float64).tolist()}


obs = illuminants.observer()
with Case('color', 'illuminants_tables') as case:
    case.json(standard_xy={str(k): list(v) for k, v in illuminants.STANDARD_XY.items()},
              lines=illuminants._sdk_temperature_table()[0].tolist(),
              directions=illuminants._sdk_temperature_table()[1].tolist(),
              observer_rows=int(obs.shape[0]), observer_sum=obs.sum(axis=0).tolist(),
              observer_first=obs[0].tolist(), observer_last=obs[-1].tolist(), observer_mid=obs[200].tolist())

xy_cases = [[.3127, .3290], [.3457, .3585], [.4476, .4074], [.2990, .3149], [.18, .26], [.35, .35], [.6, .35],
            [.15, .06], [.64, .33], [.3, .6], [.1, .1], [.99, .001], [2.0, 0.0], [1.0, 0.0], [0.0, 0.0],
            [.25, -.1], [.5, .5], [1e-9, 1e-9], [.3324, .3474], [.3, .3], [.45, .41]]
xy_cases += rng.uniform([.2, .2], [.5, .45], (40, 2)).tolist()
with Case('color', 'illuminants_temperature_tint') as case:
    case.json(cases=[{'xy': xy, **run(illuminants.temperature_tint, xy)} for xy in xy_cases])

kelvins = [1500., 1666.7, 1900., 2000., 2500., 2856., 3200., 4000., 5000., 5003., 5500., 6500., 7500., 10000.,
           15000., 50000., 1e6, 1e6 / 599., 1e6 / 601., 1e6 / 325., 1e6 / 324.9, 1e6 / 1e-3, 0., -5000., 1.0, 300.]
tints = [0., 10., -10., 50., -50., 150., -150.]
with Case('color', 'illuminants_temperature_xy') as case:
    case.json(cases=[{'kelvin': k, 'tint': t, **run(illuminants.temperature_xy, k, t)} for k in kelvins for t in tints])

with Case('color', 'illuminants_xy_xyz') as case:
    xyz_in = [[.9505, 1., 1.089], [.9642, 1., .8249], [1, 1, 1], [0, 0, 0], [-1, 1, 1], [0, 1, 0], [3, 0, 1], [1, -2, 0]]
    xy_in = [[.3127, .329], [0, 0], [1, 1], [.5, .5], [.7, .4], [1e-9, .5], [-.5, 2], [.9999999, .9999999], [.6, .6]]
    case.json(xyz_to_xy=[{'xyz': v, **run(illuminants.xyz_to_xy, v)} for v in xyz_in],
              xy_to_xyz=[{'xy': v, **run(illuminants.xy_to_xyz, v)} for v in xy_in])


def rationals(values, e):
    return b''.join(struct.pack(e + 'II', n, d) for n, d in values)


def spectrum(e, count, start, step, samples):
    return struct.pack(e + 'HI', 1, count) + rationals([start, step], e) + rationals(samples, e)


def custom_payloads():
    out = []
    for e in '<>':
        for xy in [((3127, 10000), (3290, 10000)), ((1, 3), (1, 3)), ((6, 10), (3, 10)), ((0, 1), (1, 2)),
                   ((5, 10), (0, 1)), ((9999999, 10000000), (1, 100)), ((999, 1000), (1, 1000))]:
            out.append((e, struct.pack(e + 'H', 0) + rationals(xy, e)))
        out.append((e, struct.pack(e + 'H', 0) + rationals([(1, 3)], e)))           # too short
        out.append((e, struct.pack(e + 'H', 7) + b'\0' * 16))                        # unknown kind
        out.append((e, b'\0'))
        out.append((e, b''))
        for start, step, values in [((380, 1), (5, 1), [1.0 + .002 * i for i in range(75)]),
                                    ((400, 1), (10, 1), [max(.05, 1 - ((i - 20) / 20) ** 2) for i in range(40)]),
                                    ((300, 1), (50, 1), [1.0] * 20),
                                    ((500, 1), (1, 1), [1.0] * 30),
                                    ((360, 1), (1, 1), [float(i % 7 + 1) for i in range(471)]),
                                    ((555, 2), (35, 7), [2.0, 1.0, 0.5])]:
            s = [(int(round(v * 1000)), 1000) for v in values]
            out.append((e, spectrum(e, len(s), start, step, s)))
        one = [(1, 1)]
        out.append((e, spectrum(e, 1, (400, 1), (5, 1), one)))                       # count < 2
        out.append((e, spectrum(e, 3, (400, 1), (5, 1), one * 2)))                   # length mismatch
        out.append((e, spectrum(e, 2, (0, 1), (5, 1), one * 2)))                     # start 0
        out.append((e, spectrum(e, 2, (400, 1), (0, 1), one * 2)))                   # step 0
        out.append((e, spectrum(e, 2, (400, 1), (5, 1), [(1, 1), (1, 0)])))          # zero denominator
        out.append((e, spectrum(e, 2, (400, 1), (5, 1), [(0, 1), (0, 1)])))          # no power at all
        out.append((e, spectrum(e, 2, (900, 1), (5, 1), [(1, 1), (2, 1)])))          # outside the observer range
    return out


with Case('color', 'illuminants_custom_xy') as case:
    items = [{'endian': e, 'payload': list(p), **run(illuminants.custom_xy, p, e)} for e, p in custom_payloads()]
    items.append({'endian': '<', 'payload': None, **run(illuminants.custom_xy, None, '<')})
    case.json(cases=items)

with Case('color', 'illuminants_light_xy') as case:
    items = [{'code': c, **run(illuminants.light_xy, c)} for c in range(0, 257)]
    d65 = struct.pack('<H', 0) + rationals([(3127, 10000), (3290, 10000)], '<')
    items.append({'code': 255, 'endian': '<', 'payload': list(d65), **run(illuminants.light_xy, 255, d65, '<')})
    items.append({'code': 255, 'endian': '<', 'payload': None, **run(illuminants.light_xy, 255, None, '<')})
    case.json(cases=items)

with Case('color', 'illuminants_triple_weights') as case:
    sx = illuminants.STANDARD_XY
    sets = [[sx[17], sx[21], sx[23]], [sx[17], sx[22], sx[21]], [sx[3], sx[18], sx[19]], [sx[17], sx[17], sx[21]]]
    whites = [[.9505, 1, 1.089], [1.0985, 1, .3558], [.9642, 1, .8249], [.95, 1, 1.4], [1.2, 1, .2], [.8, 1, 1.5],
              [1, 1, 1], [.96, 1, .9], [1.05, 1, .5], [0, 0, 0]]
    cases = []
    for ws in sets:
        for w in whites:
            cases.append({'whites': [list(map(float, x)) for x in ws], 'white': w,
                          **run(lambda a, b: illuminants.triple_weights(tuple(tuple(x) for x in a), np.array(b, float)), ws, w)})
    case.json(cases=cases)
print('color goldens written')
