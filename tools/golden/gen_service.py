"""Goldens for grainy-service: luma/photo_service.py, photo_edits.py, photo_labels.py and command_batch.py.

Run with the oracle's interpreter from the worktree root (writes golden/service/):
    "$GRAINY_ORACLE/.venv/Scripts/python.exe" -B tools/golden/gen_service.py

Every catalogue is a temporary one made by the oracle's own Catalog; clocks and batch ids are fixed. Paths below
the temporary folder are written as <ROOT> (the Rust tests put their own folder there). Long text columns are
compared through their SHA-256 (after the root is replaced), JSON strings in columns are compared as strings.
"""
import sys
sys.dont_write_bytecode = True
from common import GOLDEN, _plain  # noqa: E402  (puts the oracle on sys.path)
import base64  # noqa: E402
import hashlib  # noqa: E402
import json  # noqa: E402
import shutil  # noqa: E402
import tempfile  # noqa: E402
from copy import deepcopy  # noqa: E402
from datetime import datetime  # noqa: E402
from pathlib import Path  # noqa: E402
from threading import Event  # noqa: E402
from types import SimpleNamespace  # noqa: E402
from unittest.mock import patch  # noqa: E402

import numpy as np  # noqa: E402

from luma import photo_service as service, photo_edits, photo_labels, command_batch  # noqa: E402
from luma import catalog as catalog_module, library_query, quick_export, preview_store, mcp_io, engine  # noqa: E402
from luma.catalog import Catalog  # noqa: E402
from luma.engine import defaults  # noqa: E402

STAMP = '2026-11-13T01:02:03.456789+00:00'
PLACE = '<ROOT>'
LONG = 400
OUT = GOLDEN / 'service'


class Clock(datetime):
    @classmethod
    def now(cls, tz=None):
        value = datetime.fromisoformat(STAMP)
        return value if tz else value.replace(tzinfo=None)


class Tokens:
    """secrets.token_hex(3) stand-in: a fixed sequence (with a repeat to exercise the id collision loop)."""
    def __init__(self, values):
        self.values = list(values); self.used = 0

    def token_hex(self, n):
        assert n == 3
        value = self.values[self.used]; self.used += 1
        return value


def write(name, value):
    path = OUT / name / 'case.json'
    path.parent.mkdir(parents=True, exist_ok=True)
    for old in path.parent.iterdir():
        old.unlink()
    path.write_text(json.dumps(value, ensure_ascii=False, separators=(',', ':')) + '\n', encoding='utf-8', newline='\n')


DEFAULTS = defaults()


def full(overrides):
    """A settings dict as the tests build it: {**defaults(), **overrides}."""
    return {**deepcopy(DEFAULTS), **deepcopy(overrides)}


def is_settings(value):
    return isinstance(value, dict) and len(value) >= 90 and 'exposure' in value and 'grain_size' in value


def reduce(settings):
    """A settings dict compared by its key order (hash) and its values that differ from the defaults (type kept)."""
    keys = hashlib.sha256(json.dumps(list(settings)).encode()).hexdigest()[:16]
    diff = {k: v for k, v in settings.items() if k not in DEFAULTS or json.dumps(v) != json.dumps(DEFAULTS[k])}
    missing = [k for k in DEFAULTS if k not in settings]
    return {'$settings': keys, 'diff': diff, 'missing': missing}


class Scrub:
    def __init__(self, root=None):
        self.forms = []
        if root is not None:
            r = str(root)
            for form in (json.dumps(r)[1:-1], r, json.dumps(r.lower())[1:-1], r.lower()):
                if form not in [f for f, _ in self.forms]:
                    self.forms.append((form, PLACE))

    def text(self, s):
        for form, place in self.forms:
            s = s.replace(form, place)
        return s

    def __call__(self, value):
        value = _plain(value)
        if isinstance(value, dict):
            if is_settings(value):
                return reduce({k: self(v) for k, v in value.items()})
            return {self.text(str(k)): self(v) for k, v in value.items()}
        if isinstance(value, list):
            return [self(v) for v in value]
        if isinstance(value, str):
            return self.text(value)
        return value

    def unplace(self, value, root):
        if isinstance(value, dict):
            return {k: self.unplace(v, root) for k, v in value.items()}
        if isinstance(value, list):
            return [self.unplace(v, root) for v in value]
        if isinstance(value, str):
            return value.replace(PLACE, str(root))
        return value


def error_of(error):
    kind = type(error).__name__
    if isinstance(error, __import__('sqlite3').Error):
        kind = 'sqlite3.Error'
    # Operating-system texts are localized and machine-specific: only the class is compared.
    out = {'type': kind, 'message': '<os message>' if isinstance(error, OSError) else str(error)}
    if isinstance(error, command_batch.BatchConflict):
        out['kind'] = error.kind
    return out


def run(fn):
    try:
        return {'result': fn()}
    except Exception as error:  # noqa: BLE001
        return {'error': error_of(error)}


