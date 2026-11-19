"""Goldens for grainy-film: film looks (luma/film_look.py) and film negatives (luma/negative.py).

Also copies the oracle's film data (assets/film, its NOTICE) into crates/grainy-film/assets/film and writes
crates/grainy-film/src/embedded.rs, the list of those files compiled into the library.

Outputs of per-pixel functions are stored subsampled (patch centres, every 2nd or 4th pixel) where the
generator checks that nothing is lost or the Rust test computes the full image and subsamples it the same way.
"""
import shutil

from common import Case, ORACLE, ROOT, inputs, np, settings

import cv2
from luma import film_look as fl
from luma import negative as ng

CRATE = ROOT / 'crates' / 'grainy-film'
IN = inputs()

# ---------------------------------------------------------------- data files, embedded in the library
target = CRATE / 'assets' / 'film'
if target.exists():
    shutil.rmtree(target)
shutil.copytree(ORACLE / 'assets' / 'film', target)
shutil.copy(ORACLE / 'assets' / 'licenses' / 'Film' / 'NOTICE.txt', target / 'NOTICE.txt')
stocks = sorted(p.stem for p in (target / 'stocks').glob('*.json'))
papers = sorted(p.stem for p in (target / 'papers').glob('*.json'))
lines = ['//! The film data (copied from the oracle\'s assets/film by tools/golden/gen_film.py; do not edit).', '',
         'pub(crate) static COLORIMETRY: &str = include_str!("../assets/film/colorimetry.json");',
         'pub(crate) static INDEX: &str = include_str!("../assets/film/index.json");', '',
         'pub(crate) static STOCKS: &[(&str, &str)] = &[']
lines += [f'    ("{s}", include_str!("../assets/film/stocks/{s}.json")),' for s in stocks]
lines += ['];', '', 'pub(crate) static PAPERS: &[(&str, &str)] = &[']
lines += [f'    ("{s}", include_str!("../assets/film/papers/{s}.json")),' for s in papers]
lines += ['];', '']
(CRATE / 'src' / 'embedded.rs').write_text('\n'.join(lines), encoding='utf-8', newline='\n')

ALL = [e['id'] for e in fl.catalog()]
DOZEN = ['kodak-portra-400', 'kodak-ektar-100', 'fuji-pro-400h', 'kodak-portra-100t', 'fuji-velvia-50',
         'kodak-kodachrome-64', 'kodak-ektachrome-e100', 'ilford-hp5-plus', 'kodak-tri-x-400', 'kodak-vision3-500t',
         'kodak-double-x', 'cinestill-800t', 'cinestill-50d']
SPACES = ['sRGB', 'ProPhoto']


def patch_centres(a):
    """The 4x6 patch values of a render of `patches`, after checking every patch is one colour."""
    small = a[8::16, 8::16]
    assert np.array_equal(np.repeat(np.repeat(small, 16, 0), 16, 1), a)
    return small


# ---------------------------------------------------------------- numpy's float32 exp and log (SIMD, not correctly rounded)
with Case('film', 'npmath') as case:
    rng = np.random.default_rng(7)
    x = np.concatenate([rng.random(6000) * 10, 10 ** rng.uniform(-38, 38, 3000), rng.random(1000) * 1e-5,
                        [1e-5, 1., .5, 2., np.sqrt(.5), 3e-39, 0., np.inf, -1., np.nan]]).astype(np.float32)
    y = np.concatenate([rng.standard_normal(6000) * 4, rng.uniform(-104, 89, 3000),
                        [0., 1., -1., 88.7, 88.73, -103.9, -104., np.inf, -np.inf, np.nan]]).astype(np.float32)
    case.array('x', x)
    case.array('log', np.log(x))
    case.array('y', y)
    case.array('exp', np.exp(y))

