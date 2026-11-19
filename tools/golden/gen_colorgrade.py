"""Goldens for grainy-colorgrade: the colour stage (engine._develop_color and what it calls), the OpenCV HLS and
numpy HSV conversions it uses, point colours, photo filters and the Auto Color analysis.

Every case is golden/colorgrade/<name>/ with case.json {kind, image, changes, ...} and the oracle's output arrays.
`image` names a standard input ('std:<name>', golden/_inputs) or one of this generator's own inputs
('cg:<name>', golden/colorgrade/inputs/). `changes` are applied to the oracle's default settings.
golden/colorgrade/index/case.json lists the cases per kind for the Rust tests.
"""
import copy

from common import Case, inputs, np, settings

from luma import auto_color, engine, photo_filter, point_color, processing

f32 = np.float32
STD = inputs()
INDEX = {}


def own_inputs():
    rng = np.random.default_rng(20261113)
    # Hue wheel, 24x45 (45 % 8 = 5: OpenCV's scalar tail is exercised): hue along x, saturation and value by row,
    # with grey, black and white rows.
    h, w = 24, 45
    hue = np.tile(np.arange(w, dtype=f32) / w, (h, 1))
    sat = np.repeat((np.arange(h, dtype=f32) % 6 / 5)[:, None], w, axis=1)
    val = np.repeat((.25 + .75 * (np.arange(h, dtype=f32) // 6) / 3)[:, None], w, axis=1)
    wheel = engine.hsv_to_rgb(hue, sat, val).astype(f32)
    wheel[0] = 0
    wheel[1] = 1
    wheel[2, :, :] = np.linspace(0, 1, w, dtype=f32)[:, None]
    # Random RGB for the conversions: 37x61, values -.2..1.3 and exact fractions, specials in the first rows.
    rgb = (rng.random((37, 61, 3)) * 1.5 - .2).astype(f32)
    rgb[:4] = np.round(rgb[:4] * 4) / 4
    rgb[4, :8] = [[0, 0, 0], [1, 1, 1], [.5, .5, .5], [1, 0, 0], [0, 1, 1], [1, 1, 0], [0, 0, 1], [1, 0, 1]]
    rgb[5, :4] = [[.3, .3, .3 + 1e-8], [.7, .7 - 1e-7, .7], [1, .99999, .99999], [1e-6, 0, 0]]
    # Auto Color pictures (display-encoded): a grey gradient with a little noise, the garden with a cast that
    # changes with brightness, and flat pictures.
    yy, xx = np.mgrid[:48, :64].astype(f32)
    grey = np.repeat(((xx + yy) / 110)[..., None], 3, axis=2).astype(f32)
    grey = np.clip(grey + rng.normal(0, .004, grey.shape).astype(f32), 0, 1).astype(f32)
    garden = engine.to_srgb(STD['garden'])
    lum = garden @ np.array([.2126, .7152, .0722], f32)
    cast = (np.array([-.03, 0, .05], f32) * (1 - lum)[..., None] + np.array([.04, .01, -.06], f32) * lum[..., None])
    cast_garden = np.clip(garden + cast, 0, 1).astype(f32)
    tinted = np.clip(grey + np.array([.05, -.01, -.04], f32), 0, 1).astype(f32)
    constant = np.full((40, 40, 3), .5, f32)
    tiny = np.full((10, 10, 3), .4, f32)
    clipped = np.full((30, 30, 3), .999, f32)
    return {'wheel': wheel, 'rgb': rgb, 'grey': grey, 'garden_srgb': garden.astype(f32), 'cast_garden': cast_garden,
            'tinted': tinted, 'constant': constant, 'tiny': tiny, 'clipped': clipped}


OWN = own_inputs()
with Case('colorgrade', 'inputs') as case:
    for name, value in OWN.items():
        case.array(name, value)


def image(name):
    kind, key = name.split(':')
    return (STD if kind == 'std' else OWN)[key].copy()


def add(group, name, **meta):
    INDEX.setdefault(group, []).append(name)
    return meta


def stage(name, img, **changes):
    with Case('colorgrade', name) as case:
        case.json(**add('stage', name, kind='stage', image=img, changes=changes))
        case.array('output', engine._develop_color(image(img), settings(**changes)))


def mix(name, img, **changes):
    with Case('colorgrade', name) as case:
        case.json(**add('mix', name, kind='mix', image=img, changes=changes))
        case.array('output', engine._develop_color_mix(image(img), settings(**changes)))


def tools(name, img, **changes):
    with Case('colorgrade', name) as case:
        case.json(**add('tools', name, kind='tools', image=img, changes=changes))
        case.array('output', processing.color_tools(image(img), settings(**changes)))


def mix_hsl(name, img, groups):
    with Case('colorgrade', name) as case:
        case.json(**add('mix_hsl', name, kind='mix_hsl', image=img, groups=groups))
        case.array('output', processing.mix_hsl(image(img), groups))


def filt(name, img, **changes):
    with Case('colorgrade', name) as case:
        case.json(**add('photo_filter', name, kind='photo_filter', image=img, changes=changes))
        case.array('output', photo_filter.apply(image(img), settings(**changes)))


# --- conversions --------------------------------------------------------------------------------------------
rgb = OWN['rgb']
with Case('colorgrade', 'convert_hsl') as case:
    h, s, l = processing.rgb_to_hsl(rgb)
    case.array('h', h); case.array('s', s); case.array('l', l)
    rng = np.random.default_rng(7)
    hh = (rng.random((29, 51)) * 3 - 1).astype(f32)
    ss = (rng.random((29, 51)) * 1.4 - .2).astype(f32)
    ll = (rng.random((29, 51)) * 1.4 - .2).astype(f32)
    hh[0, :12] = [0, 1, .5, 1 / 6, 1 / 3, 2 / 3, 5 / 6, -1e-9, 1 - 1e-8, 2, -.25, .9999999]
    ss[1, :6] = 0; ll[2, :6] = 0; ll[3, :6] = 1; ll[4, :6] = .5
    case.array('in_h', hh); case.array('in_s', ss); case.array('in_l', ll)
    case.array('rgb_out', processing.hsl_to_rgb(hh, ss, ll))
    # 1x1: the conversions of a reference colour or a grading tint (OpenCV's scalar code).
    singles = [[.8, .2, .1], [.5, .5, .5], [.1, .9, .95], [0, 0, 0], [1, 1, 1], [.2, .25, .9]]
    case.json(singles=[[*processing.rgb_to_hsl(np.array(c, f32).reshape(1, 1, 3))] for c in singles], single_inputs=singles)
INDEX['convert'] = ['convert_hsl']
with Case('colorgrade', 'convert_hsv') as case:
    h, s, v = engine.rgb_to_hsv(rgb)
    case.array('h', h); case.array('s', s); case.array('v', v)
    rng = np.random.default_rng(8)
    hh = rng.random((29, 51)).astype(f32); hh[0, :4] = [0, 1 / 6, .5, 5 / 6]
    ss = rng.random((29, 51)).astype(f32); vv = (rng.random((29, 51)) * 1.2).astype(f32)
    case.array('in_h', hh); case.array('in_s', ss); case.array('in_v', vv)
    case.array('rgb_out', engine.hsv_to_rgb(hh, ss, vv))
with Case('colorgrade', 'convert_srgb') as case:
    case.array('linear_noise', engine.to_linear(STD['noise']))
    case.array('linear_rgb', engine.to_linear(rgb))
    case.array('srgb_wide', engine.to_srgb(STD['wide']))
    case.array('srgb_rgb', engine.to_srgb(rgb))

# --- saturation, vibrance, mixer (engine._develop_color_mix) ------------------------------------------------
for img in ['std:colours', 'std:wide', 'std:noise']:
    for value in [-100, -50, 50, 100]:
        mix(f'mix_saturation_{value}_{img[4:]}', img, saturation=float(value))
        mix(f'mix_vibrance_{value}_{img[4:]}', img, vibrance=float(value))
mix('mix_sat_vib_patches', 'std:patches', saturation=30., vibrance=-40.)
mix('mix_default_colours', 'std:colours')
BANDS = ['red', 'orange', 'yellow', 'green', 'aqua', 'blue', 'purple', 'magenta']
for mode in ['hsv', 'hsl']:
    for band in range(8):
        for comp, value in enumerate([100., -100., 60.]):
            groups = [[0., 0., 0.] for _ in range(8)]
            groups[band][comp] = value if band % 2 == 0 else -value
            mix(f'mix_{mode}_{BANDS[band]}_{"hsl"[comp]}', 'cg:wheel', mixer_mode=mode, hsl=groups)
    rng = np.random.default_rng(11)
    groups = np.round(rng.uniform(-100, 100, (8, 3))).tolist()
    mix(f'mix_{mode}_all_colours', 'std:colours', mixer_mode=mode, hsl=groups)
    mix(f'mix_{mode}_all_garden', 'std:garden', mixer_mode=mode, hsl=groups, saturation=20.)
    mix(f'mix_{mode}_extreme_wide', 'std:wide', mixer_mode=mode,
        hsl=[[100., -100., 100.] if i % 2 else [-100., 100., -100.] for i in range(8)])
    mix(f'mix_{mode}_int_values', 'cg:wheel', mixer_mode=mode, hsl=[[30, 0, -20]] + [[0, 0, 0]] * 7)
for name, groups in [('mix_hsl_direct_wheel', [[40., -30., 20.]] * 8), ('mix_hsl_direct_rgb', [[0., 0., 0.]] * 3 + [[-80., 50., -60.]] + [[0., 0., 0.]] * 4)]:
    mix_hsl(name, 'cg:wheel' if 'wheel' in name else 'cg:rgb', groups)

# --- colour tools (processing.color_tools) -----------------------------------------------------------------
I = [[0., 0.], [.25, .25], [.5, .5], [.75, .75], [1., 1.]]
S = [[0., 0.], [.25, .18], [.5, .5], [.75, .83], [1., 1.]]
tools('tools_default_wide', 'std:wide')
tools('curve_s_red', 'std:colours', rgb_curves=[S, I, I])
tools('curve_s_all', 'std:garden', rgb_curves=[S, [[0., .1], [.6, .5], [1., .9]], [[0., 0.], [1., .8]]])
tools('curve_above_one', 'std:wide', rgb_curves=[[[0., 0.], [.5, .6], [1.5, 1.2], [3., 1.4]], I, [[.2, 0.], [2., 1.]]])
tools('curve_duplicate_x', 'std:colours', rgb_curves=[[[0., 0.], [.5, .2], [.5, .8], [1., 1.]], I, I])
tools('curve_invert', 'std:patches', rgb_curves=[[[0., 1.], [1., 0.]]] * 3)
tools('curve_near_identity', 'std:noise', rgb_curves=[[[0., 0.], [.5, .500001], [1., 1.]], I, I])
for region in range(4):
    for value in [-100., 100.]:
        p = [0., 0., 0., 0.]; p[region] = value
        tools(f'parametric_{region}_{int(value)}', 'std:colours', parametric=p)
tools('parametric_combo_garden', 'std:garden', parametric=[30., -20., 45., -60.])
tools('parametric_int_wide', 'std:wide', parametric=[0, 25, 0, -40])
for primary in range(3):
    for comp in range(2):
        for value in [-100., 100.]:
            c = [[0., 0.] for _ in range(3)]; c[primary][comp] = value
            tools(f'calibration_{"rgb"[primary]}_{"hs"[comp]}_{int(value)}', 'cg:wheel', calibration=c)
tools('calibration_combo_patches', 'std:patches', calibration=[[20., -30.], [-50., 40.], [70., 10.]])
wheels = {'shadows': [30., 80., 0.], 'midtones': [210., 50., -40.], 'highlights': [120., 100., 100.]}
for i, (key, value) in enumerate(wheels.items()):
    for balance in [-100., 0., 100.]:
        g = [[0., 0., 0.] for _ in range(3)]; g[i] = value
        tools(f'grading_{key}_balance_{int(balance)}', 'std:colours', grading=g, grading_balance=balance)
tools('grading_all_garden', 'std:garden', grading=[[250., 40., -20.], [45., 30., 10.], [400., 60., 30.]], grading_balance=25.)
tools('grading_light_only', 'std:noise', grading=[[0., 0., 50.], [0., 0., 0.], [0., 0., -50.]])
M = [[1.1, -.05, -.05], [-.1, 1.25, -.15], [.02, -.22, 1.2]]
tools('camera_matrix_colours', 'std:colours', camera_matrix=M)
tools('camera_matrix_wide', 'std:wide', camera_matrix=M)
tools('camera_matrix_nonfinite', 'std:colours', camera_matrix=[[1., 0., 0.], [0., 1e39, 0.], [0., 0., 1.]])
tools('camera_matrix_wrong_shape', 'std:colours', camera_matrix=[[1., 0.], [0., 1.]])
P1 = dict(version=2, rgb=[.8, .3, .2], hue=40., saturation=-30., lightness=20.)
P2 = dict(version=2, rgb=[.2, .5, .9], range=.05, saturation_range=.3, lightness_range=.25, hue=-60., saturation=50., lightness=-30.)
P3 = dict(version=2, rgb=[.3, .8, .3], range=.3, saturation_range=1., lightness_range=1., hue=100., saturation=100., lightness=100.)
PGREY = dict(version=2, rgb=[.5, .5, .51], hue=20., saturation=60.)
PALL = dict(version=2, rgb=[.9, .1, .1], range=.5, lightness_range=.2, lightness=-80.)
PV1 = dict(rgb=[.9, .6, .1], range=.08, hue=-30., saturation=40., lightness=10.)
PV1_NARROW = dict(rgb=[.1, .2, .7], range=.001, hue=50.)
PZERO = dict(version=2, rgb=[.4, .6, .2])
for name, points in [('points_one', [P1]), ('points_narrow', [P2]), ('points_three', [P1, P2, P3]),
                     ('points_grey_reference', [PGREY]), ('points_all_hues', [PALL]), ('points_v1', [PV1]),
                     ('points_v1_narrow', [PV1_NARROW]), ('points_mixed', [PV1, PZERO, P2]), ('points_zero', [PZERO])]:
    for img in ['cg:wheel', 'std:patches']:
        tools(f'{name}_{img[4:]}', img, point_colors=points)
tools('points_three_garden', 'std:garden', point_colors=[P1, P2, P3])

# Point colours directly: the hook's calls, the weights and the coverage.
for name, img, points in [('hook_three', 'cg:wheel', [P1, PZERO, P3]), ('hook_none', 'std:colours', [])]:
    seen = []
    out = point_color.apply(image(img), points, lambda i, a: seen.append([i, float(np.asarray(a, np.float64).sum())]))
    with Case('colorgrade', name) as case:
        case.json(**add('hook', name, kind='hook', image=img, points=points, calls=seen))
        case.array('output', out)
for name, img, point in [('weights_p1', 'std:garden', P1), ('weights_p2', 'cg:wheel', P2), ('weights_grey', 'cg:wheel', PGREY),
                         ('weights_all', 'std:patches', PALL), ('coverage_v1', 'cg:wheel', PV1)]:
    with Case('colorgrade', name) as case:
        case.json(**add('weights', name, kind='weights', image=img, point=point))
        if point.get('version', 1) >= 2:
            case.array('output', point_color.weights(image(img), point))
        else:
            h, s, l = processing.rgb_to_hsl(image(img)); rh, rs, rl = processing.rgb_to_hsl(np.asarray(point['rgb'], f32).reshape(1, 1, 3))
            w = np.clip(1 - np.abs((h - float(rh[0, 0]) + .5) % 1 - .5) / max(.005, point.get('range', .12)), 0, 1)
            case.array('output', w * np.clip(1 - np.abs(s - float(rs[0, 0])) / .6, 0, 1))
errors = []
for point in [dict(version=2, rgb=[1e39, 0., 0.], hue=10.), dict(version=2, rgb=[.5, .5], hue=10.),
              dict(version=2, range=0., hue=10.), dict(version=2, range=.6, hue=10.), dict(version=2, lightness_range=1.5, hue=10.)]:
    try:
        point_color.apply(OWN['wheel'].copy(), [point]); errors.append([point, None])
    except ValueError as error:
        errors.append([point, str(error)])
with Case('colorgrade', 'point_errors') as case:
    case.json(kind='errors', cases=errors)

# --- photo filter --------------------------------------------------------------------------------------------
for key, _, _ in photo_filter.FILTERS:
    for density in [25., 60., 100.]:
        for lum in [True, False]:
            filt(f'filter_{key}_{int(density)}_{"lum" if lum else "plain"}', 'cg:wheel', photo_filter_enabled=True,
                 photo_filter=key, photo_filter_density=density, photo_filter_luminosity=lum)
for key in ['warming85', 'cooling80', 'red', 'green']:
    for lum in [True, False]:
        filt(f'filter_mono_{key}_{"lum" if lum else "plain"}', 'cg:wheel', photo_filter_enabled=True, photo_filter=key,
             photo_filter_density=80., photo_filter_luminosity=lum, monochrome=True)
filt('filter_prophoto', 'cg:wheel', photo_filter_enabled=True, photo_filter='orange', photo_filter_density=70., working_space='ProPhoto')
filt('filter_prophoto_mono', 'cg:wheel', photo_filter_enabled=True, photo_filter='blue', photo_filter_density=70., working_space='ProPhoto', monochrome=True)
filt('filter_density_above_100', 'std:garden', photo_filter_enabled=True, photo_filter='cyan', photo_filter_density=180.)
filt('filter_density_string', 'cg:wheel', photo_filter_enabled=True, photo_filter='yellow', photo_filter_density='40')
filt('filter_density_zero', 'cg:wheel', photo_filter_enabled=True, photo_filter='yellow', photo_filter_density=0.)
filt('filter_density_bad', 'cg:wheel', photo_filter_enabled=True, photo_filter='yellow', photo_filter_density='lots')
filt('filter_unknown', 'cg:wheel', photo_filter_enabled=True, photo_filter='sepia')
filt('filter_disabled', 'cg:wheel', photo_filter='red', photo_filter_density=100.)
filt('filter_wide', 'std:wide', photo_filter_enabled=True, photo_filter='warming81', photo_filter_density=50.)

# --- the whole stage (engine._develop_color) -----------------------------------------------------------------
for img in ['std:colours', 'std:patches', 'std:garden', 'std:wide', 'std:noise', 'std:ramp', 'cg:wheel']:
    stage(f'stage_default_{img[4:]}', img)
rng = np.random.default_rng(12)
mixer = np.round(rng.uniform(-60, 60, (8, 3))).tolist()
stage('stage_combo_garden', 'std:garden', saturation=15., vibrance=25., mixer_mode='hsl', hsl=mixer,
      rgb_curves=[S, I, [[0., .05], [1., .95]]], parametric=[10., -5., 0., 20.], calibration=[[10., 20.], [0., 0.], [-15., 10.]],
      point_colors=[P1, P2], grading=[[220., 30., 0.], [0., 0., 0.], [40., 25., 5.]], grading_balance=-20.,
      photo_filter_enabled=True, photo_filter='warming81', photo_filter_density=30.)
stage('stage_combo_patches', 'std:patches', saturation=-30., mixer_mode='hsv', hsl=mixer, rgb_curves=[I, S, I],
      grading=[[10., 50., 0.], [100., 20., 10.], [300., 40., -10.]], camera_matrix=M)
stage('stage_combo_noise', 'std:noise', vibrance=-60., calibration=[[-40., 30.], [25., -20.], [60., 50.]],
      point_colors=[PV1, P3], camera_matrix=M, parametric=[0., 30., -30., 0.])
stage('stage_combo_wide', 'std:wide', saturation=50., hsl=mixer, rgb_curves=[[[0., 0.], [.5, .6], [1.5, 1.2]], I, I],
      photo_filter_enabled=True, photo_filter='cooling82', photo_filter_density=55., photo_filter_luminosity=False)
stage('stage_monochrome', 'std:garden', monochrome=True)
stage('stage_monochrome_mixer', 'std:colours', monochrome=True, mixer_mode='hsl', hsl=mixer)
stage('stage_monochrome_mixer_filter', 'std:colours', monochrome=True, hsl=mixer, photo_filter_enabled=True,
      photo_filter='red', photo_filter_density=90.)
stage('stage_monochrome_filter_plain', 'cg:wheel', monochrome=True, photo_filter_enabled=True, photo_filter='green',
      photo_filter_density=90., photo_filter_luminosity=False)
with Case('colorgrade', 'stage_hdr_wide') as case:
    changes = dict(hdr=True, saturation=40., grading=[[200., 40., 0.], [0., 0., 0.], [0., 0., 0.]])
    case.json(**add('stage', 'stage_hdr_wide', kind='stage', image='std:wide', changes=changes))
    case.array('output', engine._develop_color(image('std:wide'), settings(**changes)))

# --- Auto Color ----------------------------------------------------------------------------------------------
for name, img, amount in [('auto_grey', 'cg:grey', 1.), ('auto_tinted', 'cg:tinted', 1.), ('auto_tinted_half', 'cg:tinted', .5),
                          ('auto_cast_garden', 'cg:cast_garden', 1.), ('auto_cast_garden_zero', 'cg:cast_garden', 0.),
                          ('auto_cast_garden_over', 'cg:cast_garden', 3.), ('auto_garden', 'cg:garden_srgb', 1.),
                          ('auto_constant', 'cg:constant', 1.), ('auto_tiny', 'cg:tiny', 1.), ('auto_clipped', 'cg:clipped', 1.),
                          ('auto_colours', 'std:colours', 1.), ('auto_patches', 'std:patches', 1.), ('auto_noise', 'std:noise', .8)]:
    curves, report = auto_color.channel_points(image(img), amount)
    with Case('colorgrade', name) as case:
        case.json(**add('auto', name, kind='auto', image=img, amount=amount, curves=curves, report=report))

with Case('colorgrade', 'index') as case:
    case.json(**INDEX)
print('colorgrade goldens written:', {k: len(v) for k, v in INDEX.items()})
