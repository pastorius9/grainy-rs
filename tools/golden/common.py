"""Golden reference data from the Python Grainy (the oracle), for the Rust port's tests.

Run a generator with the oracle's own interpreter, from anywhere:
    "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_<module>.py

GRAINY_ORACLE must name the Python Grainy repository (the oracle); see CONTRIBUTING.md.

A generator imports this module first (it selects the CPU reference path and puts the oracle on sys.path),
computes with the oracle's functions and writes cases:

    from common import Case, inputs, np
    from luma import engine
    with Case('tone', 'rolloff_white_2') as case:
        a = inputs()['ramp']
        case.array('input', a)
        case.json(white=2.0)
        case.array('output', engine.highlight_rolloff(a.copy(), 2.0))

A case is the folder golden/<module>/<name>/ with one .npy per array (dtype kept: float32 stays float32) and
case.json for scalars and settings. Rust reads them through grainy-testkit. Keep arrays small (inputs are
96x64 or 240x160) so the goldens can live in git. Generators are deterministic: no clocks, seeded random only.
The oracle repository is read-only: never edit it from here.
"""
import json
import os
import sys
from pathlib import Path

os.environ['GRAINY_GPU'] = '0'                      # the CPU path is the reference
sys.dont_write_bytecode = True                       # nothing is written inside the oracle repository
os.environ.setdefault('QT_QPA_PLATFORM', 'offscreen')
os.environ.setdefault('PYTHONUTF8', '1')

ROOT = Path(__file__).resolve().parents[2]
ORACLE_VARIABLE = 'GRAINY_ORACLE'
if not os.environ.get(ORACLE_VARIABLE, '').strip():
    raise SystemExit(f'{ORACLE_VARIABLE} is not set: set it to the folder of the Python Grainy repository '
                     '(the oracle) and run this generator with its interpreter; see CONTRIBUTING.md.')
ORACLE = Path(os.environ[ORACLE_VARIABLE].strip())
if not (ORACLE / 'luma').is_dir():
    raise SystemExit(f'{ORACLE_VARIABLE}={ORACLE} does not look like the Python Grainy repository '
                     '(no luma/ package there).')
GOLDEN = ROOT / 'golden'
sys.path.insert(0, str(ORACLE))

import numpy as np  # noqa: E402


def _plain(value):
    """JSON-ready copy: numpy scalars and arrays become Python numbers and lists."""
    if isinstance(value, dict):
        return {str(k): _plain(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [_plain(v) for v in value]
    if isinstance(value, np.ndarray):
        return value.tolist()
    if isinstance(value, np.generic):
        return value.item()
    if isinstance(value, Path):
        return str(value)
    return value


def write_json(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(_plain(value), ensure_ascii=False, indent=1, sort_keys=True) + '\n', encoding='utf-8', newline='\n')


class Case:
    """One golden case: `with Case(module, name) as case:` then case.array(...) / case.json(...)."""

    def __init__(self, module, name):
        self.folder = GOLDEN / module / name
        self.meta = {}

    def __enter__(self):
        self.folder.mkdir(parents=True, exist_ok=True)
        for old in self.folder.glob('*.npy'):
            old.unlink()
        return self

    def array(self, name, value):
        """Store an array as it is (dtype kept; bool becomes uint8) and return it."""
        a = np.ascontiguousarray(value)
        if a.dtype == np.bool_:
            a = a.astype(np.uint8)
        np.save(self.folder / f'{name}.npy', a, allow_pickle=False)
        return value

    def json(self, **values):
        self.meta.update(values)

    def __exit__(self, kind, error, trace):
        if kind is None:
            write_json(self.folder / 'case.json', self.meta)
        return False


def inputs():
    """The standard input images: name -> float32 HxWx3, scene-linear, deterministic. Also stored once in
    golden/_inputs/ by gen_core.py so Rust tests can load them by name."""
    rng = np.random.default_rng(20261113)
    h, w = 64, 96
    yy, xx = np.mgrid[:h, :w].astype(np.float32)
    ramp = np.repeat((xx / (w - 1))[..., None], 3, axis=2).astype(np.float32)             # 0..1 grey ramp
    wide = (ramp * 6.0).astype(np.float32)                                                 # 0..6: highlights far above white
    colours = np.stack([xx / (w - 1), yy / (h - 1), 1 - xx / (w - 1) * yy / (h - 1)], -1).astype(np.float32)
    patches = np.zeros((h, w, 3), np.float32)                                              # 6x4 colour patches
    table = rng.random((4, 6, 3)).astype(np.float32) ** 2.2
    for j in range(4):
        for i in range(6):
            patches[j * 16:(j + 1) * 16, i * 16:(i + 1) * 16] = table[j, i]
    noise = (rng.random((h, w, 3)).astype(np.float32) ** 3 * 1.5).astype(np.float32)       # dark-heavy, some >1
    edges = np.full((h, w, 3), .08, np.float32)                                            # sharp edges and a disc
    edges[16:48, 24:72] = [.7, .5, .2]
    edges[(yy - 32) ** 2 + (xx - 48) ** 2 < 100] = [.15, .4, .8]
    result = {'ramp': ramp, 'wide': wide, 'colours': colours, 'patches': patches, 'noise': noise, 'edges': edges}
    photo = ORACLE / 'docs' / 'learn' / 'img' / 'garden.jpg'
    if photo.exists():
        from PIL import Image
        from luma.engine import to_linear
        with Image.open(photo) as image:
            small = np.asarray(image.convert('RGB').resize((240, 160), Image.Resampling.BOX), np.float32) / 255
        result['garden'] = np.ascontiguousarray(to_linear(small), dtype=np.float32)
    return result


def settings(**changes):
    """The oracle's default settings with changes, as develop() takes them."""
    from luma.engine import defaults
    value = defaults()
    value.update(changes)
    return value