def dump(db, scrub):
    result = {}
    tables = [r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")]
    for table in tables:
        if table == 'preview_state':
            continue
        columns = [r[1] for r in db.execute('PRAGMA table_info(' + table + ')')]
        rows = []
        for row in db.execute('SELECT * FROM ' + table + ' ORDER BY ' + ','.join('"' + c + '"' for c in columns)):
            item = {}
            for c, v in zip(columns, row):
                if isinstance(v, bytes):
                    v = {'blob': hashlib.sha256(v).hexdigest(), 'len': len(v)}
                elif isinstance(v, str):
                    v = scrub.text(v)
                    if len(v) > LONG:
                        v = {'sha': hashlib.sha256(v.encode('utf-8')).hexdigest(), 'len': len(v)}
                item[c] = v
            rows.append(item)
        result[table] = rows
    return result


# --- pure functions -------------------------------------------------------------------------------

def changes_of(args):
    return service.parse_changes(args.get('set'), args.get('add'))


def automatic_stand_in(record, settings, entry, directory):
    if 'bad' in record['name']:
        raise ValueError('cannot read')
    result = dict(settings)
    if entry.get('auto_straighten'):
        result.update(straighten=0, upright={'mode': 'level', 'rotation': 1.5})
    if entry.get('auto_crop'):
        result['crop'] = [0.05, 0.05, 0.95, 0.95]
    return engine.normalized(result)


def pure_call(fn, a):
    """One pure case; the Rust test has the same dispatcher."""
    if fn == 'parse_changes':
        changes, operations = service.parse_changes(a['set'], a['add'])
        return [changes, operations]
    if fn == 'curve_points':
        return service.curve_points(a['value'], a['key'])
    if fn == 'canvas':
        return service.canvas(full(a['settings']), a['info'])
    if fn == 'edited':
        return service.edited(full(a['settings']), *changes_of(a), a['info'], a['ident'])
    if fn == 'compose':
        return service.compose(full(a['settings']), [changes_of(g) for g in a['groups']], a['info'], a['ident'])
    if fn == 'preset_settings':
        return service.preset_settings(a['preset'], full(a['settings']), a['record'])
    if fn == 'copy_keys':
        return sorted(service.copy_keys(a['groups']))
    if fn == 'copied_settings':
        keys = service.copy_keys(a['groups'])
        return service.copied_settings(full(a['source']), full(a['settings']), a['info'], keys)
    if fn == 'where_options':
        return service.where_options(a['where'], a['sort'], a['descending'], a['allow_empty'])
    if fn == 'fmt':
        return service.fmt(a['value'])
    if fn == 'summary':
        return service.summary({**a['record'], 'settings': full(a['record']['settings'])})
    if fn == 'summaries':
        return list(service.summaries([{**r, 'settings': full(r['settings'])} for r in a['records']]))
    if fn == 'check_naming':
        return service.check_naming(a['naming'])
    if fn == 'file_stem':
        return service.file_stem(a['naming'], a['stem'], a['n'], a['values'])
    if fn == 'name_values':
        return service.name_values({**a['record'], 'settings': full(a['record'].get('settings', {}))})
    if fn == 'safe_filename':
        return service.safe_filename(a['stem'], a['suffix'], a['index'])
    if fn == 'english_error':
        kinds = {'PermissionError': PermissionError, 'FileNotFoundError': FileNotFoundError,
                 'FileExistsError': FileExistsError, 'OSError': OSError, 'IsADirectoryError': IsADirectoryError,
                 'ValueError': ValueError, 'KeyError': KeyError, 'RuntimeError': RuntimeError}
        return service.english_error(kinds[a['kind']]('x'))
    if fn == 'export_longest':
        longest = a['longest']
        return engine.export_longest((a['h'], a['w'], 3), tuple(longest) if isinstance(longest, list) else longest)
    if fn == 'planned_size':
        record = {**a['record'], 'settings': full(a['record']['settings'])}
        longest = a['longest']
        with patch('luma.engine.load_image', lambda *x, **k: (None, {'width': a['probe'][0], 'height': a['probe'][1]})):
            return service.planned_size(record, tuple(longest) if isinstance(longest, list) else longest)
    if fn == 'crop_value':
        return photo_edits.crop_value(a['value'])
    if fn == 'auto_job':
        records = [{**r, 'settings': r['settings']} for r in a['records']]
        return service.auto_job(a['kind'], records)[0]
    if fn == 'photo_edits.compose':
        records = [{**r, 'settings': full(r['settings'])} for r in a['records']]
        sources = {int(k): {**r, 'settings': full(r['settings'])} for k, r in a['sources'].items()}
        entries = []
        for e in a['entries']:
            entry = {'ids': e['ids'], 'copy_from': e.get('copy_from'), 'changes': changes_of(e) if ('set' in e or 'add' in e) else None,
                     'preset': e.get('preset')}
            if e.get('preset') is not None:
                entry['preset_name'] = e['preset']; entry['base'] = e['base']
            if e.get('copy_from') is not None:
                entry['keys'] = service.copy_keys(e.get('groups'))
            for key in ('auto_straighten', 'auto_crop'):
                if key in e:
                    entry[key] = e[key]
            if 'crop' in e:
                entry['crop'] = photo_edits.crop_value(e['crop'])
            entries.append(entry)
        calls = {'n': 0}

        def deadline(_):
            calls['n'] += 1
            if a['deadline'] is not None and calls['n'] > a['deadline']:
                raise service.ServiceError('the time limit was reached; ask for fewer photos or try again')
        with patch.object(mcp_io, 'check_deadline', deadline), patch.object(photo_edits, 'automatic', automatic_stand_in):
            used, values, failures, presets = photo_edits.compose(entries, records, sources, 'unused', 0)
        return [[r['id'] for r in used], values, failures, {str(k): v for k, v in presets.items()}]
    raise KeyError(fn)


def pure_cases():
    cases = []

    def add(fn, **a):
        cases.append({'fn': fn, 'args': a})

    for key, (_, low, high) in service.FIELDS.items():
        if key in ('monochrome', 'flip'):
            add('parse_changes', set={key: True}, add=None)
            continue
        if key == 'film_look':
            continue
        add('parse_changes', set={key: low}, add=None)
        add('parse_changes', set={key: high}, add=None)
        add('parse_changes', set=None, add={key: high - low})
        add('parse_changes', set=None, add={key: low - high})
        add('parse_changes', set={key: high + 0.5}, add=None)
        add('parse_changes', set=None, add={key: (high - low) + 1})
    for s, a in [({'exposure': True}, None), ({'path': 1}, None), ({'exposure': 5.01}, None), ({'sharpen': -1}, None),
                 (None, {'contrast': 201}), ({'exposure': 1}, {'exposure': 1}), ({}, {}), (None, None),
                 ({'exposure': '1'}, None), ('x', None), ([], {'exposure': 1}), ({'exposure': 1, 'contrast': -5}, {'whites': -300}),
                 ({'monochrome': 1}, None), (None, {'monochrome': True}), (None, {'flip': True}), ({'flip': 1}, None),
                 ({'rotation': 90}, None), ({'rotation': 45}, None), ({'rotation': 360}, None), (None, {'rotation': -90}),
                 (None, {'rotation': -360}), (None, {'rotation': 270.0}), ({'rotation': True}, None), ({'rotation': -90}, None),
                 ({'film_look': ''}, None), ({'film_look': 'Kodak Portra 400'}, None), ({'film_look': 'portra400'}, None),
                 ({'film_look': 'zzzz qqqq'}, None), ({'film_look': 'portra'}, None), ({'film_look': "it's"}, None),
                 ({'film_look': 'off'}, None), ({'film_look': 3}, None), (None, {'film_look': 'portra'}),
                 ({'film_look': 'kodak portra 40'}, None), ({'negative_density': 1.5}, {'negative_range': 2}),
                 ({'exposure': 10**30}, None), (None, {'exposure': -10}), (None, {'exposure': 10})]:
        add('parse_changes', set=s, add=a)
    curve = [[0, 0], [.25, .2], [.75, .8], [1, 1]]
    for s, a in [({'curve': curve, 'rgb_curves': [curve, [[0, 0], [1, .9]], [[0, .1], [1, 1]]]}, None),
                 ({'rgb_curves': [curve]}, None), ({'rgb_curves': [curve, curve, [[0, 0]]]}, None), (None, {'curve': curve}),
                 (None, {'rgb_curves': [curve] * 3}), ({'curve': curve}, {'curve': curve}), ({'rgb_curves': 'x'}, None)]:
        add('parse_changes', set=s, add=a)
    for bad in ([[0, 0]], [[0, 0], [.5, .5]], [[.1, 0], [1, 1]], [[0, 0], [.5, .5], [.5, .6], [1, 1]], [[0, 0], [1, 1.2]],
                [[0, 0], [1, True]], [[0, 0, 0], [1, 1]], [[0, 0]] * 17, 'x', [[0, 0], [1, 1]], [[0, 0]] + [[i / 16, .5] for i in range(1, 15)] + [[1, 1]],
                [[0.0, 0.0], [0.5, 0.25], [1.0, 1.0]], [[0, 0], [1, -0.0]], [[0, 0], 'p']):
        add('curve_points', value=bad, key='curve')
    add('curve_points', value=[[0, 0]], key='rgb_curves[2]')
    for settings, info in [({}, {'width': 600, 'height': 400}), ({'rotation': 1}, {'width': 600, 'height': 400}),
                           ({'rotation': 3}, {'width': 600.0, 'height': 400}), ({}, {'width': 0, 'height': 400}), ({}, {}),
                           ({'rotation': 2}, {'width': 5, 'height': 7})]:
        add('canvas', settings=settings, info=info)
    # edited
    E = lambda settings, s=None, a=None, info=None, ident=1: add('edited', settings=settings, set=s, add=a, info=info or {}, ident=ident)
    E({'sharpen': 50.}, None, {'sharpen': -20, 'grain': -100})
    E({}, {'monochrome': True})
    for exposure in (0., 4.8, -5.):
        E({'exposure': exposure, 'whites': -150.}, {'contrast': 10}, {'exposure': .5, 'whites': -300})
    E({'exposure': 0.0}, None, {'exposure': -0.0})
    E({'grain': 0.0}, None, {'grain': -5})
    E({'grain': 3}, None, {'grain': 0})
    E({'tone_version': 2, 'shadows': 10.}, None, {'exposure': .1})
    E({'tone_version': 2, 'shadows': 10.}, None, {'shadows': 5})
    E({'tone_version': 2, 'shadows': 10.}, {'highlights': -20})
    E({'tone_version': 2.0, 'shadows': 10.}, {'highlights': -20})
    for rotation in (0, 1):
        info = {'width': 600, 'height': 400}
        E({'rotation': rotation}, {'straighten': 6}, None, info)
        E({'rotation': rotation, 'straighten': 6., 'crop': [.1, .1, .6, .6]}, None, {'straighten': 4}, info)
        E({'rotation': rotation, 'straighten': 6.}, None, {'straighten': -12}, info)
        E({'rotation': rotation, 'straighten': 10., 'crop': [0.0, 0.0, 1.0, 1.0]}, {'straighten': 0}, None, info)
        E({'rotation': rotation, 'straighten': 2., 'crop': [.3, .2, .9, .95]}, {'straighten': 44}, None, info)
        E({'rotation': rotation, 'straighten': 2., 'crop': [.45, .45, .55, .55]}, {'straighten': -30}, None, info)
    E({'upright': {'mode': 'level'}}, {'straighten': 6})
    E({}, {'straighten': 3}, None, {}, 31)
    E({}, None, {'negative_density': .5}, {}, 4)
    E({'negative': True}, {'negative_range': 8}, {'negative_density': .5}, {}, 4)
    E({'negative': 1}, {'negative_range': 8})
    E({'crop': [.1, .1, .9, .9]}, {'rotation': 90})
    E({'crop': [.1, .1, .9, .9], 'rotation': 1}, None, {'rotation': -90})
    E({'crop': [.1, .1, .9, .9], 'rotation': 3}, None, {'rotation': 180})
    E({'crop': [.1, .1, .9, .9]}, {'rotation': 0})
    E({'crop': [.1, .1, .9, .9], 'rotation': 2.0}, {'rotation': 180})
    E({'crop': [.1, .1, .9, .9]}, {'flip': True})
    E({'panels_off': ['curve', 'effects']}, {'curve': curve, 'rgb_curves': [curve, [[0, 0], [1, .9]], [[0, .1], [1, 1]]]})
    E({'panels_off': ['basic', 'effects', 'film']}, None, {'exposure': .3, 'grain': 10})
    E({'photo_filter_enabled': False}, None, {'grain': 10})
    E({}, {'film_look': 'Kodak Portra 400'})
    E({}, {'film_look': 'Kodak Portra 400', 'grain': 5})
    portra = service.parse_changes({'film_look': 'Kodak Portra 400'})[0]['film_look']
    with_film = __import__('luma.film_look', fromlist=['with_film']).with_film(full({}), portra)
    grain = {k: with_film[k] for k in ('grain', 'grain_size', 'grain_roughness')}
    E({'film_look': portra, **grain}, {'film_look': ''})
    E({'film_look': portra, **grain, 'grain': 40.}, {'film_look': ''})
    E({'film_look': portra, **grain, 'panels_off': ['film']}, None, {'film_strength': -10})
    E({'vignette_midpoint': 50}, None, {'vignette_midpoint': 0})
    for key, (_, low, high) in service.FIELDS.items():      # every edit field, set and add, as the sliders do
        if key in ('monochrome', 'flip', 'film_look', 'rotation'):
            continue
        base = {'negative': True} if key in service.NEGATIVE_ONLY else {}
        E({**base, 'panels_off': ['basic', 'effects', 'film', 'detail']}, {key: (low + high) / 4 + low / 2}, None, {'width': 300, 'height': 200})
        E({**base, key: high}, None, {key: (high - low) / 3}, {'width': 300, 'height': 200})
    # compose
    add('compose', settings={}, groups=[{'set': {'exposure': 1}}, {'add': {'exposure': .25}}], info={}, ident=1)
    add('compose', settings={'exposure': 1.0}, groups=[{'add': {'exposure': 3}}, {'add': {'exposure': 3}}, {'set': {'monochrome': True}}], info={}, ident=1)
    add('compose', settings={}, groups=[], info={}, ident=1)
    # presets and copied settings
    base = {'crop': [.1, .1, .9, .9], 'rotation': 1, 'straighten': 2., 'flip': True, 'exposure': 1.}
    for preset, record in [({'contrast': 12, 'exposure': .15}, {'path': 'D:/a.jpg', 'info': {'format': 'JPEG'}}),
                           ({'contrast': 12, 'raw_mode': 'sample', 'raw_neutral': [1, 1, 1]}, {'path': 'D:/a.nef', 'info': {'format': 'NEF'}}),
                           ({'raw_mode': 'sample', 'crop': [0, 0, .5, .5], 'rotation': 2}, {'path': 'D:/a.jpg', 'info': {}}),
                           ({'exposure': 2, 'panels_off': ['basic', 'nonsense']}, {'path': 'D:/a.jpg', 'info': {'format': None}})]:
        add('preset_settings', preset=preset, settings=base, record=record)
    for groups in (None, [], ['curves', 'light'], ['crop'], ['masks', 'retouch', 'negative'], ['bogus']):
        add('copy_keys', groups=groups)
    source = {'exposure': .7, 'curve': [[0., 0.], [.5, .6], [1., 1.]], 'crop': [.2, .2, .8, .8], 'panels_off': ['curve'], 'temperature': 9.}
    target = {'contrast': 5., 'crop': [.1, .1, .9, .9], 'panels_off': ['effects', 'grading']}
    for groups in (None, ['curves'], ['crop'], ['effects', 'light'], ['white_balance']):
        add('copied_settings', source=source, settings=target, info={'format': 'JPEG'}, groups=groups)
    add('copied_settings', source={'raw_mode': 'sample', 'kelvin': 4000}, settings={}, info={'format': 'NEF'}, groups=['white_balance'])
    # where_options
    W = lambda where, sort=None, descending=False, allow_empty=False: add('where_options', where=where, sort=sort, descending=descending, allow_empty=allow_empty)
    W({'text': 'roll', 'folder': 'D:/Film', 'recursive': True, 'flag': 'picked', 'rating_min': 3, 'label': 'red', 'collection': 4}, 'capture', True)
    for where in ({'flag': 'unflagged'}, {'flag': 'rejected'}, {'flag': 'other'}, {'flag': None, 'text': 'x'}, {'folder': 'x'},
                  {'recursive': True, 'text': 'a'}, {'recursive': False}, {'rating_max': 0}, {'rating_min': None, 'text': ''},
                  {'label': 'purple'}, {'label': 'pink'}, {'label': None, 'text': 'y'}, {'text': 5}, {'collection': 0}):
        W(where)
    W({})
    W(None)
    W({}, allow_empty=True)
    W(None, 'rating', True, True)
    W({'text': 'a'}, 'random')
    for sort in ('imported', 'name', 'capture'):
        W({'text': 'a'}, sort)
    # fmt
    for value in (.304, 12., True, False, -0.004, 2.346, 2.345, 7, 0.125, 0.375, -1.005, 2.675, 1e16, 123456.785, -0.0, 0.005, 1.5e-7, 99.995):
        add('fmt', value=value)
    # summaries
    record = {'id': 12, 'name': 'R12_01.NEF', 'path': 'D:/Film/R12_01.NEF', 'rating': 3, 'flag': 1, 'label': '빨강', 'keywords': '롤12',
              'settings': {'exposure': .304, 'contrast': 12., 'monochrome': True, 'crop': [.1, .1, .9, .9], 'masks': [{'a': 1}, {'b': 2}],
                           'hsl': [[1., 0., 0.]] + [[0., 0., 0.]] * 7, 'negative': True, 'tone_version': 1},
              'info': {'width': 6048, 'height': 4024, 'format': 'NEF', 'camera': 'Z 6', 'iso': 400., 'aperture': 2.8, 'shutter': '1/250',
                       'focal_length': 50., 'date': '2026:10:01 10:00:00', 'lens': '50mm', 'latitude': 37.5, 'serial': 'SN1'}}
    add('summary', record=record)
    add('summary', record={**record, 'settings': {}, 'rating': 0, 'flag': 0, 'label': '', 'keywords': '', 'info': {}})
    add('summary', record={'id': 3, 'name': 'a.png', 'path': 'D:/a.png', 'settings': {'film_look': portra, 'film_strength': 80.}})
    add('summary', record={'id': 3, 'name': 'a.png', 'path': 'D:/a.png', 'settings': {'film_look': 'gone-film'}})
    add('summary', record={'id': 1, 'name': 'a.jpg', 'path': 'D:/a.jpg', 'label': 'custom',
                           'settings': {'rotation': 3, 'flip': True, 'upright': {'mode': 'level', 'rotation': 1.234}, 'panels_off': ['basic'],
                                        'curve': [[0., 0.], [.33333333, .4], [1., 1.]], 'rgb_curves': [[[0, 0], [1, 1]], [[0, 0.123456], [1, .9]], [[0, 0], [1, 1]]],
                                        'retouch': [1, 2, 3], 'grading_balance': 10., 'lensfun_enabled': True, 'tone_mapping': True, 'parametric': [1, 0, 0, 0],
                                        'working_space': 'srgb', 'negative_peak': 0.9},
                           'info': {'iso': 0, 'aperture': '', 'focal_length': None, 'camera': 0.0, 'format': False, 'shutter': 0.004, 'date': 20261001}})
    add('summary', record={'id': 2, 'name': 'b.jpg', 'path': 'D:/b.jpg', 'settings': {'exposure': 0, 'curve': [[0, 0], [.25, .25], [.5, .5], [.75, .75], [1, 1]]}})
    add('summaries', records=[
        {'id': 3, 'name': 'a.png', 'path': 'D:/Film/a.png', 'settings': {'exposure': 1.}},
        {'id': 4, 'name': 'b.png', 'path': 'D:/Other/b.png', 'settings': {}},
        {'id': 5, 'name': 'a.png · 사본', 'path': 'D:/Film/a.png', 'virtual_source': 3, 'settings': {'crop': [.1, .1, .9, .9]}},
        {'id': 6, 'name': 'c.png', 'path': 'D:/Other/c.png', 'virtual_source': None, 'settings': {}}])
    add('summaries', records=[])
    # export naming
    for naming in ('{stem}_grainy', 'R12_{n:03d}', '{stem}-{n}', '{n:06d}', '사진_{stem}', '{n:>999999999}', '{stem.__class__}', '{0}', '{}',
                   'a/b', 'x:', 'end.', 'end ', 'bad\x07', 'a' * 81, 'a' * 80, '{n:07d}', '{n:3d}', 'CON', 'con.{stem}', 'COM1', 'LPT9.x',
                   '{stem}_{film}', '{date}_{camera}_{preset}_{n:03d}', '{film}', '{films}', '{date:%Y}', '{camera.x}', '{preset!r}', '', ' ',
                   '{stem', 'stem}', '{{stem}}', 'COM0', '{n:02d}{n:06d}', 'a\\b', 'a|b', 5, None):
        add('check_naming', naming=naming)
    for naming, values in [('{stem}_{film}', {'film': 'Kodak Portra 400'}), ('{stem}_{film}', {}), ('{film}_{stem}', {}),
                           ('{date}-{camera}-{n:03d}', {'date': '2026-10-01'}), ('{film} {preset}', {}), ('{film}_{preset}_{stem}', {'preset': 'Soft'}),
                           ('{stem}', {}), ('{camera}.{date}', {'camera': 'Z 6'}), ('R12_{n:02d}', {}), ('{film}', {}), ('{film}{preset}', {}),
                           ('a__{film}', {}), ('{film}-.x', {}), (' {film} ', {}), ('{film}...', {'film': ''}), ('x{n}{film}', {'film': None}),
                           ('{film}_{camera}_{date}', {'camera': 'Z', 'date': ''}), ('{n:06d}', {})]:
        add('file_stem', naming=naming, stem='roll', n=7, values=values)
    for rec in [{'settings': {'film_look': portra}, 'info': {'camera': 'X/Y:Z', 'date': '2026:10:01 10:00:00'}, 'user_metadata': {'_preset': 'a/b'}},
                {'settings': {'film_look': portra}, 'info': {'camera': 'X/Y:Z', 'date': '2026:10:01 10:00:00'}, 'user_metadata': {'DateTimeOriginal': '2025:01:02 03:04:05'}},
                {'settings': {}, 'info': {}, 'user_metadata': {}},
                {'settings': {'film_look': 'unknown'}, 'info': {'camera': '  Cam. ', 'date': '2026-1-01'}, 'user_metadata': None},
                {'info': {'camera': 'C' * 70 + '\x01', 'date': '٢٠٢٦:١٠:٠١'}, 'user_metadata': {'_preset': ' trailing.. '}},
                {'info': {'date': 20261001, 'camera': 123}, 'user_metadata': {'DateTimeOriginal': '', '_preset': ''}}]:
        add('name_values', record=rec)
    for stem, suffix, index in [('roll', '.jpg', 1), ('roll', '.jpg', 3), ('CON', '.jpg', 1), ('con.backup', '.jpg', 1), ('COM²', '.png', 1),
                                ('a<b>c', '.tif', 1), ('  ', '.jpg', 1), ('...', '.jpg', 2), ('x' * 300, '.jpg', 1), ('한' * 120, '.jpg', 12),
                                ('e\u0301', '.jpg', 1), ('😀' * 130, '.jpg', 1), ('name. ', '.jpg', 1), ('', '', 1), ('AUX', '', 2),
                                ('x' * 251 + '. ', '.jpg', 1)]:
        add('safe_filename', stem=stem, suffix=suffix, index=index)
    for kind in ('PermissionError', 'FileNotFoundError', 'FileExistsError', 'OSError', 'IsADirectoryError', 'ValueError', 'KeyError', 'RuntimeError'):
        add('english_error', kind=kind)
    for longest in ([1080, 0, 'inside'], [0, 1350, 'inside'], [1080, 1080, 'inside'], [1080, 1080, 'cover'], [8000, 0, 'inside'], [0, 0, 'inside'],
                    [1000, 1000, 'cover'], 2000, 0, [7, 5, 'cover'], [1, 1, 'inside']):
        add('export_longest', h=4000, w=6000, longest=longest)
    for h, w in ((2999, 4001), (1001, 3333), (37, 4999)):
        for limits in ((1000, 0), (333, 777), (7, 5)):
            for fit in ('inside', 'cover'):
                add('export_longest', h=h, w=w, longest=[*limits, fit])
    for settings, info, longest in [({}, {'width': 6000, 'height': 4000}, 0), ({'rotation': 1}, {'width': 6000, 'height': 4000}, 1000),
                                    ({'crop': [.1, .2, .7, .9]}, {'width': 6000, 'height': 4000}, [1080, 1080, 'cover']),
                                    ({'crop': [0.0001, 0.0001, 0.0002, 0.0002]}, {'width': 600, 'height': 400}, 0),
                                    ({'crop': [.125, .125, .875, .875], 'rotation': 3}, {'width': 100, 'height': 60}, 50),
                                    ({}, {}, [48, 40, 'inside']), ({}, {'width': 0, 'height': 10}, 0), ({}, {'width': 10, 'height': 10}, 100)]:
        add('planned_size', record={'path': 'x.jpg', 'settings': settings, 'info': info}, longest=longest, probe=[96, 64])
    for value in (None, [0, 0, 1, 1], [.1, .2, .3, .4], [0, 0, 1], [.5, 0, .5, 1], [0, .6, 1, .5], [0, 0, 1, 1.1], [True, 0, 1, 1], 'x', [0, 0, 1, 1, 0]):
        add('crop_value', value=value)
    recs = [{'id': 1, 'path': 'a.jpg', 'settings': {'exposure': 1, 'negative': False}}, {'id': 2, 'path': 'b.nef', 'settings': {}}]
    for kind in ('tone', 'color', 'negative', 'roll', 'other'):
        add('auto_job', kind=kind, records=recs)
    # photo_edits.compose
    missing = 'nonexistent-folder-of-grainy-tests'
    records = [{'id': 1, 'name': 'a.jpg', 'path': missing + '/a.jpg', 'settings': {'exposure': .5}, 'info': {'width': 600, 'height': 400, 'format': 'JPEG'}},
               {'id': 2, 'name': 'b.jpg', 'path': missing + '/b.jpg', 'settings': {'negative': True, 'panels_off': ['curve']}, 'info': {'format': 'JPEG'}},
               {'id': 3, 'name': 'bad.jpg', 'path': missing + '/bad.jpg', 'settings': {}, 'info': {}},
               {'id': 4, 'name': 'd.jpg', 'path': missing + '/d.jpg', 'settings': {'contrast': 3.}, 'info': {}}]
    sources = {'9': {'id': 9, 'name': 's.jpg', 'path': missing + '/s.jpg', 'settings': {'exposure': -1., 'curve': [[0., 0.], [.5, .6], [1., 1.]], 'panels_off': ['curve']}, 'info': {}}}
    C = lambda entries, deadline=None, recs=records: add('photo_edits.compose', entries=entries, records=recs, sources=sources, deadline=deadline)
    C([{'ids': [1, 2, 3, 4], 'add': {'exposure': .5}}, {'ids': [2, 1], 'set': {'contrast': 20}}])
    C([{'ids': [1, 2], 'add': {'negative_density': .5}}, {'ids': [1, 2], 'add': {'exposure': 1}}])
    C([{'ids': [1, 2, 3], 'copy_from': 9}, {'ids': [4], 'copy_from': 9, 'groups': ['curves'], 'set': {'exposure': 2}}])
    C([{'ids': [1, 3], 'set': {'straighten': 5}}, {'ids': [1, 3, 4], 'copy_from': 3}])
    C([{'ids': [1, 4], 'copy_from': 4, 'set': {'contrast': 50}}, {'ids': [1, 4], 'copy_from': 1}])
    C([{'ids': [1, 2, 3], 'preset': 'Soft', 'base': {'contrast': 12, 'exposure': .15}}, {'ids': [2], 'add': {'negative_density': 9}}])
    C([{'ids': [1, 2, 3, 4], 'auto_straighten': True, 'auto_crop': False}, {'ids': [4], 'auto_crop': True, 'crop': None},
       {'ids': [2], 'crop': [.1, .2, .3, .4]}])
    C([{'ids': [1, 2, 99], 'set': {'exposure': 1}}], deadline=3)
    C([{'ids': [1], 'set': {'exposure': 1}}, {'ids': [2], 'set': {'exposure': 1}}], deadline=2)
    C([])
    return cases


def generate_pure():
    scrub = Scrub()
    cases = pure_cases()
    for case in cases:
        a = deepcopy(case['args'])
        outcome = run(lambda: pure_call(case['fn'], a))
        case.update(scrub(outcome))
    write('pure', {'defaults': DEFAULTS, 'cases': cases})
    return len(cases)


# --- scripted catalogue -----------------------------------------------------------------------------

PROFILE_BYTES = b'Generated DCP storage test for the service goldens\x00\xff'
PROFILE = {'name': 'Generated 한글', 'sha256': hashlib.sha256(PROFILE_BYTES).hexdigest(), 'data': base64.b64encode(PROFILE_BYTES).decode()}


class Runner:
    """Executes script operations on the oracle; tools/golden mirrors this in crates/grainy-service/tests."""

    def __init__(self, catalog, root):
        self.c = catalog; self.root = root; self.snapshotted = set(); self.captures = {}

    def records(self, ids):
        records, errors = service.freeze(self.c, ids)
        return records, errors

    def call(self, op, a):
        c = self.c
        if op == 'add':
            return c.add(a[0], full(a[1]), a[2], resolved=True)
        if op == 'virtual_copy':
            return c.virtual_copy(a[0])
        if op == 'update':
            return c.update(a[0], **a[1])
        if op == 'edit':
            return c.edit(a[0], {**c.photo(a[0])['settings'], **a[1]}, a[2])
        if op == 'save_preference':
            return c.save_preference(a[0], a[1])
        if op == 'sql':
            c.db.execute(a[0], a[1]); c.db.commit(); return None
        if op == 'remove_photos':
            return c.remove_photos(a[0])
        if op == 'stacked':
            ids, depth, redo, order, int_first = a
            for ident in ids:
                base = c.photo(ident)['settings']
                undo = [{**base, 'exposure': -i / 100} for i in range(depth)]
                if order and undo:
                    undo[0] = dict(reversed(list(undo[0].items())))
                if int_first and undo:
                    undo[0]['contrast'] = 0
                c.save_preference(f'undo:{ident}', {'undo': undo, 'redo': [{**base, 'contrast': float(i)} for i in range(redo)]})
            return None
        if op == 'undo_state':
            value = c.preference(f'undo:{a[0]}')
            return None if value is None else {k: len(v) if isinstance(v, list) else v for k, v in value.items()}
        if op == 'query_ids':
            where, sort, descending, allow_empty, cancelled = a
            cancel = Event()
            if cancelled:
                cancel.set()
            return service.query_ids(c.directory, where, sort=sort, descending=descending, cancel=cancel, allow_empty=allow_empty)
        if op == 'page_rows':
            return list(service.page_rows(c.directory, a[0]))
        if op == 'folder_rows':
            cancel = Event()
            if a[1]:
                cancel.set()
            return service.folder_rows(c.directory, a[0], cancel)
        if op == 'freeze':
            return list(service.freeze(c, a[0]))
        if op == 'capture':
            current, selected, folder, name = a
            self.captures[name] = command_batch.capture(c, current, selected, folder)
            return self.captures[name]
        if op == 'describe':
            return command_batch.describe(self.captures[a[0]], a[1])
        if op == 'targets':
            return command_batch.targets(self.captures.get(a[0]), a[1])
        if op == 'cb_apply':
            name, scope, change, options, extra = a
            records = command_batch.targets(self.captures[name], scope)
            values = [{**r['settings'], **change} for r in records] + ([{}] if extra else [])
            return command_batch.apply(c, records, values, **options)
        if op == 'cb_duplicate':
            records = command_batch.targets(self.captures[a[0]], a[1])
            return command_batch.apply(c, records + records[:1], [r['settings'] for r in records + records[:1]])
        if op == 'cb_restore':
            redo, key, expect = a
            return command_batch.restore(c, redo, batch_key=key, expect=expect)
        if op == 'batch':
            ids, set_values, add_values, tool, switch, presets = a
            records, errors = service.freeze(c, ids)
            values = [service.compose(r['settings'], [service.parse_changes(set_values, add_values)], r['info'], r['id']) for r in records]
            presets = {int(k): v for k, v in presets.items()} if presets is not None else None
            bid, changed = service.apply_batch(c, records, values, label='AI 보정', tool=tool, snapshot_name='AI 이전',
                                               snapshotted=self.snapshotted, switch=switch, presets=presets)
            return [bid, changed, errors]
        if op == 'batch_values':
            ids, values_set, switch, presets = a
            records, errors = service.freeze(c, ids)
            values = [{**r['settings'], **values_set} for r in records]
            presets = {int(k): v for k, v in presets.items()} if presets is not None else None
            bid, changed = service.apply_batch(c, records, values, label='AI 보정', tool='apply_edits', snapshot_name='AI 이전',
                                               snapshotted=self.snapshotted, switch=switch, presets=presets)
            return [bid, changed, errors]
        if op == 'undo_batch':
            return list(service.undo_batch(c, a[0], a[1]))
        if op == 'batch_index':
            return service.batch_index(c)
        if op == 'last_batch':
            return service.last_batch(c)
        if op == 'snapshotted':
            return sorted(self.snapshotted)
        if op == 'labels':
            return list(photo_labels.apply(c, a[0]))
        if op == 'busy':
            c.db.execute('BEGIN')
            try:
                return self.call(a[0], a[1])
            finally:
                c.db.rollback()
        raise KeyError(op)


def script():
    ops = []

    def add(op, *args):
        ops.append({'op': op, 'args': list(args)})
    P = PLACE + '\\photos\\'
    add('add', P + 'A\\one.jpg', {}, {'width': 600, 'height': 400, 'format': 'JPEG', 'camera': 'Z 6'})
    add('add', P + 'A\\two.jpg', {'exposure': 0.2}, {'width': 600, 'height': 400})
    add('add', P + 'A\\three.jpg', {'exposure': 0.4, 'contrast': 5}, {})
    add('add', P + 'A\\sub\\four.jpg', {}, {'width': 10, 'height': 20})
    add('add', P + 'B\\five.NEF', {'dcp_profile': PROFILE, 'exposure': 0.1}, {'format': 'NEF', 'width': 30, 'height': 20})
    add('add', P + 'A\\movie.MOV', {}, {})
    add('add', P + 'A\\gone.jpg', {}, {})
    add('virtual_copy', 2)
    add('update', 1, {'rating': 4, 'flag': 1, 'label': '빨강', 'keywords': 'roll12, 가족'})
    add('update', 2, {'rating': 2, 'flag': -1})
    add('update', 3, {'rating': 5, 'label': '파랑'})
    A = P + 'A'
    for where, sort, descending, allow_empty, cancelled in [
            ({'folder': A}, None, False, False, False), ({'folder': A, 'recursive': True}, None, False, False, False),
            ({'rating_min': 4}, None, False, False, False), ({'flag': 'picked'}, None, False, False, False),
            ({'flag': 'rejected'}, None, False, False, False), ({'flag': 'unflagged'}, None, False, False, False),
            ({'label': 'red'}, None, False, False, False), ({'text': 'roll12'}, None, False, False, False),
            ({'text': '가족'}, None, False, False, False), ({'text': 'THREE'}, None, False, False, False),
            ({}, None, False, True, False), ({}, 'rating', True, True, False), ({}, 'name', False, True, False),
            ({'rating_max': 2}, 'name', True, False, False), ({'rating_min': 1}, None, False, False, True), ({}, None, False, False, False)]:
        add('query_ids', where, sort, descending, allow_empty, cancelled)
    add('page_rows', [3, 1, 8, 7, 999, 4, 5])
    add('page_rows', [])
    add('folder_rows', None, False)
    add('folder_rows', A, False)
    add('folder_rows', None, True)
    add('freeze', [1, 6, 999, 2])
    add('capture', 1, [2, 1, 2], None, 'one')
    for scope in ('current', 'selection', 'folder'):
        add('describe', 'one', scope)
        add('targets', 'one', scope)
    add('capture', None, [], None, 'none')
    add('targets', 'none', 'current')
    add('targets', 'missing', 'current')
    add('describe', 'none', 'folder')
    add('capture', 4, [999], A, 'four')
    add('describe', 'four', 'folder')
    add('capture', None, [3, 2, 1], None, 'sel')
    # command_batch v1 (full) records
    add('cb_restore', False, command_batch.BATCH_KEY, None)
    add('cb_restore', True, command_batch.BATCH_KEY, None)
    add('cb_apply', 'sel', 'selection', {'monochrome': True}, {}, False)
    add('cb_restore', False, command_batch.BATCH_KEY, None)
    add('cb_restore', False, command_batch.BATCH_KEY, None)
    add('cb_restore', True, command_batch.BATCH_KEY, None)
    add('capture', None, [3, 2, 1], None, 'sel2')
    add('cb_apply', 'sel2', 'selection', {}, {}, False)
    add('cb_apply', 'sel2', 'selection', {'contrast': 7.}, {}, True)
    add('cb_duplicate', 'sel2', 'selection')
    add('cb_restore', False, command_batch.BATCH_KEY, None)
    add('capture', None, [1, 2], None, 'sel3')
    add('edit', 2, {'exposure': 1.0}, '나중 보정')
    add('cb_apply', 'sel3', 'selection', {'monochrome': False}, {}, False)
    # compact record with meta and expect
    add('capture', None, [1, 2, 3], None, 'sel4')
    add('cb_apply', 'sel4', 'selection', {'saturation': 9.}, {'batch_key': 'undo:mcp:b123456', 'meta': {'id': 'b123456', 'tool': 'apply_edits', 'v': 7}, 'compact': True}, False)
    add('cb_restore', False, 'undo:mcp:b123456', 'bffffff')
    add('cb_restore', False, 'undo:mcp:b123456', 'b123456')
    add('cb_restore', True, 'undo:mcp:b123456', 'b123456')
    add('cb_restore', False, 'undo:mcp:b123456', None)
    # v1 with an 80+ stack, then a member's history changes
    add('stacked', [1, 2], 81, 2, True, False)
    add('capture', None, [1, 2], None, 'sel5')
    add('cb_apply', 'sel5', 'selection', {'vibrance': 4.}, {'label': '일괄', 'switch': False}, False)
    add('undo_state', 1)
    add('save_preference', 'undo:2', {'undo': [], 'redo': []})
    add('cb_restore', False, command_batch.BATCH_KEY, None)
    add('cb_restore', False, 'undo:missing-key', None)
    # AI batches: snapshots, undo/redo walk, ids
    add('sql', "DELETE FROM preferences WHERE key LIKE 'undo:%'", [])
    add('batch', [1, 2], None, {'exposure': .5}, 'apply_edits', True, None)
    add('batch', [1], {'contrast': 20}, None, 'auto_tone', True, None)
    add('snapshotted')
    add('batch_index')
    add('last_batch')
    add('undo_batch', None, False)
    add('undo_batch', None, False)
    add('undo_batch', None, False)
    add('undo_batch', None, True)
    add('undo_batch', 'b000001', True)
    add('undo_batch', 'b000002', True)
    add('undo_batch', None, True)
    add('undo_batch', 'b000001', False)
    # depth: the oldest batch and its record go; a new batch drops redo
    for n in range(4):
        add('batch', [3], None, {'exposure': .1}, 'apply_edits', True, None)
    add('batch_index')
    add('undo_batch', None, False)
    add('batch', [3], {'contrast': 4}, None, 'apply_edits', True, None)
    add('batch_index')
    add('undo_batch', None, True)
    # presets remembered with the batch
    add('batch_values', [2, 3, 6, 999], {'exposure': .4}, False, {'2': 'Soft daylight', '3': ''})
    add('undo_batch', None, False)
    add('undo_batch', None, True)
    add('batch_values', [2], {'exposure': .45}, False, {'2': 'Other'})
    add('undo_batch', None, False)
    add('batch_values', [1], {}, True, None)
    # conflicts after later edits, history changes and removal
    add('batch', [1, 2], None, {'exposure': .1}, 'apply_edits', True, None)
    add('edit', 2, {'exposure': 3.0}, '나중 보정')
    add('undo_batch', None, False)
    add('batch', [3], None, {'exposure': .1}, 'apply_edits', True, None)
    add('save_preference', 'undo:3', {'undo': [], 'redo': []})
    add('undo_batch', None, False)
    add('batch', [3, 4], None, {'grain': 5}, 'apply_edits', True, None)
    add('sql', 'UPDATE photos SET path=? WHERE id=?', [P + 'A\\sub\\moved.jpg', 4])
    add('undo_batch', None, False)
    add('sql', 'UPDATE photos SET path=? WHERE id=?', [P + 'A\\sub\\four.jpg', 4])
    add('undo_batch', None, False)
    add('undo_batch', None, True)
    add('batch', [1, 4], None, {'grain': 5}, 'apply_edits', True, None)
    add('remove_photos', [4])
    add('undo_batch', None, False)
    # a failing write rolls everything back
    add('sql', "CREATE TRIGGER fail_batch BEFORE UPDATE OF edits ON photos WHEN NEW.id=3 BEGIN SELECT RAISE(ABORT,'injected'); END;", [])
    add('batch', [1, 3], {'contrast': 5}, None, 'apply_edits', True, None)
    add('sql', 'DROP TRIGGER fail_batch', [])
    # camera profiles stay references
    add('batch', [5, 8], None, {'exposure': .3}, 'auto_color', True, None)
    add('undo_batch', None, False)
    add('undo_batch', None, True)
    # compact records with long histories (head entries, other key order, int values, text not in dumps form)
    add('stacked', [1, 2], 85, 2, True, False)
    add('stacked', [3], 79, 0, False, False)
    add('stacked', [8], 80, 1, False, True)
    add('batch', [1, 2, 3, 5], {'clarity': 11}, None, 'apply_edits', True, None)
    add('undo_state', 1)
    add('undo_state', 5)
    add('undo_batch', None, False)
    add('undo_state', 1)
    add('undo_batch', None, True)
    add('undo_batch', None, False)
    add('undo_batch', None, True)
    add('batch', [8], {'clarity': 11}, None, 'apply_edits', True, None)
    add('undo_batch', None, False)
    add('stacked', [1], 80, 0, False, True)
    add('batch', [1], {'clarity': 12}, None, 'apply_edits', True, None)
    add('undo_batch', None, False)
    add('sql', 'INSERT OR REPLACE INTO preferences VALUES(?,?)', ['undo:2', '{"undo":[],"redo":[]}'])
    add('batch', [2], {'clarity': 13}, None, 'apply_edits', True, None)
    add('undo_batch', None, False)
    add('sql', 'INSERT OR REPLACE INTO preferences VALUES(?,?)', ['undo:3', '{"redo": [], "undo": [], "note": "kept"}'])
    add('batch', [3], {'clarity': 14}, None, 'apply_edits', True, None)
    add('undo_batch', None, False)
    add('undo_batch', None, True)
    # label batches
    add('labels', [{'ids': [1, 2, 999], 'fields': {'rating': 3}}, {'ids': [2], 'fields': {'label': '초록', 'flag': 1}}])
    add('labels', [{'ids': [1], 'fields': {'rating': 3}}])
    add('undo_batch', None, False)
    add('undo_batch', None, True)
    add('update', 1, {'rating': 1})
    add('undo_batch', None, False)
    add('labels', [{'ids': [3], 'fields': {'rating': 0, 'label': None}}])
    add('edit', 3, {'exposure': 2.5}, '나중 보정')
    add('undo_batch', None, False)
    add('labels', [{'ids': [8], 'fields': {'flag': -1}}])
    add('batch_index')
    # nothing starts while another write is open
    add('busy', 'batch', [[1], None, {'exposure': .1}, 'apply_edits', True, None])
    add('busy', 'undo_batch', [None, False])
    add('busy', 'labels', [[{'ids': [1], 'fields': {'rating': 5}}]])
    add('busy', 'cb_apply', ['sel4', 'selection', {'saturation': 1.}, {}, False])
    add('busy', 'cb_restore', [False, command_batch.BATCH_KEY, None])
    add('batch_index')
    add('snapshotted')
    return ops


TOKENS = ['000001', '000002', '000001', '000003', '000004', '000005', '000006', '000007'] + [f'{n:06x}' for n in range(0x10, 0x60)]
FILES = ['photos/A/one.jpg', 'photos/A/two.jpg', 'photos/A/three.jpg', 'photos/A/sub/four.jpg', 'photos/B/five.NEF', 'photos/A/movie.MOV']


def generate_script():
    temporary = Path(tempfile.mkdtemp(prefix='grainy-service-')).resolve()
    try:
        for name in FILES:
            path = temporary / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b'generated ' + name.encode())
        scrub = Scrub(temporary)
        tokens = Tokens(TOKENS)
        with patch.object(catalog_module, 'datetime', Clock), patch.object(command_batch, 'datetime', Clock), \
                patch.object(service, 'datetime', Clock), patch.object(photo_labels, 'datetime', Clock), \
                patch.object(library_query, 'datetime', Clock), patch.object(service, 'secrets', tokens), \
                patch.object(photo_labels, 'secrets', tokens):
            catalog = Catalog(temporary / 'data')
            runner = Runner(catalog, temporary)
            steps = script()
            previous = dump(catalog.db, scrub)
            initial = previous
            for step in steps:
                args = scrub.unplace(deepcopy(step['args']), temporary)
                outcome = run(lambda: runner.call(step['op'], args))
                step.update(scrub(outcome))
                current = dump(catalog.db, scrub)
                step['delta'] = {k: v for k, v in current.items() if v != previous.get(k)}
                previous = current
            catalog.close()
        write('script', {'stamp': STAMP, 'tokens': TOKENS, 'tokens_used': tokens.used, 'files': FILES, 'initial': initial,
                         'steps': steps, 'final': previous})
        return len(steps)
    finally:
        shutil.rmtree(temporary, ignore_errors=True)


