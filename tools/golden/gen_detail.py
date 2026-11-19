"""Goldens for grainy-detail: the detail stage of engine.develop (`_develop_detail`) on the CPU path, its tools in
luma/processing.py (detail_tools, sharpen, blur, the noise reduction family) and the OpenCV primitives they call,
each with the oracle's exact arguments.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_detail.py [section ...]
Sections: cv, noise, tools, stage (default: all).
"""
import sys

import cv2

from common import Case, inputs, np, settings

from luma import engine, processing as P

MODULE = 'detail'
SECTIONS = set(sys.argv[1:]) or {'cv', 'noise', 'tools', 'stage'}
IN = inputs()


U64 = np.uint64


def bits_sum(a):
    """Wrapping sum of the float64 bit patterns of a float32 array: changes when any value changes by one ulp."""
    return int(np.ascontiguousarray(a, np.float64).view(U64).sum(dtype=U64))


def exact(case, name, a):
    """An output the Rust port reproduces bit for bit: its full bit sum and a 1/9 subsample (keeps goldens small)."""
    case.json(**{f'{name}_bits': str(bits_sum(a))})
    case.array(f'{name}_sub', a[::3, ::3])


def srgb(name):
    """The detail stage works on display-referred (sRGB-encoded) pixels: the inputs encoded and clipped as the
    colour stage leaves them, except 'wide' (kept above 1 for the HDR branch and clipping)."""
    a = IN[name]
    return a if name == 'wide' else np.ascontiguousarray(np.clip(engine.to_srgb(a), 0, 1), np.float32)


def lab_l(name):
    return np.ascontiguousarray(P.rgb_to_lab(srgb(name))[..., 0])


# ---------------------------------------------------------------- OpenCV primitives with the oracle's arguments
if 'cv' in SECTIONS:
    garden = srgb('garden')
    images = {'garden': np.ascontiguousarray(garden[40:104, 60:156]),       # 64x96
              'odd': np.ascontiguousarray(garden[3:40, 5:58]),              # 37x53: SIMD tails on every row
              'noise': srgb('noise')}
    for name, a in images.items():
        with Case(MODULE, f'cv_image_{name}') as case:
            sigmas = [0.05, 0.2, 0.325, 0.8, 1.3, 2.0, 3.0, 3.75]
            radii = [1, 2, 5, 16, 30]
            case.json(sigmas=sigmas, radii=radii)
            case.array('input', a)
            for sigma in sigmas:
                exact(case, f'gauss_{sigma}', P.blur(a, sigma))
                n = int(np.round(max(sigma, .05) * 8 + 1)) | 1
                case.array(f'kernel_{sigma}', cv2.getGaussianKernel(n, max(sigma, .05), cv2.CV_32F).ravel())
            for r in radii:
                exact(case, f'box_{r}', P._box(a, r))
            m = a.min(axis=-1)
            case.array('min', m)
            for d in (3, 7):
                case.array(f'erode_{d}', cv2.erode(m, np.ones((d, d), np.uint8)))
    planes = {'odd': np.ascontiguousarray(images['odd'][..., 0]),
              'L': np.ascontiguousarray(lab_l('garden')[40:104, 60:156]), 'Lnoise': lab_l('noise'),
              'Lodd': np.ascontiguousarray(lab_l('garden')[3:40, 5:58])}
    for name, p in planes.items():
        with Case(MODULE, f'cv_plane_{name}') as case:
            case.array('input', p)
            for r in [1, 2, 7, 30]:
                case.array(f'box_{r}', P._box(p, r))
            case.array('box55', cv2.boxFilter(p, -1, (5, 5), borderType=cv2.BORDER_REFLECT_101))
            case.array('median5', cv2.medianBlur(p, 5))
            for level in range(5):
                case.array(f'atrous_{level}', P._atrous_smooth(p, level))
            case.array('laplacian', cv2.Laplacian(p, cv2.CV_32F))
            case.array('sobel_x', cv2.Sobel(p, cv2.CV_32F, 1, 0))
            case.array('sobel_y', cv2.Sobel(p, cv2.CV_32F, 0, 1))
            params = [(7, 2.0, 1.0), (7, 10.0, 2.5), (7, 18.0, 4.0), (3, 2.0, .25), (3, 18.0, 1.0), (3, 10.0, .625)]
            case.json(bilateral=params)
            for i, (d, sc, ss) in enumerate(params):
                # The oracle's build runs Intel IPP's bilateral filter; OpenCV's own is stored too.
                case.array(f'bilateral_{i}', cv2.bilateralFilter(p, d, sc, ss))
                cv2.ipp.setUseIPP(False)
                case.array(f'bilateral_noipp_{i}', cv2.bilateralFilter(p, d, sc, ss))
                cv2.ipp.setUseIPP(True)


# ---------------------------------------------------------------- inputs of the function and stage cases


