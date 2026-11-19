"""Goldens for grainy-engine: whole develops through luma/engine.py `develop` on the CPU path.

Each case is one call develop(source, settings, use_crop, output_space, original_size=...) with stored settings
(only the changed keys; the Rust side normalizes them as the oracle does). Stages are tested exhaustively in
their own crates; these cases check the order, the arguments passed between stages and the end of the chain
(negative, film look, HDR-off clip, output colour spaces, pixel scale, switched-off panels).

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_engine.py
"""
from common import Case, inputs, np

from common import ORACLE

from luma import dcp, dust_shapes, engine, negative, optics, rawcolor, tone_mapping

MODULE = 'engine'
IN = inputs()

I = [[0., 0.], [.25, .25], [.5, .5], [.75, .75], [1., 1.]]
S = [[0., 0.], [.25, .18], [.5, .5], [.75, .83], [1., 1.]]
P1 = dict(version=2, rgb=[.8, .3, .2], hue=40., saturation=-30., lightness=20.)
P2 = dict(version=2, rgb=[.2, .5, .9], range=.05, saturation_range=.3, lightness_range=.25, hue=-60., saturation=50., lightness=-30.)
MIXER = [[20., -30., 10.], [0., 0., 0.], [-40., 25., 0.], [15., 0., -20.], [0., 50., 0.], [-25., -10., 30.], [0., 0., 0.], [35., 20., -15.]]
GRADE = dict(grading=[[220., 30., 0.], [0., 0., 0.], [40., 25., 5.]], grading_balance=-20.)


def colour_negative(scene, gamma=(.62, .66, .70)):
    """A plausible scan of a colour negative of linear `scene` (as tools/golden/gen_film.py makes it)."""
    base = np.array([.78, .52, .30], np.float32)
    return (base * np.power(np.maximum(scene, 1e-4) / .18, -np.array(gamma, np.float32)) * .35).astype(np.float32)


SOURCES = dict(IN)
SOURCES['negative'] = np.ascontiguousarray(np.clip(colour_negative(IN['garden'][::2, ::2]), 0, 4))     # 80x120
NEG = SOURCES['negative']
BASE, PEAK = negative.analyse(NEG)
NEGATIVE = dict(negative=True, negative_base=[float(x) for x in BASE], negative_peak=[float(x) for x in PEAK])

cases = []


def profile_json(p):
    result = dict(p)
    for k in ('hs1', 'hs2', 'hs3', 'look'):
        a = p.get(k)
        if a is not None:
            result[k] = dict(dims=list(a.shape[:3]), data=a.ravel(), double=a.dtype == np.float64)
    return result


def info_json(info):
    """raw_info as tools/golden/gen_raw.py stores it (grainy_raw::rawcolor::CameraInfo reads it)."""
    result = dict(info)
    if result.get('base_profile'):
        result['base_profile'] = profile_json(result['base_profile'])
    return result


# A RAW file's camera frame: the oracle's own test DNG, decoded as load_image does for camera colour.
DNG = ORACLE / 'assets' / 'test-triple-illuminant.dng'
CAMERA, CAMERA_META = engine.load_image(DNG, raw_options=dict(raw_mode='as_shot'))
assert isinstance(CAMERA, rawcolor.CameraSource)
SOURCES['camera'] = CAMERA
DCP = dcp.load_profile(ORACLE / 'assets' / 'test-camera.dcp')
DCP3 = dcp.load_profile(ORACLE / 'assets' / 'test-triple-illuminant.dcp')


def case(name, source, changes=None, output='sRGB', use_crop=True, original_size=None):
    cases.append((name, source, dict(changes or {}), output, use_crop, original_size))


# The chain's ends.
case('defaults_garden', 'garden')
case('defaults_wide', 'wide')
case('defaults_noise', 'noise')
case('defaults_working', 'wide', output=None)
case('defaults_uncropped', 'colours', use_crop=False)
case('defaults_all', 'colours', use_crop='all')