# ---------------------------------------------------------------- spectral model
with Case('film', 'spectral') as case:
    c = fl.colorimetry()
    case.array('wavelengths', c['wavelengths'])
    for k in ('cmf', 'daylight', 'basis'):
        case.array(k, c[k])
    for kelvin in (4000, 5003, 5503, 6504, 7000, 7001, 12000):
        case.array(f'daylight_{kelvin}', fl.daylight(kelvin))
    for kelvin in (2000, 3200, 3999):
        case.array(f'blackbody_{kelvin}', fl.blackbody(kelvin))
    names = ['daylight', 'tungsten', 'D50', 'D55', 'D65', '3200K', '5000K', '2856K']
    for name in names:
        case.array(f'light_{name}', fl.light(name))
    case.array('adaptation_d50', fl.adaptation(np.array([.9642, 1., .8251])))
    case.array('adaptation_a', fl.adaptation(np.array([1.0985, 1., .3558])*.7))
    case.array('sampled_fill', fl.sampled([[600, 1.], [420, .5], [700, 2.]], 0.))
    case.array('sampled_ends', fl.sampled([[600, 1.], [420, .5], [700, 2.]], None))
    case.array('sampled_nan', fl.sampled([[500, 1.], [450, .5]], np.nan))
    case.array('prophoto', fl._prophoto())
    for space in SPACES:
        into, out = fl._matrices(space)
        case.array(f'into_{space}', into)
        case.array(f'out_{space}', out)
    case.json(lights=names)

# ---------------------------------------------------------------- stock intermediates
for name in ['kodak-portra-400', 'fuji-provia-100f', 'kodak-kodachrome-64', 'ilford-hp5-plus', 'cinestill-800t',
             'kodak-vision3-500t']:
    s = fl.stock(name)
    with Case('film', f'stock_{name}') as case:
        meta = dict(id=name, kind=s.kind, channels=s.channels, ref=s.ref, dx=s.dx)
        case.array('matrix', s.matrix)
        case.array('grid', s.grid)
        case.array('tables', s.tables)
        case.array('slopes', s.slopes)
        if hasattr(s, 'lights'):
            case.array('lights', np.asarray(s.lights, float))
        if s.channels == 1:
            meta['paper_white'] = float(s.paper_white)
        else:
            dyes = s.dyes if s.kind == 'colour-slide' else s.paper_dyes
            for k in ('spectra', 'base', 'base_bands', 'unmix', 'view', 'weights', 'adapt', 'levels', 'strengths'):
                case.array(f'dyes_{k}', getattr(dyes, k))
            for k in ('lo', 'hi', 'step', 'inverse_step'):
                case.array(k, getattr(s, k))
            case.array('cube_sub', s.cube[::4, ::4, ::4])
            probe = np.array([[.3, .5, .7], [1., 1., 1.], [2.5, .2, 1.4], [0., 0., 0.]]) + s.lo
            case.array('colour_probe', probe)
            case.array('colour_of_probe', dyes.colour(probe))
        p = IN['patches']
        case.array('densities_patches', patch_centres(s.densities(p)))
        case.array('xyz_patches', patch_centres(s.xyz(p)))
        case.json(**meta)

# ---------------------------------------------------------------- render
for name in ALL:
    with Case('film', f'render_patches_{name}') as case:
        for space in SPACES:
            case.array(space, patch_centres(fl.render(IN['patches'], name, space)))
        case.json(id=name)

for name in DOZEN:
    halation = bool(fl._read('stocks', name).get('halation'))
    with Case('film', f'render_{name}') as case:
        for space in SPACES:
            wide = fl.render(IN['wide'], name, space)
            assert (wide == wide[:1]).all()
            case.array(f'wide_{space}', wide[:1])
            case.array(f'colours_{space}', fl.render(IN['colours'], name, space)[::2, ::2])
            step = 2 if halation else 4
            case.array(f'garden_{space}', fl.render(IN['garden'], name, space)[::step, ::step])
            if halation:                                   # the garden has no light above 1: brighten it to glow
                bright = fl.render(IN['garden'] * 4, name, space)
                assert fl.halation(IN['garden'] * 4, fl._read('stocks', name)['halation'], np.eye(3, dtype=np.float32)) is not None
                case.array(f'garden4_{space}', bright[::2, ::2])
        case.json(id=name, garden_step=2 if halation else 4)

