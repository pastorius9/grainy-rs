"""Goldens for grainy-tone: the tone stage of engine.develop (engine._develop_tone and what it calls),
tone_response (process 3 Highlights/Shadows and its OpenCV primitives), tone_mapping (the DRT) and hdr."""
import cv2

from common import Case, inputs, np, settings

from luma import engine, film_look, hdr, tone_mapping, tone_response

I = inputs()
f32 = np.float32
NEG = (I['noise'] - f32(.2)).astype(f32)
# Stage inputs are cropped to 32x96 so the goldens stay small: [y0, y1, x0, x1] of the standard input.
CROPS = {'garden': [64, 96, 72, 168]}


def crop_of(name):
    return CROPS.get(name, [16, 48, 0, 96])


def stage_input(name):
    y0, y1, x0, x1 = crop_of(name)
    a = NEG if name == 'neg' else I[name]
    return np.array(a[y0:y1, x0:x1], f32, copy=True)          # _develop_tone works in place


def procedural(height, width):
    """A deterministic test frame Rust rebuilds bit for bit: f32(k)*0.015 + f32(x)*0.002, k = (7x+13y+5c) % 101."""
    yy, xx, cc = np.mgrid[:height, :width, :3]
    k = ((7 * xx + 13 * yy + 5 * cc) % 101).astype(f32)
    return (k * f32(0.015) + xx.astype(f32) * f32(0.002)).astype(f32)


def normal(**changes):
    return engine.normalized(settings(**changes))


# --- engine.highlight_rolloff, rolloff_white -------------------------------------------------------------------
for name, source, white, knee in [('wide_2', 'wide', 2.0, .5), ('wide_4', 'wide', 4.0, .5), ('wide_1', 'wide', 1.0, .5),
                                  ('wide_knee', 'wide', 1.3, .3), ('noise_3', 'noise', 3.0, .5)]:
    with Case('tone', f'rolloff_{name}') as case:
        case.json(input=source, white=white, knee=knee)
        case.array('output', engine.highlight_rolloff(I[source].copy(), white, knee))

whites = []
for change in [dict(), dict(exposure=1.0), dict(exposure=3.0), dict(exposure=-1.0, temperature=50.0),
               dict(lens_vignette=40.0), dict(lens_vignette=-40.0, exposure=.5),
               dict(kelvin_enabled=True, kelvin=3000.0, wb_reference=6500.0, exposure=.3)]:
    s = settings(**change)
    whites.append({'settings': s, 'tone_white': engine.tone_white(s), 'rolloff_white': engine.rolloff_white(s)})
with Case('tone', 'rolloff_white') as case:
    case.json(cases=whites)

# --- to_srgb / to_linear (private copies) ----------------------------------------------------------------------
with Case('tone', 'srgb') as case:
    x = np.concatenate([I['wide'][::4], NEG[::4], I['ramp'][::4]], 0).astype(f32)
    case.array('input', x)
    case.array('srgb', engine.to_srgb(x))
    case.array('linear', engine.to_linear(x))

# --- np.interp as the curve uses it ----------------------------------------------------------------------------
with Case('tone', 'interp') as case:
    x = np.linspace(-.2, 1.2, 1001)
    xp = np.array([0, .1, .1, .35, .5, .9, 1.], np.float64)
    fp = np.array([.05, .1, .3, .32, .6, .95, .9], np.float64)
    case.array('x', x); case.array('xp', xp); case.array('fp', fp)
    case.array('output', np.interp(x, xp, fp))

# --- tone_response.curve ----------------------------------------------------------------------------------------
pairs = [(0., 0.), (0., 100.), (0., -100.), (100., 0.), (-100., 0.), (-100., 100.), (100., -100.), (-37., 52.),
         (64., -18.), (250., -300.), (1., -1.)]
with Case('tone', 'response_curve') as case:
    case.json(pairs=pairs)
    case.array('curves', np.stack([tone_response.curve(h, s) for h, s in pairs]))