# --- automatic values ------------------------------------------------------------------------------

def generate_auto():
    temporary = Path(tempfile.mkdtemp(prefix='grainy-service-auto-')).resolve()
    try:
        names = ['a.png', 'gone.png', 'fail.png', 'changes.png', 'b.png']
        for name in names:
            if name != 'gone.png':
                (temporary / name).write_bytes(b'pixels of ' + name.encode())
        records = [{'id': n + 1, 'name': name, 'path': str(temporary / name),
                    'settings': {'working_space': 'prophoto', 'exposure': [0.0, 0.5, 1.0, 2.0, 0.25][n]}} for n, name in enumerate(names)]
        scrub = Scrub(temporary)
        calls = []

        def loader(path, size, space, raw_options=None):
            path = Path(path); calls.append(['load', path.name, size, space])
            if path.name == 'fail.png':
                raise ValueError('cannot decode')
            if path.name == 'changes.png':
                with open(path, 'ab') as f:
                    f.write(b'+')
            return np.full((2, 2, 3), raw_options['exposure'], np.float32), {}

        def offline(directory, ident, settings):
            calls.append(['offline', ident])
            raise FileNotFoundError('no packet')

        def compute(source, settings):
            calls.append(['compute', float(source.mean())])
            return {'mean': float(source.mean()), 'ws': settings['working_space']}
        runs = []
        for strict, subset, cancel_at in [(False, None, None), (True, [0, 1], None), (True, [0, 2], None), (True, [3], None),
                                          (False, None, 2), (False, None, 0), (True, [0, 4], None)]:
            calls.clear(); seen = []
            cancel = Event()
            if cancel_at == 0:
                cancel.set()

            def progress(n):
                seen.append(n)
                if cancel_at is not None and n >= cancel_at:
                    cancel.set()
            chosen = records if subset is None else [records[i] for i in subset]
            with patch.object(preview_store, 'load_offline', offline):
                outcome = run(lambda: list(service.auto_values(chosen, compute, str(temporary), cancel=cancel, progress=progress,
                                                               strict=strict, loader=loader)))
            runs.append({'strict': strict, 'subset': subset, 'cancel_at': cancel_at, **scrub(outcome), 'progress': seen, 'calls': scrub(calls)})
        reasons = [{'error': kind, 'path': name, 'reason': service.reason(error, str(temporary / name))}
                   for kind, error in (('ValueError', ValueError('x')), ('OriginalChanged', service.OriginalChanged('x')))
                   for name in ('a.png', 'gone.png')]
        write('auto', {'names': names, 'records': scrub(records), 'runs': runs, 'reasons': reasons})
        return len(runs)
    finally:
        shutil.rmtree(temporary, ignore_errors=True)