def store(case, out, s):
    """Outputs the Rust port reproduces bit for bit are stored as a full-frame bit sum and a 1/9 subsample (small
    goldens); outputs through the oracle's IPP bilateral filter (luminance noise reduction of detail_version 1-2)
    or with grain, which are compared with a tolerance, are stored whole."""
    if (s['noise_luma'] and s.get('detail_version', 1) < 3) or s['grain']:
        case.array('output', out)
    else:
        case.json(bits_sum=str(bits_sum(out)), step=[3, 3])
        case.array('output_sub', out[::3, ::3])


GARDEN = srgb('garden')
IMGS = {'garden': np.ascontiguousarray(GARDEN[48:112, 72:168]), 'edges': srgb('edges'), 'noise': srgb('noise'),
        'colours': srgb('colours'), 'wide': srgb('wide')}
if SECTIONS & {'noise', 'tools', 'stage'}:
    with Case(MODULE, 'inputs') as case:
        for name, a in IMGS.items():
            case.array(name, a)
        case.array('garden_full', GARDEN)


def tiled_srgb(h, w):
    """The large frame: the sRGB garden tiled with mirrored copies (rows, then columns), cropped."""
    g = GARDEN
    row = np.concatenate([g, g[:, ::-1]], axis=1)
    big = np.concatenate([row, row[::-1]], axis=0)
    big = np.concatenate([big, big], axis=1)
    return np.ascontiguousarray(big[:h, :w])


# ---------------------------------------------------------------- noise reduction family
if 'noise' in SECTIONS:
    with Case(MODULE, 'noise_params') as case:
        rows = []
        for amount in [0, 1, 10, 25, 37.5, 50, 64, 75, 100, 150]:
            for ps in [1., .5, .25, .125, 2.]:
                r, eps, ceps, blend = P.chroma_guided_params(amount, ps)
                rows.append([amount, ps, r, float(eps), float(ceps), float(blend), float(P.wavelet_lambda(amount, ps))])
        case.json(rows=rows, kernels=[P.wavelet_kernel(level).tolist() for level in range(5)])
    other = None
    for name in ['garden', 'noise', 'colours']:
        lab = P.rgb_to_lab(IMGS[name])
        L = np.ascontiguousarray(lab[..., 0])
        with Case(MODULE, f'noise_{name}') as case:
            case.array('lab', lab)
            profiles = P.wavelet_profiles(L)
            case.array('profiles', profiles)
            s0 = P._atrous_smooth(L, 0)
            case.array('profile0', P._noise_profile(L - s0, s0))
            for amount in [25, 70, 100]:
                for ps in [1., .25]:
                    case.array(f'wavelet_{amount}_{ps}', P.luma_wavelet(L, amount, ps))
                    exact(case, f'chroma_{amount}_{ps}', P.chroma_guided(lab.copy(), amount, ps))
                for ps in [1., .5, .25]:
                    case.array(f'bilateral_{amount}_{ps}', P.luma_noise_reduction(L, amount, ps))
            if other is not None:
                case.array('given_profiles', other)
                case.array('wavelet_given', P.luma_wavelet(L, 60, 1., other))
            other = profiles


# ---------------------------------------------------------------- detail_tools and sharpen
TOOLS = [
    ('texture_50', dict(texture=50.)), ('texture_-60', dict(texture=-60.)), ('texture_100', dict(texture=100.)),
    ('dehaze_40', dict(dehaze=40.)), ('dehaze_-50', dict(dehaze=-50.)), ('dehaze_100', dict(dehaze=100.)),
    ('defringe_60', dict(defringe=60.)), ('defringe_100', dict(defringe=100.)),
    ('luma_60_v1', dict(noise_luma=60., detail_version=1)), ('luma_60_v2', dict(noise_luma=60., detail_version=2)),
    ('luma_30_v3', dict(noise_luma=30., detail_version=3)), ('luma_100_v3', dict(noise_luma=100., detail_version=3)),
    ('color_40', dict(noise_color=40.)), ('color_100', dict(noise_color=100.)),
    ('all_v3', dict(noise_luma=50., noise_color=30., dehaze=25., texture=35., defringe=70., detail_version=3)),
    ('all_v1', dict(noise_luma=50., noise_color=30., dehaze=-20., texture=-35., defringe=40., detail_version=1)),
]
SHARPEN = [(50., .8, 50., 0., False), (50., .8, 50., 0., True), (150., .5, 25., 0., False), (150., 3., 100., 0., True),
           (80., 1.2, 0., 40., False), (80., 1.2, 75., 100., True), (40., .8, 100., 0., False), (20., 2., 10., 5., True)]
