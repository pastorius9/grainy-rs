"""Goldens for grainy-core: the standard inputs, the default settings, the panel table, and normalized()/effective()."""
from common import Case, GOLDEN, ROOT, inputs, np, settings, write_json

from luma import engine, panel_keys

# The standard inputs, loadable by name from Rust.
folder = GOLDEN / '_inputs'
folder.mkdir(parents=True, exist_ok=True)
for name, image in inputs().items():
    np.save(folder / f'{name}.npy', np.ascontiguousarray(image, dtype=np.float32), allow_pickle=False)

# The defaults and the panel table are compiled into grainy-core.
assets = ROOT / 'crates' / 'grainy-core' / 'assets'
write_json(assets / 'defaults.json', engine.defaults())
write_json(assets / 'panels.json', {'panel_keys': {k: list(v) for k, v in panel_keys.PANEL_KEYS.items()},
                                   'always': list(panel_keys.ALWAYS), 'switches': panel_keys.SWITCHES,
                                   'off_panels': list(panel_keys.OFF_PANELS)})

# normalized() and effective(normalized()) for stored settings of different ages.
stored = {
    'empty': {},
    'unknown_keys': {'exposure': 1.5, 'not_a_setting': 3, 'contrast': 20.0},
    'old_gain_above_one': {'exposure': 1.0},                               # no tone_version: process 1
    'old_no_gain': {'exposure': -1.0, 'shadows': 30.0},                    # no tone_version: process 2 kept
    'old_no_gain_plain': {'exposure': -1.0},                               # 2 becomes 3 without those sliders
    'version2_with_shadows': {'tone_version': 2, 'shadows': 10.0},
    'old_sharpen': {'sharpen': 40.0},                                      # no detail_version: 1
    'old_noise': {'noise_luma': 20.0},                                     # no detail_version: 2
    'hdr_stored': {'hdr': True, 'exposure': 0.5, 'tone_version': 3},
    'panels_off': {'panels_off': ['effects', 'basic', 'nonsense'], 'exposure': 1.0, 'vignette': -30.0,
                   'tone_version': 3, 'film_look': 'kodak-portra-400'},
    'panels_off_not_a_list': {'panels_off': 'basic', 'tone_version': 3},
    'film_and_drt': {'film_look': 'kodak-portra-400', 'film_strength': 60.0, 'tone_mapping': 'drt', 'drt_toe': 35.0,
                     'tone_version': 3},
}
cases = {}
for name, value in stored.items():
    normal = engine.normalized(value)
    cases[name] = {'stored': value, 'normalized': normal, 'effective': panel_keys.effective(normal)}
with Case('core', 'normalized') as case:
    case.json(cases=cases)
# white_balance() and tone_white(), the gain that decides the process version of old edits.
from luma.processing import white_balance  # noqa: E402

gains = []
for change in [dict(), dict(temperature=40.0), dict(temperature=-100.0, tint=60.0), dict(tint=-80.0, exposure=2.0),
               dict(kelvin_enabled=True, kelvin=3200.0, wb_reference=5500.0),
               dict(kelvin_enabled=True, kelvin=9000.0, wb_reference=6500.0, temperature=20.0, tint=-10.0,
                    lens_vignette=40.0, exposure=-1.5),
               dict(kelvin_enabled=True, kelvin=1500.0, wb_reference=60000.0),
               dict(kelvin_enabled=True, kelvin=6600.0, wb_reference=1900.0),
               dict(lens_vignette=-50.0, exposure=0.7)]:
    s = settings(**change)
    gains.append({'settings': s, 'gain': white_balance(s), 'tone_white': engine.tone_white(s)})
with Case('core', 'white_balance') as case:
    case.json(cases=gains)
print('core goldens written:', len(cases), 'settings cases,', len(inputs()), 'inputs')