# Tone.
BASIC = dict(exposure=.8, contrast=25., highlights=-60., shadows=45., whites=15., blacks=-20.)
case('basic_garden', 'garden', BASIC)
case('basic_wide', 'wide', BASIC)
case('basic_v1', 'colours', dict(BASIC, tone_version=1))
case('basic_v2', 'wide', dict(BASIC, tone_version=2))
case('white_balance', 'patches', dict(temperature=35., tint=-20.))
case('kelvin', 'patches', dict(kelvin_enabled=True, kelvin=3400., wb_reference=5600., tint=12.))
case('curve', 'ramp', dict(curve=S, contrast=-30.))
case('lens_vignette', 'colours', dict(lens_vignette=60., exposure=-.5))
for index, (look, values) in enumerate(tone_mapping.LOOKS.items()):
    case(f'drt_look{index}', 'wide' if index % 2 else 'garden', dict(tone_mapping='drt', **dict(zip(tone_mapping.KEYS, values))))
case('drt_basic', 'wide', dict(BASIC, tone_mapping='drt', drt_toe=40., drt_white=-30.))

# Colour.
case('saturation', 'colours', dict(saturation=40., vibrance=-35.))
case('mixer_hsv', 'patches', dict(hsl=MIXER))
case('mixer_hsl', 'patches', dict(hsl=MIXER, mixer_mode='hsl'))
case('curves', 'colours', dict(rgb_curves=[S, I, [[0., .05], [1., .95]]], parametric=[10., -5., 0., 20.]))
case('grading', 'garden', dict(GRADE, calibration=[[10., 20.], [0., 0.], [-15., 10.]]))
case('points', 'patches', dict(point_colors=[P1, P2]))
case('photo_filter', 'colours', dict(photo_filter_enabled=True, photo_filter='cooling80', photo_filter_density=40.))
case('photo_filter_off', 'colours', dict(photo_filter_enabled=False, photo_filter_density=80.))
case('monochrome', 'patches', dict(monochrome=True, hsl=MIXER, contrast=20.))

# Detail and effects.
case('sharpen', 'edges', dict(sharpen=80., sharpen_radius=1.2, sharpen_detail=30., sharpen_mask=40.))
case('clarity_texture', 'garden', dict(clarity=40., texture=30., dehaze=20.))
case('noise_v3', 'noise', dict(noise_luma=50., noise_color=40.))
case('noise_v2', 'noise', dict(noise_luma=50., noise_color=40., detail_version=2))
case('noise_v1', 'noise', dict(noise_luma=35., sharpen=40., detail_version=1))
case('defringe', 'edges', dict(defringe=60.))
case('vignette', 'colours', dict(vignette=-60., vignette_midpoint=30., vignette_roundness=-40., vignette_feather=70., vignette_highlights=50.))
case('grain', 'ramp', dict(grain=45.))
case('grain_rough', 'garden', dict(grain=60., grain_size=70., grain_roughness=85.))

# Pixel scale (a reduced preview of a larger original).
case('scale_quarter', 'garden', dict(sharpen=70., clarity=30., noise_luma=40., grain=40., grain_size=60.), original_size=(960, 640))
case('scale_portrait', 'edges', dict(sharpen=50., texture=40.), original_size=(64 * 3, 96 * 3))
case('scale_smaller_original', 'edges', dict(sharpen=50.), original_size=(48, 32))

# Film looks.
case('film_portra', 'garden', dict(film_look='kodak-portra-400'))
case('film_blend', 'garden', dict(film_look='kodak-portra-400', film_strength=60., exposure=.5, shadows=30.))
case('film_velvia', 'colours', dict(film_look='fuji-velvia-50', contrast=10.))
case('film_bw', 'patches', dict(film_look='kodak-tri-x-400', grain=30.))
case('film_wide_gamut', 'garden', dict(film_look='kodak-portra-400', working_space='ProPhoto'))
case('film_off_panel', 'garden', dict(film_look='kodak-portra-400', panels_off=['film']))
case('film_strength_zero', 'colours', dict(film_look='kodak-portra-400', film_strength=0.))