# --- cv2.boxFilter (BORDER_REFLECT_101) -------------------------------------------------------------------------
rng = np.random.default_rng(7)
planes = {
    'garden': (np.clip(engine.to_srgb(I['garden']) @ tone_response.WEIGHTS, 0, 1).astype(f32), 5),
    'noise': ((I['noise'] @ tone_response.WEIGHTS).astype(f32), 2),
    'tiny': (rng.random((3, 2)).astype(f32), 4),
    'row': (rng.random((1, 7)).astype(f32), 2),
    'column': (rng.random((9, 1)).astype(f32), 3),
    'big_radius': (rng.random((20, 30)).astype(f32), 10),
}
for name, (plane, r) in planes.items():
    with Case('tone', f'box_{name}') as case:
        case.json(r=r)
        case.array('input', plane)
        case.array('output', tone_response._box(plane, r))

# --- cv2.resize INTER_AREA (tone_base) and INTER_LINEAR on float64 bands (tone_response._band) -------------------
for name, source, size in [('half', 'noise', (48, 32)), ('third', 'noise', (32, 16)), ('odd', 'noise', (37, 25)),
                           ('near', 'noise', (95, 63)), ('one', 'noise', (1, 1)), ('x_only', 'noise', (96, 40)),
                           ('garden', 'garden', (77, 51))]:
    with Case('tone', f'area_{name}') as case:
        case.json(input=source, width=size[0], height=size[1])
        case.array('output', cv2.resize(I[source], size, interpolation=cv2.INTER_AREA))
for name, shape, width in [('up', (5, 37), 96), ('single', (3, 1), 10), ('double', (4, 120), 240), ('down', (2, 50), 31)]:
    band = rng.random(shape) * 2 - .5
    with Case('tone', f'linear_{name}') as case:
        case.json(width=width)
        case.array('input', band)
        case.array('output', cv2.resize(band, (width, shape[0]), interpolation=cv2.INTER_LINEAR))

# --- tone_response.analyse / apply ------------------------------------------------------------------------------
def encoded(a, **change):
    return engine._tone_encoded(np.array(a, f32, copy=True), normal(**change))


GARDEN = np.ascontiguousarray(I['garden'][40:120, 60:180])          # 80x120
for name, frame in [('garden', GARDEN), ('patches', I['patches']), ('edges', I['edges'])]:
    e = encoded(frame)
    with Case('tone', f'analyse_{name}') as case:
        case.array('input', e)
        base = tone_response.analyse(e, e.shape[:2])
        case.array('a', base['a']); case.array('b', base['b'])

# apply: the whole frame, a band with an offset, and maps of another size (bilinear rows, cv2 columns).
g = encoded(GARDEN)
small_e = encoded(cv2.resize(GARDEN, (50, 33), interpolation=cv2.INTER_AREA))
rows_e = encoded(cv2.resize(GARDEN, (120, 23), interpolation=cv2.INTER_AREA))
apply_cases = [('whole', g, tone_response.analyse(g, g.shape[:2]), 0, len(g), 60., -45.),
               ('band', g, tone_response.analyse(g, g.shape[:2]), 17, 51, -100., 100.),
               ('resized', g, tone_response.analyse(small_e, g.shape[:2]), 0, len(g), 30., 80.),
               ('resized_band', g, tone_response.analyse(small_e, g.shape[:2]), 41, 80, -70., -20.),
               ('rows_only', g, tone_response.analyse(rows_e, g.shape[:2]), 3, 70, 100., 100.)]
for name, frame, base, first, stop, highlights, shadows in apply_cases:
    local = tone_response.with_curve(base, highlights, shadows)
    out = tone_response.apply(frame[first:stop], local, first)
    with Case('tone', f'apply_{name}') as case:
        case.json(first=first, stop=stop, shape=list(base['shape']), highlights=highlights, shadows=shadows,
                  dtype=str(out.dtype))
        case.array('a', base['a']); case.array('b', base['b'])
        case.array('output', out.astype(f32))
with Case('tone', 'apply_frame') as case:
    case.array('frame', g)

# --- tone_base on frames larger than MAP_SIZE (INTER_AREA reduction) --------------------------------------------
for name, (height, width), change in [('large', (60, 530), dict(exposure=.4)), ('fast', (64, 1024), dict()),
                                      ('tall', (700, 90), dict(lens_vignette=30.))]:
    frame = procedural(height, width)
    s = normal(shadows=40., **change)
    base = engine.tone_base(frame.copy(), s)
    with Case('tone', f'tone_base_{name}') as case:
        case.json(height=height, width=width, settings=s, shape=list(base['shape']))
        case.array('a', base['a']); case.array('b', base['b'])