# halation itself, and the Gaussian blur it uses
with Case('film', 'halation') as case:
    spec = fl._read('stocks', 'cinestill-800t')['halation']
    out = {}
    for space in SPACES:
        into = fl._matrices(space)[0].astype(np.float32)
        glow = fl.halation(IN['garden'] * 4, spec, into)
        case.array(f'garden_{space}', glow[::2, ::2])
        wide = {'radius': .03, 'threshold': .5, 'strength': [.5, .2, .05]}
        case.array(f'edges_{space}', fl.halation(IN['edges'] * 3, wide, into))
        out[space] = {'garden_none': fl.halation(IN['garden'], spec, into) is None, 'small_none': fl.halation(IN['edges'], spec, into) is None,
                      'dark_none': fl.halation(IN['patches'], {'radius': .05}, into) is None,
                      'empty_none': fl.halation(IN['edges'], {}, into) is None}
    case.json(spec=spec, wide={'radius': .03, 'threshold': .5, 'strength': [.5, .2, .05]}, none=out)

with Case('film', 'gaussian') as case:
    plane = IN['garden'][..., 1].copy()
    small = IN['noise'][..., 0].copy()
    sigmas = [.6, 1.1534, 2.5, 7.0, 20.0]
    for i, sigma in enumerate(sigmas):
        if sigma in (1.1534, 7.0):
            case.array(f'garden_{i}', cv2.GaussianBlur(plane, (0, 0), sigma))
        case.array(f'noise_{i}', cv2.GaussianBlur(small, (0, 0), sigma))
    case.json(sigmas=sigmas)

# ---------------------------------------------------------------- catalogue
with Case('film', 'catalog') as case:
    by_maker = [[maker, [[title, [e['id'] for e in group]] for title, group in kinds]] for maker, kinds in fl.by_maker()]
    queries = ['Kodak Portra 400', 'kodak-portra-400', 'portra400', 'Portra 400', 'PORTRA-400', 'tri-x 400', 'hp5',
               'Velvia 50', 'velvia', 'portra', 'kodak', 'ilford', 'cinestill 800t', '800T', 'zzz', 'Portra 4000',
               'Fuji Provia', 'Ektachrome', 'kodachrome64', 'acros', 'Kodak Vision3 500T', 'vision3', 'double x',
               'Trix', 'trix400', 'portr 400', 'kodak gold', 'superia', '', None, 'none', 'off', 'No film', 'None',
               '  ', '---', 'delta', 'Delta 3200', 'xp2', 'Ilford HP5+', 'fuji 400', 'Fujicolor', 'ektar100']
    found = []
    for q in queries:
        stock_id, close = fl.find(q)
        found.append({'query': q, 'id': stock_id, 'close': close})
    case.json(catalog=fl.catalog(), by_maker=by_maker, find=found,
              short=[fl.short_name(e) for e in fl.catalog()],
              short_extra=[fl.short_name({'name': 'Kodak', 'maker': 'Kodak'}), fl.short_name({'name': 'Plain'}),
                           fl.short_name({'name': 'Kodakx 1', 'maker': 'Kodak'})],
              known={n: fl.known(n) for n in ['kodak-portra-400', 'nope', '', None]},
              grain={n: fl.grain_settings(n) for n in ['kodak-portra-400', 'agfa-vista-plus-200', 'ilford-hp5-plus',
                                                        'nope', '']},
              kinds=list(fl.KINDS), makers=list(fl.MAKERS))