# Negatives.
case('negative', 'negative', NEGATIVE)
case('negative_tuned', 'negative', dict(NEGATIVE, negative_range=5., negative_density=1.5, exposure=.3, saturation=20.))
case('negative_stored', 'negative', dict(negative=True, negative_base=[.8, .5, .25], negative_peak=[.02, .012, .009], contrast=15.))
case('negative_film', 'negative', dict(NEGATIVE, film_look='kodak-portra-400', film_strength=80.))
case('negative_unmeasured', 'negative', dict(negative=True))

# Working and output colour spaces.
WIDE = dict(working_space='ProPhoto', saturation=30., exposure=.4)
case('prophoto_srgb', 'garden', WIDE)
case('prophoto_working', 'garden', WIDE, output=None)
case('prophoto_p3', 'colours', WIDE, output='Display P3')
case('prophoto_adobe', 'colours', WIDE, output='Adobe RGB')
case('prophoto_linear_srgb', 'colours', WIDE, output='Linear sRGB')
case('prophoto_prophoto', 'colours', WIDE, output='ProPhoto RGB')
case('prophoto_linear', 'wide', WIDE, output='Linear ProPhoto')
case('srgb_p3', 'patches', dict(saturation=50.), output='Display P3')
case('srgb_adobe', 'patches', dict(saturation=50.), output='Adobe RGB')
case('srgb_working', 'wide', dict(exposure=1.), output=None)

# Switched-off panels and stored values the normalizer repairs.
case('panels_basic_off', 'garden', dict(BASIC, saturation=40., panels_off=['basic']))
case('panels_many_off', 'garden', dict(BASIC, **GRADE, hsl=MIXER, sharpen=60., grain=40., vignette=-40., curve=S,
                                      panels_off=['curve', 'mixer', 'grading', 'detail', 'effects']))
case('panels_filter_off', 'colours', dict(photo_filter_enabled=True, photo_filter_density=60., panels_off=['photo_filter']))
case('unknown_keys', 'colours', dict(exposure=.5, not_a_setting=3, hdr=True))

# Everything at once.
ALL = dict(BASIC, **GRADE, temperature=12., tint=-6., curve=S, saturation=10., vibrance=20., hsl=MIXER, mixer_mode='hsl',
           rgb_curves=[S, I, I], parametric=[5., -10., 15., 0.], calibration=[[5., 10.], [-5., 0.], [0., -10.]],
           point_colors=[P1], photo_filter_enabled=True, texture=20., clarity=15., dehaze=10., sharpen=45.,
           noise_luma=20., noise_color=25., vignette=-25., grain=25., grain_size=40., grain_roughness=60.)
case('all_garden', 'garden', ALL)
case('all_garden_film_wide', 'garden', dict(ALL, film_look='kodak-portra-400', film_strength=70., working_space='ProPhoto'), output='Display P3')
case('all_garden_film_wide_working', 'garden', dict(ALL, film_look='kodak-portra-400', film_strength=70., working_space='ProPhoto'), output=None)
case('all_garden_wide_no_nr', 'garden', dict(ALL, noise_luma=0., noise_color=0., film_look='kodak-portra-400', film_strength=70., working_space='ProPhoto'), output='Display P3')
case('all_noise_drt', 'noise', dict(ALL, tone_mapping='drt', drt_contrast=20., drt_shoulder=-30.), original_size=(384, 256))

# RAW sources: the camera colour stage in front of the chain.
case('camera_as_shot', 'camera', dict(raw_mode='as_shot'))
case('camera_custom', 'camera', dict(raw_mode='custom', raw_kelvin=4200., raw_tint=8.))
case('camera_neutral', 'camera', dict(raw_mode='custom', raw_neutral=[.5, 1., .7]))
case('camera_legacy_setting', 'camera', dict(raw_mode='legacy'))
case('camera_working', 'camera', dict(raw_mode='as_shot', exposure=.5), output=None)
case('camera_wide_p3', 'camera', dict(raw_mode='as_shot', working_space='ProPhoto', saturation=20.), output='Display P3')
case('camera_edits', 'camera', dict(BASIC, raw_mode='as_shot', saturation=20., sharpen=40., grain=20.))
case('camera_film', 'camera', dict(raw_mode='as_shot', film_look='kodak-portra-400', film_strength=80.))
case('camera_dcp3', 'camera', dict(raw_mode='as_shot', dcp_profile=DCP3))
case('camera_dcp3_plain', 'camera', dict(raw_mode='custom', raw_kelvin=5200., dcp_profile=DCP3, dcp_huesat=False, dcp_look=False,
                                         dcp_tone=False, dcp_exposure=False))
