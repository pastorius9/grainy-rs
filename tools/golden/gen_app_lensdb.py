"""Goldens for the built-in lens database (HANDOFF2 section C).

The oracle chooses a camera and a lens automatically from photo metadata (`optics.match_metadata`,
`optics.automatic_profile`) with its full bundled Lensfun database. This generator runs that choice for

  * the EXIF of the CC0 sample raws (camera, lens name and shooting values as the oracle reads them; only the
    keys that matter for the choice are kept),
  * the same cameras paired with a real lens of the database, and
  * real camera/lens pairs spread over the makers and the mounts of the database,

and stores the chosen camera, the candidate lens ids, the reason and the profile with its numbers (the lens XML
is stored as a SHA-256, plus the interpolated distortion / TCA / vignetting terms at the profile's conditions).
Only the oracle's database XML and the CC0 samples' tags are read. Run:

    "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_lensdb.py
"""
import sys
sys.dont_write_bytecode = True
from common import Case, ROOT, np, write_json  # noqa: E402
import hashlib  # noqa: E402
import json  # noqa: E402
import os  # noqa: E402
from pathlib import Path  # noqa: E402
import lensfunpy as lf  # noqa: E402
from luma import optics  # noqa: E402
from luma.engine import read_metadata  # noqa: E402

# The CC0 camera samples (a folder of RAW files): GRAINY_SAMPLES; see CONTRIBUTING.md.
if not os.environ.get('GRAINY_SAMPLES', '').strip():
    raise SystemExit('GRAINY_SAMPLES is not set: set it to the folder of the CC0 RAW samples; see CONTRIBUTING.md.')
SAMPLES = Path(os.environ['GRAINY_SAMPLES'].strip())
KEEP = ('make', 'camera', 'lens', 'focal_length', 'aperture', 'subject_distance')
OTHER_PAIRS = 40


def terms(calibration):
    if calibration is None:
        return None
    return {'model': calibration.model.name.lower(), 'terms': [float(t) for t in calibration.terms]}


def numbers(profile):
    """The profile without its XML, plus what the lens model interpolates at the profile's conditions."""
    if profile is None:
        return None
    lens = optics.profile_lens({'xml': profile['xml']})
    focal, aperture, distance = profile['focal'], profile['aperture'], profile['distance']
    out = {k: v for k, v in profile.items() if k != 'xml'}
    out['xml_sha256'] = hashlib.sha256(profile['xml'].encode()).hexdigest()
    out['xml_length'] = len(profile['xml'])
    out['distortion'] = terms(lens.interpolate_distortion(focal))
    out['tca'] = terms(lens.interpolate_tca(focal))
    out['vignetting'] = terms(lens.interpolate_vignetting(focal, aperture, distance)) if aperture > 0 else None
    return out


def run(metadata, origin):
    camera, candidates, reason = optics.match_metadata(metadata)
    profile, why = optics.automatic_profile(metadata)
    return dict(origin=origin, metadata=metadata, camera=None if camera is None else camera.model,
                camera_variant=None if camera is None else (camera.variant or ''),
                candidates=[r['id'] for r in candidates], reason=reason, profile_reason=why,
                profile=numbers(profile), description=optics.description(profile))


def unique_lens(camera, prefer_maker=True):
    """A real lens of the database that the oracle picks unambiguously for this camera."""
    options = []
    for record in optics.compatible_lenses(camera):
        if not record['model']:
            continue
        meta = dict(make=camera.maker, camera=camera.model, lens=record['model'], focal_length=1, aperture=4)
        c, rs, reason = optics.match_metadata(meta)
        if c is not None and len(rs) == 1 and rs[0]['id'] == record['id']:
            lens = optics.profile_lens(record)
            if lens.min_focal and lens.interpolate_distortion(lens.min_focal) is not None:
                same = record['maker'].lower().split()[0] in camera.maker.lower() or camera.maker.lower().split()[0] in record['maker'].lower()
                options.append((not same if prefer_maker else 0, record['maker'], record['model'], record))
    options.sort(key=lambda o: o[:3])
    return options[0][3] if options else None


def lens_metadata(camera, record):
    lens = optics.profile_lens(record)
    focal = round(float((lens.min_focal * lens.max_focal) ** .5), 1) if lens.max_focal else float(lens.min_focal)
    return dict(make=camera.maker, camera=camera.model, lens=record['model'], focal_length=focal, aperture=4.0,
                subject_distance=1000.0, source_orientation=1)


def main():
    cams = optics.cameras()
    cases = []

    # 1. The CC0 samples, as the oracle reads their tags.
    for path in sorted(SAMPLES.glob('*')):
        if path.suffix.lower() in ('.json', '.txt'):
            continue
        meta = read_metadata(path)
        meta = {k: meta[k] for k in KEEP if k in meta}
        meta['source_orientation'] = 1
        cases.append(run(meta, 'sample:' + path.name))
        # The same camera with a real lens of the database (samples often carry no lens name).
        c, _, _ = optics.match_metadata(meta)
        if c is not None:
            record = unique_lens(c)
            if record is not None:
                cases.append(run(lens_metadata(c, record), 'sample-camera+lens:' + path.name))

    # 2. Real camera/lens pairs spread over makers and mounts.
    groups = {}
    for c in cams:
        matches = [x for x in cams if optics._key(x.maker) == optics._key(c.maker)
                   and optics._model_key(x.model, x.maker) == optics._model_key(c.model, c.maker)]
        if len(matches) == 1:
            groups.setdefault((c.maker, c.mount), []).append(c)
    keys = sorted(groups)
    step = max(len(keys) // OTHER_PAIRS, 1)
    chosen = 0
    for key in keys[::step]:
        for c in groups[key]:
            record = unique_lens(c)
            if record is not None:
                cases.append(run(lens_metadata(c, record), f'pair:{c.maker}/{c.mount}'))
                chosen += 1
                break
    # 3. EXIF-style spellings and failure paths.
    cases += [
        run({}, 'empty'),
        run({'make': 'Canon', 'camera': 'not a camera'}, 'unknown-camera'),
        run({'make': 'NIKON CORPORATION', 'camera': 'NIKON D750', 'lens': 'AF-S Nikkor 24-70mm f/2.8G ED',
             'focal_length': 35.0, 'aperture': 5.6}, 'exif-spelling'),
        run({'make': 'Canon', 'camera': 'Canon EOS 5D Mark IV', 'lens': 'EF24-70mm f/2.8L II USM',
             'focal_length': 50.0, 'aperture': 4.0}, 'exif-spelling'),
        run({'make': 'SONY', 'camera': 'ILCE-7M3', 'lens': 'FE 24-70mm F2.8 GM', 'focal_length': 50.0,
             'aperture': 2.8}, 'exif-spelling'),
    ]
    print(len(cases), 'cases;', chosen, 'database pairs;', sum(1 for c in cases if c['profile']), 'with a profile')
    makers = sorted({c['metadata'].get('make', '') for c in cases})
    with Case('app', 'lensdb_pairs') as case:
        case.json(cases=cases, database_files=len(list((Path(lf.__file__).parent / 'db_files').glob('*.xml'))),
                  lensfunpy=lf.__version__ if hasattr(lf, '__version__') else '', lensfun_version=list(lf.lensfun_version),
                  cameras=len(cams), lenses=len(optics.lens_records()), makers=makers)


main()