if 'tools' in SECTIONS:
    names = list(IMGS)
    for t, (label, changes) in enumerate(TOOLS):
        changes = {'detail_version': 3, **changes}
        s = settings(**changes)
        for name in (names[t % 5], names[(t + 2) % 5]):
            for ps in (1., .25) if t % 3 else (1., .5, .25):
                with Case(MODULE, f'tools_{label}_{name}_{ps}') as case:
                    case.json(image=name, pixel_scale=ps, settings=changes)
                    store(case, P.detail_tools(IMGS[name].copy(), s, ps), s)
    for t, (amount, radius, detail, masking, lum) in enumerate(SHARPEN):
        for name in (names[t % 5], names[(t + 3) % 5]):
            for ps in (1., .25):
                with Case(MODULE, f'sharpen_{t}_{name}_{ps}') as case:
                    case.json(image=name, pixel_scale=ps, amount=amount, radius=radius, detail=detail, masking=masking,
                              luminance=lum)
                    store(case, P.sharpen(IMGS[name].copy(), amount, radius, detail, masking, ps, luminance=lum),
                          settings())


# ---------------------------------------------------------------- the stage (_develop_detail)
STAGE = [
    ('default', {}), ('clarity_60', dict(clarity=60.)), ('clarity_-50', dict(clarity=-50.)),
    ('clarity_100', dict(clarity=100.)), ('monochrome', dict(monochrome=True)),
    ('sharpen_v1', dict(sharpen=70., sharpen_radius=1.1, sharpen_detail=30., sharpen_mask=20., detail_version=1)),
    ('sharpen_v3', dict(sharpen=70., sharpen_radius=1.1, sharpen_detail=30., sharpen_mask=20., detail_version=3)),
    ('combo_v3', dict(noise_luma=40., noise_color=25., texture=20., clarity=30., dehaze=15., defringe=50., sharpen=40.,
                      detail_version=3)),
    ('combo_v2_mono', dict(noise_luma=40., texture=-20., clarity=-30., sharpen=60., sharpen_mask=30., monochrome=True,
                           detail_version=2)),
    ('vignette', dict(vignette=-40., vignette_roundness=30., vignette_feather=70., clarity=20.)),
    ('grain', dict(grain=40., grain_size=30., grain_roughness=60., texture=30.)),
    ('combo_all', dict(noise_luma=30., noise_color=20., texture=25., clarity=25., dehaze=10., defringe=40., sharpen=50.,
                       sharpen_mask=10., vignette=-30., vignette_highlights=40., grain=25., detail_version=3)),
]
if 'stage' in SECTIONS:
    for t, (label, changes) in enumerate(STAGE):
        s = engine.normalized(changes)
        changes = {**changes, 'detail_version': s['detail_version']}
        for name in IMGS:
            for ps in (1., .25):
                if label not in ('default', 'combo_all', 'combo_v3') and ps == .25 and name not in ('garden', 'edges'):
                    continue
                with Case(MODULE, f'stage_{label}_{name}_{ps}') as case:
                    case.json(image=name, pixel_scale=ps, settings=changes)
                    store(case, engine._develop_detail(IMGS[name].copy(), s, ps), s)
        if label in ('default', 'clarity_60', 'combo_all'):
            with Case(MODULE, f'stage_{label}_garden_full_1.0') as case:
                case.json(image='garden_full', pixel_scale=1., settings=changes)
                store(case, engine._develop_detail(GARDEN.copy(), s, 1.), s)
    # The HDR branch (unreachable through develop while engine.HDR_FEATURE is off): detail tools on pixels divided
    # by their highlight gain. Called directly with hdr on, on the unclipped wide ramp darkened towards the top.
    for label, changes in [('hdr_texture', dict(texture=60., dehaze=20.)),
                           ('hdr_noise', dict(noise_luma=50., noise_color=40., clarity=30.))]:
        s = {**engine.normalized(changes), 'hdr': True}
        changes = {**changes, 'hdr': True, 'detail_version': s['detail_version']}
        wide = np.ascontiguousarray(IN['wide'] * np.linspace(.2, 1, 64, dtype=np.float32)[:, None, None], np.float32)
        with Case(MODULE, f'stage_{label}') as case:
            case.json(pixel_scale=1., settings=changes)
            case.array('input', wide)
            case.array('output', engine._develop_detail(wide.copy(), s, 1.))
    # A larger frame: row-band parallel paths and the thread-count check.
    big = tiled_srgb(300, 451)
    for label, changes in [('large_v3', dict(STAGE[-1][1])), ('large_v1', dict(STAGE[5][1], noise_luma=30.))]:
        s = engine.normalized(changes)
        changes = {**changes, 'detail_version': s['detail_version']}
        out = engine._develop_detail(big.copy(), s, 1.)
        with Case(MODULE, f'stage_{label}') as case:
            case.json(shape=list(big.shape), pixel_scale=1., settings=changes, bits_sum=str(bits_sum(out)), step=[5, 7],
                      sum=float(out.sum(dtype=np.float64)))
            case.array('output_sub', out[::5, ::7])