case('camera_dcp3_wide', 'camera', dict(raw_mode='as_shot', dcp_profile=DCP3, working_space='ProPhoto', contrast=15.))
case('camera_negative', 'camera', dict(negative=True, exposure=.3))

# Geometry: crop, turns, flip, straighten, the manual transform, upright; with the edits after it.
CROP = [.1, .2, .8, .9]
case('crop', 'garden', dict(crop=CROP))
case('crop_uncropped', 'garden', dict(crop=CROP, exposure=.3), use_crop=False)
case('crop_all', 'colours', dict(crop=CROP, straighten=4.), use_crop='all')
for turns in (1, 2, 3):
    case(f'rotation_{turns}', 'colours', dict(rotation=turns, flip=turns == 2, vignette=-40.))
case('flip', 'patches', dict(flip=True, lens_vignette=40.))
case('straighten', 'garden', dict(straighten=3.5, crop=[.05, .08, .95, .92]))
case('straighten_negative', 'edges', dict(straighten=-7.25))
case('transform', 'garden', dict(perspective_v=20., perspective_h=-10., distortion=8., transform_scale=110., shift_x=3., aspect_scale=-5.))
case('transform_ca', 'edges', dict(ca_red=30., ca_blue=-20., distortion_k2=4.))
case('guides', 'colours', dict(guides=[[.05, .1], [.9, .02], [.97, .95], [.02, .9]]))
UPRIGHT = dict(matrix=[[1.02, .04, -.01], [-.03, 1.01, .005], [.02, -.015, 1.]], crop_scale=1.12, mode='full')
case('upright', 'garden', dict(upright=UPRIGHT))
case('upright_uncropped_view', 'garden', dict(upright=UPRIGHT, upright_crop=False, rotation=1))
case('geometry_edits', 'garden', dict(BASIC, crop=CROP, rotation=3, straighten=-2., sharpen=50., grain=30., vignette=-30.),
     original_size=(960, 640))
case('geometry_camera', 'camera', dict(raw_mode='as_shot', crop=[.2, .1, .9, .8], rotation=1, straighten=1.5))
case('geometry_negative', 'negative', dict(NEGATIVE, flip=True, crop=CROP))

# Local adjustments: every mask kind, moved with the geometry.
LINEAR = dict(type='linear', points=[[.2, .2], [.8, .8]])
RADIAL = dict(type='radial', points=[[.3, .3], [.7, .8]], feather=60, angle=25.)
BRUSH = dict(type='brush', points=[[.2, .5], [.5, .4], [.8, .6]], radius=.08, feather=50)
AUTO = dict(type='brush', points=[[.3, .3], [.6, .6]], radius=.2, feather=40, auto_mask=True, auto_tolerance=.25)
LUMA = dict(type='luma', range=[.3, .8], softness=.1)
COLOUR = dict(type='color', rgb=[.2, .5, .8], tolerance=.4)


def layer(components, **adjustments):
    return dict(components=components, adjustments=adjustments)


case('mask_linear', 'garden', dict(masks=[layer([LINEAR], exposure=1.)]))
case('mask_radial', 'colours', dict(masks=[layer([RADIAL], saturation=-80., contrast=30.)]))
case('mask_brush', 'garden', dict(masks=[layer([BRUSH], exposure=-.8, temperature=40.)]))
case('mask_brush_auto', 'patches', dict(masks=[layer([AUTO], exposure=.7)]))
case('mask_range', 'garden', dict(masks=[layer([LUMA], shadows=60.), layer([COLOUR], saturation=50., hue=30.)]))
case('mask_combined', 'garden', dict(masks=[layer([LINEAR, dict(RADIAL, operation='subtract'), dict(LUMA, operation='intersect')],
                                                   exposure=.6, clarity=40., texture=30.)]))