# --- the stage: engine._develop_tone ------------------------------------------------------------------------------
_render = film_look.render
calls = []


def recording_render(a, name, working_space='sRGB'):
    out = _render(a, name, working_space)
    calls.append((np.array(a, f32, copy=True), np.array(out, f32, copy=True)))
    return out


film_look.render = recording_render

S_CURVE = [[0., 0.], [.25, .18], [.5, .5], [.75, .83], [1., 1.]]
stage = [
    # defaults
    ('default_ramp', 'ramp', dict()), ('default_wide', 'wide', dict()), ('default_colours', 'colours', dict()),
    ('default_garden', 'garden', dict()), ('default_noise', 'noise', dict()),
    # process 3 Highlights/Shadows
    ('p3_shadows', 'garden', dict(shadows=50.)), ('p3_highlights', 'garden', dict(highlights=-60.)),
    ('p3_ends', 'patches', dict(shadows=100., highlights=-100.)), ('p3_reverse', 'edges', dict(shadows=-100., highlights=100.)),
    ('p3_mixed', 'garden', dict(shadows=30., highlights=-40., exposure=.7, contrast=15.)),
    ('p3_wide', 'wide', dict(shadows=70., highlights=-80., exposure=.5)),
    ('p3_lens', 'garden', dict(shadows=60., highlights=-30., lens_vignette=40.)),
    ('p3_curve', 'garden', dict(shadows=45., curve=S_CURVE)),
    # process 2 and 1
    ('p2_sliders', 'garden', dict(shadows=50., highlights=-60., tone_version=2)),
    ('p2_ends', 'colours', dict(shadows=-100., highlights=100., tone_version=2, exposure=.8)),
    ('p1_exposure', 'ramp', dict(exposure=1., tone_version=1)), ('p1_sliders', 'wide', dict(shadows=40., highlights=-50., tone_version=1)),
    ('p1_wide_exposure', 'wide', dict(exposure=.5, tone_version=1)),
    # endpoints and contrast
    ('whites_up', 'ramp', dict(whites=100.)), ('whites_down', 'garden', dict(whites=-100.)), ('whites_clip', 'ramp', dict(whites=250.)),
    ('blacks_down', 'ramp', dict(blacks=-100.)), ('blacks_floor', 'garden', dict(blacks=-200.)),
    ('blacks_up', 'ramp', dict(blacks=100.)), ('blacks_clip', 'colours', dict(blacks=300.)),
    ('contrast_up', 'garden', dict(contrast=100.)), ('contrast_down', 'colours', dict(contrast=-100.)),
    ('mixed', 'garden', dict(whites=40., blacks=-30., contrast=25., shadows=20., highlights=-20.)),
    ('mixed_p3_blacks', 'garden', dict(blacks=35., whites=-25., shadows=-50.)),
    # exposure and the roll-off
    ('exposure_wide', 'wide', dict(exposure=2.)), ('exposure_ramp', 'ramp', dict(exposure=1.)),
    ('exposure_down', 'wide', dict(exposure=-1.5)), ('exposure_capped', 'wide', dict(exposure=3.)),
    ('exposure_noise', 'noise', dict(exposure=.6)),
    # lens vignette lift
    ('lens_up', 'ramp', dict(lens_vignette=40.)), ('lens_down', 'colours', dict(lens_vignette=-60.)),
    ('lens_full', 'garden', dict(lens_vignette=100., exposure=-.3)),
    # white balance
    ('wb_temp', 'garden', dict(temperature=40., tint=-20.)), ('wb_cool', 'wide', dict(temperature=-100., tint=60.)),
    ('wb_kelvin', 'garden', dict(kelvin_enabled=True, kelvin=3200., wb_reference=5500., exposure=.4)),
    # curves
    ('curve_s', 'garden', dict(curve=S_CURVE)),
    ('curve_identity_close', 'ramp', dict(curve=[[0., 0.], [.5, .5000001], [1., 1.]])),
    ('curve_steps', 'ramp', dict(curve=[[0., .1], [.3, .3], [.3, .6], [.8, .7], [1., .95]], contrast=20.)),
    ('curve_inner', 'colours', dict(curve=[[.1, 0.], [.6, .4], [.9, 1.]])),
    # negative input pixels
    ('negative_default', 'neg', dict()), ('negative_p3', 'neg', dict(shadows=50.)),
]
# DRT looks and slider ends
for index, (look, values) in enumerate(tone_mapping.LOOKS.items()):
    for source in ('wide', 'colours'):
        stage.append((f'drt_look{index}_{source}', source, dict(tone_mapping='drt', **dict(zip(tone_mapping.KEYS, values)))))