with Case('film', 'with_film') as case:
    base = settings()
    cases = []
    portra = fl.with_film(base, 'kodak-portra-400')
    for start, name, explicit in [(base, 'kodak-portra-400', ()), (base, 'agfa-vista-plus-200', ()),
                                  (portra, 'kodak-tri-x-400', ()), (portra, '', ()),
                                  ({**portra, 'grain': 40.}, 'kodak-tri-x-400', ()),
                                  (portra, 'kodak-tri-x-400', ('grain_size',)),
                                  (portra, 'agfa-vista-plus-200', ()), ({'film_look': 'kodak-portra-400'}, '', ()),
                                  ({'film_look': 'kodak-portra-400', 'grain': 17.1, 'grain_size': 32.0,
                                    'grain_roughness': 50.0}, 'ilford-hp5-plus', ())]:
        cases.append({'start': start, 'name': name, 'explicit': list(explicit),
                      'result': fl.with_film(start, name, explicit)})
    active = []
    for change in [dict(film_look='kodak-portra-400'), dict(film_look=''), dict(film_look='nope'),
                   dict(film_look='kodak-portra-400', hdr=True), dict(film_look='kodak-portra-400', film_strength=0.),
                   dict(film_look='kodak-portra-400', film_strength=-5.), dict(film_look=None)]:
        active.append({'settings': change, 'active': fl.active(change)})
    case.json(cases=cases, active=active)

# ---------------------------------------------------------------- negatives
MASK = np.float32([.82, .52, .27])          # orange mask: clear film transmits less blue and green


def colour_negative(scene, gamma=(.62, .66, .70)):
    """A plausible scan of a colour negative of linear `scene`: per-layer density of log exposure over the mask."""
    exposure = np.log10(np.maximum(scene, 0) + .004)
    density = np.float32(gamma) * (exposure - np.log10(.004))
    return (MASK * 10 ** -density).astype(np.float32)


def bw_negative(scene):
    y = scene @ np.float32([.2126, .7152, .0722])
    density = .7 * (np.log10(np.maximum(y, 0) + .003) - np.log10(.003))
    return (np.float32([.78, .77, .75]) * 10 ** -density[..., None]).astype(np.float32)


colour = colour_negative(IN['garden'][::2, ::2])               # 80x120
mono = bw_negative(IN['colours'])
dusty = colour_negative(IN['patches']).copy()
dusty[30:33, 40:43] = 1e-7                                     # specks denser than the picture, a clipped channel
dusty[5:9, 70:80, 2] = 0.
frames = [colour_negative(IN[n] * k) for n, k in (('patches', 1.), ('colours', .5), ('edges', 2.))]

with Case('film', 'negative_inputs') as case:
    case.array('colour', colour)
    case.array('mono', mono)
    case.array('dusty', dusty)
    for i, f in enumerate(frames):
        case.array(f'frame_{i}', f)

with Case('film', 'negative_analyse') as case:
    out = {}
    for name, a in (('colour', colour), ('mono', mono), ('dusty', dusty)):
        case.array(f'inside_{name}', ng.inside(a))
        out[name] = ng.analyse(a)
    out['few'] = ng.ends(np.full((50, 3), 1e-6, np.float32))          # fewer than 100 usable pixels
    out['tiny'] = ng.ends(np.float32([[.5, .4, .3]] * 120))
    out['ramp'] = ng.ends(np.linspace(.001, .9, 3 * 1001, dtype=np.float32).reshape(-1, 3))
    out['base_from'] = ng.base_from(colour[:5, :7])
    out['base_from_dark'] = ng.base_from(np.zeros((2, 2, 3), np.float32))
    case.json(results=out)

with Case('film', 'negative_measured') as case:
    meas = []
    for i, change in enumerate([dict(), dict(crop=[.1, .2, .8, .9]), dict(rotation=90), dict(rotation=270, flip=True), dict(rotation=1), dict(rotation=3, flip=True),
                                dict(rotation=-1, crop=[.2, 0., 1., .6]),
                                dict(flip=True, crop=[0., 0., .5, .5]), dict(rotation=180, crop=[.3, .1, .9, .7])]):
        s = settings(**change)
        part = ng.measured(colour, s)
        case.array(f'measured_{i}', part[::3, ::3])      # the Rust test subsamples its full result alike
        smp = ng.sample(colour, s)
        case.array(f'sample_{i}', smp[::7])
        meas.append({'settings': change, 'ends': ng.ends(part.reshape(-1, 3)), 'shape': part.shape,
                     'sample_rows': len(smp)})
    big = np.repeat(np.repeat(colour, 3, 0), 3, 1)                    # 240x360: sample() takes every 2nd pixel
    big_sample = ng.sample(big, settings())
    case.array('sample_big', big_sample[::7])
    meas.append({'settings': {}, 'big_rows': len(big_sample)})
    case.json(cases=meas)