case('mask_detail', 'noise', dict(masks=[layer([RADIAL], noise_luma=60., sharpen=50.)], detail_version=2))
case('mask_disabled', 'colours', dict(masks=[dict(layer([LINEAR], exposure=2.), enabled=False), layer([RADIAL], exposure=.5)]))
case('mask_panel_off', 'colours', dict(masks=[layer([LINEAR], exposure=2.)], panels_off=['masks']))
case('mask_geometry', 'garden', dict(masks=[layer([LINEAR], exposure=1.), layer([BRUSH], saturation=60.), layer([AUTO], exposure=-.5)],
                                    crop=CROP, rotation=1, flip=True, straighten=3., perspective_v=15.))
case('mask_uncropped', 'garden', dict(masks=[layer([RADIAL], exposure=1.)], crop=CROP), use_crop=False)
case('mask_scale', 'garden', dict(masks=[layer([BRUSH], clarity=50., sharpen=40., noise_color=30.)]), original_size=(960, 640))
case('mask_wide_p3', 'garden', dict(masks=[layer([LINEAR], exposure=.8, saturation=30.)], working_space='ProPhoto'), output='Display P3')
case('mask_camera', 'camera', dict(raw_mode='as_shot', masks=[layer([RADIAL], exposure=1.), layer([AUTO], saturation=-50.)]))

# Sky masks: detected in the developed colour source, moved with the geometry.
SKY = dict(type='sky')
case('mask_sky', 'garden', dict(masks=[layer([SKY], exposure=-1., saturation=30.)]))
case('mask_sky_options', 'garden', dict(masks=[layer([dict(SKY, sensitivity=75, feather=20)], exposure=-.7),
                                               layer([dict(SKY, invert=True)], shadows=40.)]))
case('mask_sky_geometry', 'garden', dict(masks=[layer([SKY, dict(LINEAR, operation='intersect')], exposure=-1.)],
                                         crop=CROP, rotation=1, straighten=2.5, exposure=.4))
case('mask_sky_uncropped', 'garden', dict(masks=[layer([SKY], temperature=-40.)], crop=CROP, flip=True), use_crop=False)
case('mask_sky_none', 'patches', dict(masks=[layer([SKY], exposure=-1.)]))
case('mask_sky_camera', 'camera', dict(raw_mode='as_shot', masks=[layer([SKY], exposure=-1.)]))

# Retouch: on the scene-linear frame before geometry.
HEAL = dict(type='heal', points=[[.5, .5], [.55, .52]], source=[.3, .3], radius=.03, feather=60)
HEAL2 = dict(type='heal', version=2, points=[[.4, .6]], source=[.7, .3], radius=.04)
HEAL2_AUTO = dict(type='heal', version=2, points=[[.6, .4], [.62, .45]], radius=.03)
CLONE = dict(type='clone', points=[[.25, .7]], source=[.6, .2], radius=.05, feather=30)
INPAINT = dict(type='inpaint', points=[[.7, .7], [.75, .72]], radius=.02)
RED_EYE = dict(type='red_eye', points=[[.2, .3]], radius=.04)
case('retouch_heal', 'garden', dict(retouch=[HEAL]))
case('retouch_heal2', 'garden', dict(retouch=[HEAL2]))
case('retouch_heal2_auto', 'garden', dict(retouch=[HEAL2_AUTO]))
case('retouch_clone_inpaint', 'garden', dict(retouch=[CLONE, INPAINT, dict(HEAL, enabled=False)]))
case('retouch_red_eye', 'patches', dict(retouch=[RED_EYE]))
case('retouch_panel_off', 'garden', dict(retouch=[HEAL, CLONE], panels_off=['healing']))
case('retouch_geometry_masks', 'garden', dict(BASIC, retouch=[HEAL, HEAL2_AUTO, INPAINT], crop=CROP, straighten=2., rotation=3,
                                             masks=[layer([LINEAR], exposure=.5)], film_look='kodak-portra-400', film_strength=60.))