# --- export -------------------------------------------------------------------------------------------

EXPORT_FILES = ['out/roll_grainy.jpg', 'out/ROLL_GRAINY_2.JPG', 'src/A/roll.png', 'src/B/roll.png', 'src/C/bad.png', 'src/D/locked.png',
                'src/E/raw.NEF', 'imp/a.jpg', 'imp/b.PNG', 'imp/notes.txt', 'imp/sub/c.tif', 'imp/sub/deep/d.nef', 'imp/folder.jpg/x.txt',
                'orig/raw.xmp', 'library/x.txt']


def export_records_list(portra):
    def rec(ident, path, name=None, **settings):
        return {'id': ident, 'name': name or Path(path).name, 'path': PLACE + '\\' + path.replace('/', '\\'), 'settings': settings,
                'info': {'width': 100 + ident, 'height': 50 + ident, 'camera': 'Cam/1'}, 'user_metadata': {'_preset': 'Soft'} if ident == 2 else {},
                'keywords': 'k' + str(ident)}
    return [rec(1, 'src/A/roll.png'), rec(2, 'src/B/roll.png', exposure=.4, film_look=portra), rec(3, 'src/C/bad.png'),
            rec(4, 'src/D/locked.png'), rec(5, 'src/E/raw.NEF', 'raw.NEF · 사본')]