with Case('film', 'negative_apply') as case:
    cases = []
    base, peak = ng.analyse(colour)
    variants = {
        'measured': dict(negative=True, negative_base=base, negative_peak=peak),
        'stored': dict(negative=True, negative_base=[.8, .5, .25], negative_peak=[.02, .012, .009]),
        'range_low': dict(negative=True, negative_base=base, negative_peak=peak, negative_range=3.),
        'range_high': dict(negative=True, negative_base=base, negative_peak=peak, negative_range=10.),
        'density_low': dict(negative=True, negative_base=base, negative_peak=peak, negative_density=-3.),
        'density_high': dict(negative=True, negative_base=base, negative_peak=peak, negative_density=3.),
        'peak_above_base': dict(negative=True, negative_base=[.5, .4, .3], negative_peak=[.6, 1e-9, .2]),
        'off': dict(negative=False, negative_base=base, negative_peak=peak),
        'no_base': dict(negative=True, negative_base=None, negative_peak=peak),
    }
    for name, change in variants.items():
        s = settings(**change)
        out = ng.apply(colour, s)
        case.array(f'colour_{name}', out[::2, ::2])
        cases.append({'name': name, 'settings': s, 'same': out is colour})
    mb, mp = ng.analyse(mono)
    for name, change in {'mono': dict(negative=True, negative_base=mb, negative_peak=mp),
                         'mono_dense': dict(negative=True, negative_base=mb, negative_peak=mp, negative_density=1.5,
                                            negative_range=8.)}.items():
        s = settings(**change)
        case.array(name, ng.apply(mono, s))
        cases.append({'name': name, 'settings': s, 'same': False})
    db, dp = ng.analyse(dusty)
    s = settings(negative=True, negative_base=db, negative_peak=dp)
    case.array('dusty', ng.apply(dusty, s))
    cases.append({'name': 'dusty', 'settings': s, 'same': False})
    four = np.concatenate([mono, np.full(mono.shape[:2] + (1,), .25, np.float32)], -1)
    s = settings(negative=True, negative_base=mb, negative_peak=mp)
    case.array('four_channels', ng.apply(four, s))
    cases.append({'name': 'four_channels', 'settings': s, 'same': False})
    case.json(cases=cases)

with Case('film', 'negative_convert') as case:
    from luma.raw_defaults import BUILTIN_RAW, BUILTIN_LINEAR_RAW
    single = []
    for change in [dict(), dict(crop=[.1, .1, .9, .9]), dict(BUILTIN_RAW), dict(BUILTIN_LINEAR_RAW),
                   dict(exposure=1.5, blacks=-29.), dict(negative_range=8., exposure=.3)]:
        s = settings(**change)
        single.append({'settings': s, 'colour': ng.converted(colour, s), 'mono': ng.converted(mono, s)})
    roll_settings = [settings(), settings(**BUILTIN_RAW), settings(crop=[.05, .05, .95, .95])]
    samples = [ng.sample(f, s) for f, s in zip(frames, roll_settings)]
    roll = ng.converted_together(samples, roll_settings)
    placed = ng.placed(settings(), samples[0], [.8, .5, .27], [.01, .005, .002])
    case.json(single=single, roll_settings=roll_settings, roll=roll, placed=placed,
              defaults=ng.DEFAULTS, keys=list(ng.KEYS))

print('film goldens written:', len(ALL), 'stocks,', len(DOZEN), 'on all inputs')