# Dust removal: spots in source coordinates, repaired before geometry.
SHAPE = IN['garden'].shape
SPOTS = [dust_shapes.manual([[.30, .40], [.34, .43], [.37, .41]], SHAPE, 6.), dust_shapes.manual([[.7, .6]], SHAPE, 9.),
         dust_shapes.manual([[.12, .8], [.2, .82]], SHAPE, 3.)]
DUST = dict(type='dust', version=4, spots=SPOTS)
case('retouch_dust_smooth', 'garden', dict(retouch=[dict(DUST, repair_method='smooth')]))
case('retouch_dust_texture', 'garden', dict(retouch=[dict(DUST, repair_method='texture')]))
case('retouch_dust_default_method', 'garden', dict(retouch=[DUST, HEAL], crop=CROP, rotation=1))
case('retouch_dust_empty', 'colours', dict(retouch=[dict(type='dust', spots=[])]))

# Lens profiles: distortion, lateral CA and vignetting of a stored profile (grainy-lens reimplements the models,
# so these are not bit-exact: see LENS_TOLERANCE in the Rust test).
def lens_profile(model, maker, crop, focal, aperture=4., orientation=1, camera=''):
    for record in optics.lens_records():
        if record['maker'] == maker and record['model'] == model and record['crop'] == crop:
            return optics.make_profile(record, crop, focal, aperture, 1000, orientation, camera)
    raise LookupError(model)


CANON = lens_profile('Canon EF 24-105mm f/4L IS USM', 'Canon', 1.0, 35., camera='Canon EOS 5D Mark II')
LENS = dict(lensfun_enabled=True, lensfun=CANON)
case('lens_all', 'garden', LENS)
case('lens_distortion_only', 'garden', dict(LENS, lensfun_tca=False, lensfun_vignette=False))
case('lens_no_scale', 'colours', dict(LENS, lensfun_scale=False))
case('lens_vignette_only', 'colours', dict(LENS, lensfun_distortion=False, lensfun_tca=False))
case('lens_portrait', 'edges', dict(lensfun_enabled=True, lensfun=lens_profile('Canon EF 24-105mm f/4L IS USM', 'Canon', 1.0, 70., 5.6, 6,
                                                                                 'Canon EOS 5D Mark II')))
case('lens_edits_masks', 'garden', dict(BASIC, **LENS, crop=CROP, straighten=1.5, sharpen=40.,
                                        masks=[layer([RADIAL], exposure=.5), layer([SKY], exposure=-.5)]))
case('lens_panel_off', 'garden', dict(LENS, panels_off=['lens']))
case('lens_disabled', 'garden', dict(lensfun_enabled=False, lensfun=CANON))

# A stored lens profile that cannot be used is an error, never a silent skip (the Rust test checks it fails).
PENDING = [('broken_lens', dict(lensfun_enabled=True, lensfun=dict(crop=1., focal=50., xml='<lens/>')))]

names = set()
for name, source, changes, output, use_crop, original_size in cases:
    assert name not in names, name
    names.add(name)
    a = SOURCES[source]
    result = engine.develop(a if source == 'camera' else a.copy(), dict(changes), use_crop, output, original_size=original_size)
    assert result.dtype == np.float32, (name, result.dtype)
    with Case(MODULE, name) as c:
        c.json(source=source, settings=changes, output=output, use_crop=use_crop,
               original_size=list(original_size) if original_size else None)
        if source == 'camera':
            c.array('camera', np.asarray(a))
            c.json(info=info_json(a.raw_info))
        elif source not in IN:
            c.array('input', a)
        if result.shape[0] > 96:
            # Larger frames: every third pixel and the full-frame mean per channel keep the goldens small.
            c.json(mean=result.mean(axis=(0, 1), dtype=np.float64).tolist(), shape=list(result.shape), step=3)
            c.array('output_sub', result[::3, ::3])
        else:
            c.array('output', result)

with Case(MODULE, '_index') as c:
    c.json(cases=[name for name, *_ in cases], pending=[dict(name=name, settings=changes) for name, changes in PENDING])
print(len(cases), 'cases')