for key in tone_mapping.KEYS:
    for value in (-100., 100.):
        stage.append((f'drt_{key[4:]}_{"up" if value > 0 else "down"}', 'wide', dict(tone_mapping='drt', **{key: value})))
stage += [('drt_exposure', 'garden', dict(tone_mapping='drt', exposure=2., drt_shoulder=-60.)),
          ('drt_p3', 'garden', dict(tone_mapping='drt', shadows=40., highlights=-50., drt_toe=50.)),
          ('drt_negative', 'neg', dict(tone_mapping='drt', drt_toe=-80.)),
          ('drt_clip', 'wide', dict(tone_mapping='drt', drt_contrast=300., drt_toe=-250., drt_shoulder=150., drt_white=-120.))]
# film looks (rendered by grainy-film; the case stores the render's input and output)
stage += [('film_full', 'garden', dict(film_look='kodak-portra-400')),
          ('film_blend', 'garden', dict(film_look='kodak-portra-400', film_strength=60., exposure=.5)),
          ('film_p3', 'garden', dict(film_look='kodak-portra-400', film_strength=100., shadows=40.)),
          ('film_blend_p3_drt', 'garden', dict(film_look='kodak-portra-400', film_strength=35., shadows=-30.,
                                                highlights=-40., tone_mapping='drt')),
          ('film_off', 'garden', dict(film_look='kodak-portra-400', film_strength=0.))]
for name, name_in, change in stage:
    s = normal(**change)
    calls.clear()
    out = engine._develop_tone(stage_input(name_in), s)
    with Case('tone', f'stage_{name}') as case:
        case.json(input=name_in, crop=crop_of(name_in), settings=s, dtype=str(out.dtype), film_calls=len(calls))
        case.array('output', np.asarray(out, f32))
        if calls:
            for i, (into, rendered) in enumerate(calls):
                assert np.array_equal(into, calls[0][0]) and np.array_equal(rendered, calls[0][1])
            case.array('film_input', calls[0][0]); case.array('film', calls[0][1])

# HDR settings as _develop_tone receives them without normalized() (the mode is off in the app)
hdr_stage = [('hdr_curve', 'wide', dict(hdr=True, exposure=1., curve=S_CURVE)),
             ('hdr_sliders', 'garden', dict(hdr=True, shadows=40., highlights=-30., exposure=.5)),
             ('hdr_plain', 'wide', dict(hdr=True, whites=50.))]
for name, name_in, change in hdr_stage:
    s = settings(**change)
    out = engine._develop_tone(stage_input(name_in), s)
    with Case('tone', f'stage_{name}') as case:
        case.json(input=name_in, crop=crop_of(name_in), settings=s, dtype=str(out.dtype), film_calls=0)
        case.array('output', np.asarray(out, f32))

# _develop_tone_pixels on a band with the whole frame's tone_local (the frame is the 80x120 garden crop)
s = normal(shadows=55., highlights=-35., blacks=10.)
local = engine.tone_local(GARDEN, s)
out = engine._develop_tone_pixels(GARDEN[30:50].copy(), s, local, 30)
with Case('tone', 'pixels_band') as case:
    case.json(settings=s, first=30, stop=50, crop=[40, 120, 60, 180], dtype=str(out.dtype))
    case.array('output', np.asarray(out, f32))

# Large frames: row bands (process 3 rows(), plain transform()) and the whole-frame lens path; strided outputs.
for name, (height, width), change in [('large_p3', (260, 530), dict(shadows=60., highlights=-45.)),
                                      ('large_plain', (260, 530), dict(exposure=1.2, contrast=20.)),
                                      ('large_lens', (260, 530), dict(shadows=-40., lens_vignette=-30.)),
                                      ('large_film', (260, 530), dict(film_look='kodak-portra-400', film_strength=70.)),
                                      ('large_film_p3', (260, 530), dict(film_look='kodak-portra-400', shadows=50.))]:
    s = normal(**change)
    calls.clear()
    # Large film cases use a stand-in render that depends on the band it is given (its first row), so the
    # Rust test can check that the film sees the same row bands as in the oracle.
    film_look.render = lambda a, name, ws='sRGB': (calls.append((len(a), a.shape)) or
                                                   np.clip(a * f32(.8) + a[:1].mean(axis=(0, 2))[None, :, None], 0, 1).astype(f32))
    out = engine._develop_tone(procedural(height, width), s)
    with Case('tone', f'stage_{name}') as case:
        case.json(height=height, width=width, settings=s, dtype=str(out.dtype), film_calls=len(calls), step=[7, 5],
                  film_rows=[c[0] for c in calls])
        case.array('output', np.asarray(out, f32)[::7, ::5])