class Exporter:
    def __init__(self):
        self.calls = []

    def export_image(self, path, destination, settings, format, quality, longest, color_space, keep_metadata, user_metadata, keywords,
                     grayscale=True, dither=True):
        self.calls.append([Path(destination).name, format, quality, list(longest) if isinstance(longest, tuple) else longest, color_space,
                           keep_metadata, user_metadata, keywords, grayscale, dither, settings.get('exposure')])
        name = Path(path).name
        if name == 'bad.png':
            raise ValueError('bad photo')
        if name == 'locked.png':
            raise PermissionError(13, 'locked')
        with open(destination, 'xb') as f:
            f.write(b'x' * (10 + len(name)))
        return (100 + len(name), 50)

    def copy_original(self, record, destination):
        self.calls.append(['original', Path(destination).name])
        with open(destination, 'xb') as f:
            f.write(b'o' * 7)
        with open(Path(destination).with_suffix('.xmp'), 'xb') as f:
            f.write(b'xmp')
        return (record['info']['width'], record['info']['height'])


def generate_export():
    temporary = Path(tempfile.mkdtemp(prefix='grainy-service-export-')).resolve()
    try:
        for name in EXPORT_FILES:
            path = temporary / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b'taken')
        scrub = Scrub(temporary)
        portra = service.parse_changes({'film_look': 'Kodak Portra 400'})[0]['film_look']
        own = [PLACE + '\\library', PLACE + '\\mcp']
        steps = []

        def add(fn, **a):
            steps.append({'fn': fn, 'args': a})
        add('export_folder', root=PLACE + '\\Pictures\\Grainy', subfolder='Roll 12', own=own, create=True)
        add('export_folder', root=PLACE + '\\root', subfolder=None, own=own, create=True)
        add('export_folder', root=PLACE + '\\later', subfolder='x', own=own, create=False)
        for bad in ('..', '.', 'a/b', 'a\\b', 'CON', 'con.txt', 'LPT1', 'x:', 'end.', 'end ', '', ' ' * 3, 'tab\there', 'a' * 81, 'q?', '<x>', 5):
            add('export_folder', root=PLACE, subfolder=bad, own=own, create=True)
        add('export_folder', root=PLACE, subfolder='library', own=own, create=True)
        add('export_folder', root=PLACE + '\\library', subfolder=None, own=own, create=True)
        add('export_folder', root=PLACE + '\\out\\roll_grainy.jpg', subfolder=None, own=own, create=True)
        for text in (PLACE + '\\out\\new', PLACE + '\\out\\..\\imp', 'relative\\folder', '.', '', '   ', '\\no drive', PLACE + '\\library',
                     PLACE + '\\library\\thumbnails', PLACE + '\\mcp\\..\\library\\x', '  ' + PLACE + '\\out  ', PLACE + '\\a\x00b', 7):
            add('named_folder', text=text, own=own, what='path')
        add('image_files', folder=PLACE + '\\imp', recursive=False, cancel=False, limit=10, own=own)
        add('image_files', folder=PLACE + '\\imp', recursive=True, cancel=False, limit=10, own=own)
        add('image_files', folder=PLACE + '\\imp', recursive=True, cancel=False, limit=3, own=own)
        add('image_files', folder=PLACE + '\\imp', recursive=True, cancel=True, limit=10, own=own)
        add('image_files', folder=PLACE + '\\imp', recursive=True, cancel=False, limit=10, own=own + [PLACE + '\\imp\\sub\\deep'])
        add('image_files', folder=PLACE + '\\none', recursive=False, cancel=False, limit=10, own=own)
        records = export_records_list(portra)
        for fmt, naming in (('JPEG', '{stem}_grainy'), ('PNG', 'R12_{n:03d}'), ('Original', '{stem}'), ('JPEG', '{stem}_{film}_{preset}'),
                            ('TIFF 16-bit', 'roll_grainy'), ('Bogus', '{stem}')):
            add('export_plans', records=records, folder=PLACE + '\\out', format=fmt, naming=naming)
        add('export_plans', records=records[:2], folder=PLACE + '\\orig', format='Original', naming='raw')
        add('export_plans', records=records[:1], folder=PLACE + '\\missing', format='JPEG', naming='{stem}')
        export = lambda folder, options, cancel_at=None, english=False, subset=None: add(
            'export_records', records=records if subset is None else [records[i] for i in subset], folder=folder, options=options,
            cancel_at=cancel_at, english=english)
        export(PLACE + '\\out', {'format': 'JPEG', 'quality': 80, 'longest': 0}, english=True)
        export(PLACE + '\\out', {'format': 'PNG', 'naming': 'R12_{n:03d}', 'longest': [48, 40, 'inside'], 'keep_metadata': True,
                                 'color_space': 'Display P3', 'grayscale': False, 'dither': False}, english=False)
        export(PLACE + '\\out', {'format': 'JPEG', 'naming': 'cancel_{n}'}, cancel_at=2, subset=[0, 1, 4])
        export(PLACE + '\\out', {'format': 'JPEG', 'naming': 'none_{n}'}, cancel_at=0, subset=[0, 1])
        export(PLACE + '\\orig', {'format': 'Original', 'naming': 'raw'}, subset=[4, 0])
        export(PLACE + '\\out', {'format': 'JPEG', 'longest': 2000, 'naming': '{stem}_{camera}'}, english=True, subset=[0])
        outputs = [{'photo_id': i, 'path': f'{PLACE}\\out\\f{i}.jpg', 'size': [i, i + 1], 'bytes': i * 10} for i in range(5)]
        add('output_files', outputs=outputs, limit=3)
        add('output_files', outputs=outputs, limit=100)
        add('output_files', outputs=[], limit=100)

        def call(fn, a):
            if fn == 'export_folder':
                return service.export_folder(a['root'], a['subfolder'], a['own'], create=a['create'])
            if fn == 'named_folder':
                return service.named_folder(a['text'], a['own'], a['what'])
            if fn == 'image_files':
                cancel = Event()
                if a['cancel']:
                    cancel.set()
                return service.image_files(a['folder'], a['recursive'], cancel, a['limit'], own=a['own'])
            if fn == 'export_plans':
                return [[r['id'], p] for r, p in service.export_plans(a['records'], a['folder'], a['format'], a['naming'])]
            if fn == 'export_records':
                exporter = Exporter(); cancel = Event(); seen = []
                if a['cancel_at'] == 0:
                    cancel.set()

                def progress(n):
                    seen.append(n)
                    if a['cancel_at'] is not None and n >= a['cancel_at']:
                        cancel.set()
                options = dict(a['options'])
                if isinstance(options.get('longest'), list):
                    options['longest'] = tuple(options['longest'])
                with patch.object(quick_export, 'export_concurrency', lambda records: 1), \
                        patch.object(quick_export, 'copy_original', exporter.copy_original):
                    result = service.export_records(a['records'], a['folder'], cancel=cancel, progress=progress, exporter=exporter.export_image,
                                                    english=a['english'], **options)
                return {'result': result, 'progress': seen, 'calls': exporter.calls,
                        'listing': sorted(p.name for p in Path(a['folder']).iterdir())}
            if fn == 'output_files':
                return service.output_files(a['outputs'], a['limit'])
            raise KeyError(fn)
        for step in steps:
            a = scrub.unplace(deepcopy(step['args']), temporary)
            step.update(scrub(run(lambda: call(step['fn'], a))))
        write('export', {'files': EXPORT_FILES, 'steps': steps})
        return len(steps)
    finally:
        shutil.rmtree(temporary, ignore_errors=True)


if __name__ == '__main__':
    print('pure', generate_pure())
    print('script', generate_script())
    print('auto', generate_auto())
    print('export', generate_export())