film_look.render = _render

# --- tone_mapping --------------------------------------------------------------------------------------------------
drt_settings = [dict(), dict(drt_contrast=100.), dict(drt_contrast=-100.), dict(drt_toe=100.), dict(drt_toe=-100.),
                dict(drt_shoulder=100.), dict(drt_shoulder=-100.), dict(drt_white=100.), dict(drt_white=-100.),
                dict(drt_contrast=40., drt_toe=30., drt_shoulder=30.), dict(drt_contrast=15., drt_toe=35., drt_shoulder=-20., drt_white=-45.),
                dict(drt_contrast=500., drt_toe='abc'), dict(drt_toe=None, drt_white='12.5')]
x = np.concatenate([np.linspace(-1, 1, 101), np.geomspace(1e-6, 300, 400)]).astype(f32)
with Case('tone', 'drt') as case:
    rows = []
    for change in drt_settings:
        s = settings(**change)
        p = tone_mapping.params(s)
        try:
            look = tone_mapping.look_of(s)
        except ValueError:
            look = None                     # look_of has no guard against text values
        rows.append({'settings': change, 'params': list(p), 'look': look})
    case.json(cases=rows)
    case.array('x', x)
    case.array('curves', np.stack([tone_mapping.curve(x, tone_mapping.params(settings(**c))) for c in drt_settings]))
    case.array('renders', np.stack([tone_mapping.render(NEG[:16] * f32(4), settings(**c)) for c in drt_settings]))
looks = []
for change in [dict(), dict(drt_contrast=-35., drt_toe=-25., drt_shoulder=-40.), dict(drt_contrast=40.0004, drt_toe=30., drt_shoulder=30.),
               dict(drt_contrast=40.0006, drt_toe=30., drt_shoulder=30.), dict(drt_contrast=15., drt_toe=35., drt_shoulder=-20., drt_white=-45.),
               dict(drt_contrast=15.), dict(drt_toe=None)]:
    s = settings(**change)
    looks.append({'settings': s, 'look': tone_mapping.look_of(s), 'active': tone_mapping.active(s)})
for change in [dict(tone_mapping='drt'), dict(tone_mapping='drt', hdr=True), dict(tone_mapping='classic')]:
    s = settings(**change)
    looks.append({'settings': s, 'look': tone_mapping.look_of(s), 'active': tone_mapping.active(s)})
with Case('tone', 'drt_looks') as case:
    case.json(cases=looks, names=list(tone_mapping.LOOKS))

# --- hdr ---------------------------------------------------------------------------------------------------------
with Case('tone', 'hdr') as case:
    a = (I['wide'][:16] * f32(1.4) - f32(.3)).astype(f32)
    case.array('input', a)
    case.json(curves=[S_CURVE, [[0., 0.], [.5, .6], [1., .9]], [[0., 0.], [1., 1.]], [[0., .2], [.99999995, .5], [1., .7]]],
              limits=[dict(), dict(hdr_brightness=60., hdr_limit=2.5), dict(hdr_brightness=-100., hdr_limit=9.)],
              sdr=[dict(), dict(sdr_exposure=-1.5, sdr_compression=0.), dict(sdr_exposure=7., sdr_compression=100.),
                   dict(sdr_compression=30.)])
    case.array('curves', np.stack([hdr.curve_extended(a, c) for c in case.meta['curves']]))
    case.array('limits', np.stack([hdr.limit(a, c) for c in case.meta['limits']]))
    case.array('sdr', np.stack([hdr.sdr_rendition(a, c) for c in case.meta['sdr']]))
    case.array('operation', hdr.color_operation(a, lambda bounded: np.clip(bounded * f32(1.2) - f32(.05), 0, 1)))

print('tone goldens written')
