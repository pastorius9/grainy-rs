"""Goldens of the RAW package of grainy-app (APP_PLAN package 20): `golden/app/raw*/`.

Everything runs the oracle's own code (nothing is reimplemented):

* `raw_profiles`: luma/profile_library.py `profile_roots`, `scan` (three scans of a tree of DCP files: first, cached,
  after a change and a removal; a capped and a cancelled scan), `compatible`, `load_selected`; and
  luma/profile_dialog.py `ProfileDialog` driven headless on a stand-in window (its real widgets are read back).
* `raw_defaults`: `RawDefaultsDialog` on a stand-in window over a real `Catalog` (scopes, sources, groups, ISO
  anchors and their refusals, toggling, removal, applying to the open photo).
* `raw_studio`: luma/studio.py `change_working`, `apply_dcp_record`, `clear_dcp`, `change_raw_mode`,
  `raw_temperature_changed`, `display_raw_white` and `camera_profile` on a stand-in window, with the camera
  calibration of a generated DNG (`golden/rawload/_files/triple.dng`).

The DCP files are the ones in `golden/raw/_files/` (the test reads the same files); the temporary folders are
written as `<T>` and every file time is fixed, so the output is deterministic. The environment variables that
`profile_roots` reads are pointed at the temporary folder before anything of the oracle is imported, so no folder
of this computer is read.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_raw.py
"""
import os
import shutil
import tempfile
from pathlib import Path

TEMP = Path(tempfile.mkdtemp(prefix='grainy-raw-golden-')).resolve()
ENVIRONMENT = {'PROGRAMDATA': TEMP / 'programdata', 'APPDATA': TEMP / 'appdata', 'LOCALAPPDATA': TEMP / 'localappdata'}
for _key, _value in ENVIRONMENT.items():
    os.environ[_key] = str(_value)

from common import GOLDEN, ORACLE, write_json  # noqa: E402
from datetime import datetime  # noqa: E402
from threading import Event  # noqa: E402
import sqlite3  # noqa: E402
import json  # noqa: E402

from luma import catalog as catalog_module  # noqa: E402
from luma import dcp, profile_library, profile_dialog, raw_defaults, studio  # noqa: E402
from luma.catalog import Catalog  # noqa: E402
from luma.engine import PRESETS, defaults, load_image, normalized  # noqa: E402
from luma.studio import StudioTools  # noqa: E402
from PySide6.QtWidgets import QApplication, QMessageBox, QWidget  # noqa: E402

_qt = QApplication.instance() or QApplication([])

STAMP = '2026-11-13T01:02:03.456789+00:00'
TIME = 1_700_000_000


class Clock(datetime):
    @classmethod
    def now(cls, tz=None):
        value = datetime.fromisoformat(STAMP)
        return value if tz else value.replace(tzinfo=None)


catalog_module.datetime = Clock

RAW_FILES = GOLDEN / 'raw' / '_files'
DNG = GOLDEN / 'rawload' / '_files' / 'triple.dng'


def fixture(name):
    return RAW_FILES / name / 'profile.dcp'


def text(value):
    """Paths below the temporary folder written as <T> (also lower-cased index keys)."""
    if isinstance(value, str):
        value = value.replace(str(TEMP), '<T>').replace(os.path.normcase(str(TEMP)), '<t>')
        return value
    if isinstance(value, list):
        return [text(v) for v in value]
    if isinstance(value, dict):
        return {k: text(v) for k, v in value.items()}
    return value


def place(files):
    """Copies (destination below TEMP, source below golden) and fixes every file's time."""
    for destination, source in files:
        target = TEMP / destination
        target.parent.mkdir(parents=True, exist_ok=True)
        if source is None:
            target.write_bytes(b'not a profile')
        else:
            shutil.copyfile(GOLDEN / source, target)
        os.utime(target, ns=(TIME * 10**9, TIME * 10**9))


class Warnings:
    def __init__(self):
        self.log = []

    def __call__(self, parent, title, message, *args):
        self.log.append([title, message])


WARNINGS = Warnings()
QMessageBox.warning = staticmethod(WARNINGS)


def slim(value):
    """Pinned DCP records (their bytes) written as their name, camera and digest."""
    if isinstance(value, dict):
        return {k: (dcp_summary(v) if k == 'dcp_profile' and isinstance(v, dict) else slim(v)) for k, v in value.items()}
    if isinstance(value, list):
        return [slim(v) for v in value]
    return value


def dcp_summary(value):
    if not value:
        return None
    return {'name': value['name'], 'camera': value['camera'], 'sha256': value['sha256']}


# ---- raw_profiles ------------------------------------------------------------------------------------------------

PROFILE_FILES = [
    ['lib/camera-profiles/Luma Test.dcp', 'raw/_files/profile_test-camera/profile.dcp'],
    ['lib/camera-profiles/dual.dcp', 'raw/_files/profile_synthetic_dual/profile.dcp'],
    ['lib/camera-profiles/generic.dcp', 'raw/_files/profile_synthetic_custom/profile.dcp'],
    ['lib/camera-profiles/hdr.dcp', 'raw/_files/invalid_hdr/profile.dcp'],
    ['lib/camera-profiles/notes.txt', None],
    ['lib/camera-profiles/sub/copy.DCP', 'raw/_files/profile_test-camera/profile.dcp'],
    ['programdata/Adobe/CameraRaw/CameraProfiles/triple.dcp', 'raw/_files/profile_test-triple-illuminant/profile.dcp'],
    ['localappdata/Luma/CameraProfiles/forward.dcp', 'raw/_files/profile_synthetic_forward/profile.dcp'],
    ['extra/synth triple.dcp', 'raw/_files/profile_synthetic_triple/profile.dcp'],
    ['extra/single.dcp', 'raw/_files/profile_synthetic_single/profile.dcp'],
    ['extra/short.dcp', 'raw/_files/invalid_short/profile.dcp'],
]
CHANGED = ['extra/single.dcp', 'raw/_files/profile_synthetic_big_endian/profile.dcp']
REMOVED = 'lib/camera-profiles/dual.dcp'

METADATA = {
    'test': {'make': 'Test', 'camera': 'Test Camera', 'format': 'DNG'},
    'synthetic': {'make': 'Grainy', 'camera': 'Synthetic', 'format': 'DNG'},
    'other': {'make': 'Other', 'camera': 'Nothing', 'format': 'NEF'},
    'unique': {'make': 'X', 'camera': 'Y', 'unique_camera': 'Test Camera'},
}


def rows_json(rows):
    return [text({**r, 'error': bool(r['error'])}) for r in rows]


def scan_json(result):
    return {'files': result['files'], 'parsed': result['parsed'], 'errors': len(result['errors']),
            'cancelled': result['cancelled'], 'truncated': result['truncated'], 'profiles': rows_json(result['profiles'])}


def index_json(cache):
    with sqlite3.connect(cache) as db:
        rows = db.execute('SELECT path,stamp,summary FROM profiles ORDER BY path').fetchall()
    out = []
    for path, stamp, summary in rows:
        value = json.loads(summary)
        value['error'] = bool(value['error'])
        out.append({'path': text(path), 'stamp': stamp.split(':')[0] + ':' + stamp.split(':')[2], 'summary': text(value)})
    return out


class ProfileWindow(QWidget):
    def __init__(self, catalog):
        super().__init__()
        self.catalog = catalog

    def spawn(self, work, ready, failed):
        try:
            result = work()
        except Exception as error:  # noqa: BLE001 - the dialog's own failure path
            failed(error)
            return
        ready(result)


def dialog_state(d):
    items = [d.list.item(i).text() for i in range(d.list.count())]
    return {'items': items, 'current': d.list.currentRow(), 'detail': text(d.detail.text()), 'status': d.status.text(),
            'apply': d.apply_button.isEnabled(), 'favorite': d.favorite_button.isEnabled(),
            'favorites': sorted(d.favorites)}


def raw_profiles():
    place(PROFILE_FILES)
    lib = TEMP / 'lib'
    extra = str(TEMP / 'extra')
    roots = profile_library.profile_roots(lib, [extra])
    cache = lib / 'cache' / 'dcp-index.sqlite'
    out = {'files': PROFILE_FILES, 'changed': CHANGED, 'removed': REMOVED, 'time': TIME,
           'environment': {k: text(str(v)) for k, v in ENVIRONMENT.items()}, 'extra': text(extra),
           'roots': text(roots), 'metadata': METADATA}
    scans = []
    first = profile_library.scan(roots, cache)
    scans.append(scan_json(first))
    scans.append(scan_json(profile_library.scan(roots, cache)))
    place([CHANGED])
    os.utime(TEMP / CHANGED[0], ns=((TIME + 100) * 10**9, (TIME + 100) * 10**9))
    (TEMP / REMOVED).unlink()
    third = profile_library.scan(roots, cache)
    scans.append(scan_json(third))
    out['scans'] = scans
    out['index'] = index_json(cache)
    capped_cache = TEMP / 'capped.sqlite'
    out['capped'] = scan_json(profile_library.scan(roots, capped_cache, limit=3))
    out['capped_index'] = index_json(capped_cache)
    cancel = Event()
    cancel.set()
    out['cancelled'] = scan_json(profile_library.scan(roots, TEMP / 'cancelled.sqlite', cancel))
    rows = third['profiles']
    favorites = sorted({r['sha256'] for r in rows if r['name'] == 'Synthetic triple'})
    compatible = []
    for meta in METADATA:
        for search in ['', 'luma', 'LUMA  test', 'xyz', 'synthetic dual', 'Camera']:
            for only in [False, True]:
                found = profile_library.compatible(rows, METADATA[meta], search, favorites, only)
                compatible.append({'metadata': meta, 'search': search, 'only': only,
                                   'names': [text(r['path']) for r in found]})
    out['favorites'] = favorites
    out['compatible'] = compatible
    loads = []
    for row in rows:
        for meta in ['test', 'synthetic']:
            try:
                record = profile_library.load_selected(row, METADATA[meta])
                loads.append({'path': text(row['path']), 'metadata': meta, 'record': dcp_summary(record)})
            except (ValueError, OSError) as error:
                loads.append({'path': text(row['path']), 'metadata': meta, 'error': str(error)})
    out['loads'] = loads
    # A profile changed after the scan is refused at application time.
    stale = next(r for r in rows if r['name'] == 'Synthetic triple')
    place([['extra/synth triple.dcp', 'raw/_files/profile_synthetic_dual/profile.dcp']])
    try:
        profile_library.load_selected(stale, METADATA['synthetic'])
        out['stale'] = None
    except ValueError as error:
        out['stale'] = str(error)
    place([['extra/synth triple.dcp', 'raw/_files/profile_synthetic_triple/profile.dcp']])

    # The dialog, on the last tree, with its own catalogue's preferences.
    catalog = Catalog(TEMP / 'dialog-lib')
    catalog.save_preference('dcp_folders', [extra])
    window = ProfileWindow(catalog)
    original_roots = profile_library.profile_roots
    profile_dialog.profile_library.profile_roots = lambda directory, folders: original_roots(lib, folders)
    states = []
    dialog = profile_dialog.ProfileDialog(window, METADATA['test'])
    states.append(['open', dialog_state(dialog)])
    dialog.refresh()
    states.append(['refresh', dialog_state(dialog)])
    dialog.list.setCurrentRow(1)
    states.append(['select 1', dialog_state(dialog)])
    dialog.toggle_favorite()
    states.append(['favorite', dialog_state(dialog)])
    dialog.search.setText('luma')
    states.append(['search luma', dialog_state(dialog)])
    dialog.search.setText('')
    dialog.favorites_only.setChecked(True)
    states.append(['favorites only', dialog_state(dialog)])
    dialog.favorites_only.setChecked(False)
    dialog.list.setCurrentRow(0)
    dialog.toggle_favorite()
    states.append(['unfavorite', dialog_state(dialog)])
    dialog.search.setText('zzz')
    states.append(['search zzz', dialog_state(dialog)])
    dialog.search.setText('')
    dialog.list.setCurrentRow(dialog.list.count() - 1)
    states.append(['select last', dialog_state(dialog)])
    dialog.choose()
    states.append(['choose disabled', {'record': dcp_summary(dialog.record), 'warnings': list(WARNINGS.log)}])
    dialog.list.setCurrentRow(2)
    dialog.choose()
    states.append(['choose', {'record': dcp_summary(dialog.record), 'warnings': list(WARNINGS.log)}])
    WARNINGS.log.clear()
    out['dialog_favorites'] = catalog.preference('dcp_favorites', [])
    dialog.done(0)
    dialog = profile_dialog.ProfileDialog(window, METADATA['synthetic'])
    dialog.refresh()
    states.append(['synthetic', dialog_state(dialog)])
    dialog.list.setCurrentRow(0)
    states.append(['synthetic select 0', dialog_state(dialog)])
    profile_dialog.QInputDialog.getItem = staticmethod(lambda *args: (extra, True))
    dialog.remove_folder()
    states.append(['remove folder', dialog_state(dialog)])
    out['folders_after_remove'] = text(catalog.preference('dcp_folders', []))
    dialog.remove_folder()
    states.append(['remove folder again', dialog_state(dialog)])
    profile_dialog.QFileDialog.getExistingDirectory = staticmethod(lambda *args: extra)
    dialog.add_folder()
    dialog.add_folder()
    states.append(['add folder', dialog_state(dialog)])
    out['folders_after_add'] = text(catalog.preference('dcp_folders', []))
    dialog.choose()
    states.append(['synthetic choose', {'record': dcp_summary(dialog.record), 'warnings': list(WARNINGS.log)}])
    WARNINGS.log.clear()
    dialog.done(0)
    catalog.db.close()
    out['dialog'] = states
    profile_dialog.profile_library.profile_roots = original_roots
    write_json(GOLDEN / 'app' / 'raw_profiles' / 'case.json', out)


# ---- raw_defaults ------------------------------------------------------------------------------------------------

ROOT = 'Z:\\GrainyGolden\\raw'
DETAIL = {'noise_luma': 10, 'noise_color': 25, 'sharpen': 40, 'sharpen_radius': 1, 'sharpen_detail': 25,
          'sharpen_mask': 0, 'clarity': 5, 'texture': 0, 'dehaze': 0, 'blacks': -10}
PHOTOS = [
    ('a.ARW', {'make': 'SONY', 'camera': 'ILCE-7M3', 'serial': '1234', 'iso': 100, 'format': 'ARW'},
     {'exposure': 0.5, 'noise_luma': 10, 'raw_mode': 'sample', 'raw_neutral': [0.5, 1, 0.7], 'grain': 20}),
    ('b.ARW', {'make': 'SONY', 'camera': 'ILCE-7M3', 'serial': '1234', 'iso': 3200, 'format': 'ARW'},
     {'noise_luma': 40, 'sharpen': 20, 'blacks': -20}),
    ('c.ARW', {'make': 'SONY', 'camera': 'ILCE-7M3', 'serial': '9999', 'iso': '800', 'format': 'ARW'},
     {'noise_luma': 25, 'clarity': 12}),
    ('d.NEF', {'make': 'NIKON CORPORATION', 'camera': 'NIKON Z 6', 'iso': 400, 'format': 'NEF'},
     {'exposure': -0.3, 'raw_mode': 'daylight'}),
    ('e.jpg', {'make': 'SONY', 'camera': 'ILCE-7M3', 'format': 'JPEG'}, {'exposure': 0.2}),
    ('f.DNG', {'make': 'Test', 'camera': 'Test Camera', 'serial': ' 77\x00', 'iso': 200, 'format': 'DNG'},
     {'contrast': 10}),
    ('g.ARW', {'make': 'SONY', 'camera': 'ILCE-7M3', 'serial': '1234', 'iso': 100, 'format': 'ARW'},
     {'noise_luma': 33}),
]
USER_PRESET = {'exposure': 0.25, 'vibrance': 15, 'noise_luma': 8, 'temperature': 5}


def photo_settings(values):
    return normalized({**DETAIL, **values})


class Commits:
    pass


class DefaultsWindow(QWidget):
    def __init__(self, catalog):
        super().__init__()
        self.catalog = catalog
        self.current_id = None
        self.settings = None
        self.last_saved = None
        self.selection = []
        self.commits = []
        self.source = object()
        self.render_version = 0
        self.all_presets = {**{name: normalized({'tone_version': defaults()['tone_version'], **values})
                               for name, values in PRESETS.items()}, **catalog.presets()}
        self.manager = self

    def open(self, ident, live=None, selection=None):
        self.current_id = ident
        self.settings = self.catalog.photo(ident)['settings'] if ident else None
        self.last_saved = dict(self.settings) if ident else None
        if live:
            self.settings = {**self.settings, **live}
        self.selection = selection or []

    def selected_ids(self):
        return list(self.selection) or ([self.current_id] if self.current_id else [])

    def commit(self, label='보정 변경'):
        if self.current_id and self.settings != self.last_saved:
            self.catalog.edit(self.current_id, self.settings, label)
            self.commits.append([label, dict(self.settings)])
            self.last_saved = dict(self.settings)

    def load_controls(self):
        pass

    def render(self):
        pass

    def refresh_thumbnails(self, ids):
        pass


def defaults_state(d, w, kind):
    return slim({'rules': d.rules, 'stored_same': w.catalog.preference(raw_defaults.PREFERENCE, []) == d.rules,
            'items': [d.list.item(i).text() for i in range(d.list.count())],
            'scopes': [[d.scope.itemText(i), d.scope.itemData(i)] for i in range(d.scope.count())],
            'scope': d.scope.currentIndex(),
            'sources': [[d.source.itemText(i), d.source.itemData(i)] for i in range(d.source.count())],
            'source': d.source.currentIndex(),
            'groups': {k: c.isChecked() for k, c in d.groups.items()},
            'status': d.status.text(), 'apply': d.apply_button.isEnabled(),
            'warnings': list(WARNINGS.log), 'commits': [label for label, _ in w.commits],
            'settings': w.settings if kind == 'apply' else None})


# The script: each step names an action of the box and its values (the test runs the same list).
DEFAULTS_SCRIPT = [
    ['open', 1, {'raw_mode': 'sample', 'exposure': 0.75}, [1]],
    ['save'],
    ['set', {'scope': 'serial', 'name': '  바디 규칙 ', 'groups': {'빛': True}}],
    ['save'],
    ['set', {'scope': 'master', 'source': 'Mine', 'name': ''}],
    ['save'],
    ['set', {'scope': 'model', 'source': None, 'iso': True}],
    ['select', [1, 2, 3]],
    ['save'],
    ['set', {'scope': 'serial'}],
    ['save'],
    ['set', {'scope': 'model'}],
    ['select', [1]],
    ['save'],
    ['select', [1, 4]],
    ['save'],
    ['select', [1, 7]],
    ['save'],
    ['select', list(range(1, 8)) * 10],
    ['save'],
    ['set', {'iso': False, 'groups': {'RAW 프로파일': False, '화이트 밸런스': False, '빛': False, '디테일': False}}],
    ['save'],
    ['row', 0],
    ['toggle'],
    ['row', 1],
    ['remove'],
    ['row', None],
    ['toggle'],
    ['apply'],
    ['open', 3, None, [3]],
    ['apply'],
    ['open', 2, None, [2]],
    ['apply'],
    ['open', 4, {'noise_luma': 15}, [4]],
    ['save'],
    ['set', {'scope': 'master', 'source': 'Soft daylight'}],
    ['save'],
    ['open', 5, None, [5]],
    ['apply'],
    ['save'],
    ['open', 6, {'dcp_profile': 'test-camera'}, [6]],
    ['save'],
    ['open', 1, {'dcp_profile': 'test-camera'}, [1]],
    ['save'],
    ['set', {'scope': 'master'}],
    ['save'],
    ['open', 6, None, [6]],
    ['apply'],
    ['open', None, None, []],
    ['save'],
]


def dcp_record(name):
    return dcp.load_profile(fixture('profile_' + name))


def raw_defaults_case():
    folder = TEMP / 'defaults-lib'
    catalog = Catalog(folder)
    for name, info, values in PHOTOS:
        ident = catalog.add(ROOT + '\\' + name, photo_settings(values), info, resolved=True)
        assert ident
    catalog.save_preset('Mine', normalized(USER_PRESET))
    window = DefaultsWindow(catalog)
    dialog = None
    steps = []
    last_rules = None
    for step in DEFAULTS_SCRIPT:
        WARNINGS.log.clear()
        kind = step[0]
        if kind == 'open':
            live = dict(step[2] or {})
            if live.get('dcp_profile'):
                live['dcp_profile'] = dcp_record(live['dcp_profile'])
            window.open(step[1], live, step[3])
            window.commits = []
            dialog = profile_dialog.RawDefaultsDialog(window)
        elif kind == 'set':
            values = step[1]
            if 'scope' in values:
                dialog.scope.setCurrentIndex(dialog.scope.findData(values['scope']))
            if 'source' in values:
                source = values['source']
                dialog.source.setCurrentIndex(0 if source is None else dialog.source.findData(source))
            if 'name' in values:
                dialog.name.setText(values['name'])
            for group, on in values.get('groups', {}).items():
                dialog.groups[group].setChecked(on)
            if 'iso' in values:
                dialog.iso.setChecked(values['iso'])
        elif kind == 'select':
            dialog.photo_ids = step[1]
        elif kind == 'save':
            dialog.save_rule()
        elif kind == 'row':
            dialog.list.setCurrentRow(-1 if step[1] is None else step[1])
        elif kind == 'toggle':
            dialog.toggle()
        elif kind == 'remove':
            dialog.remove()
        elif kind == 'apply':
            dialog.apply_current()
        state = defaults_state(dialog, window, kind)
        if steps and state['rules'] == last_rules:
            state['rules'] = None
        else:
            last_rules = state['rules']
        steps.append(state)
    photos = [{'name': n, 'info': i, 'settings': photo_settings(v)} for n, i, v in PHOTOS]
    write_json(GOLDEN / 'app' / 'raw_defaults' / 'case.json',
               {'root': ROOT, 'photos': photos, 'preset': normalized(USER_PRESET), 'script': DEFAULTS_SCRIPT,
                'steps': steps})
    catalog.db.close()


# ---- raw_studio --------------------------------------------------------------------------------------------------

class Combo:
    def __init__(self, data):
        self.data = data
        self.index = 0

    def currentData(self):
        return self.data[self.index] if self.index >= 0 else None

    def findData(self, value):
        return self.data.index(value) if value in self.data else -1

    def setCurrentIndex(self, index):
        self.index = index

    def blockSignals(self, value):
        return False


class Slider:
    def __init__(self, scale):
        self.scale = scale
        self.slider = self
        self.raw = 0

    def value(self):
        return self.raw

    def set_value(self, value):
        self.raw = round(value * self.scale)


class StudioPhotos:
    def __init__(self, rows):
        self.rows = rows

    def photo(self, ident):
        return self.rows[ident]


class StudioWindow:
    def __init__(self):
        self.settings = defaults()
        self.last_saved = dict(self.settings)
        self.commits = []
        self.current_id = 1
        self.source = None
        self.catalog = None
        self.adjustments = {'raw_kelvin': Slider(1), 'raw_tint': Slider(10)}

    def set_setting(self, key, value):
        from copy import deepcopy
        from luma.panel_keys import switch_on
        self.settings[key] = deepcopy(value)
        if key in ('highlights', 'shadows') and self.settings.get('tone_version') == 2:
            self.settings['tone_version'] = 3
        switch_on(self.settings, (key,))

    def commit(self, label='보정 변경'):
        if self.settings != self.last_saved:
            self.commits.append(label)
            self.last_saved = dict(self.settings)

    def load_controls(self):
        pass

    def render(self):
        pass


class Studio:
    change_working = StudioTools.change_working
    apply_dcp_record = StudioTools.apply_dcp_record
    clear_dcp = StudioTools.clear_dcp
    change_raw_mode = StudioTools.change_raw_mode
    raw_temperature_changed = StudioTools.raw_temperature_changed
    display_raw_white = StudioTools.display_raw_white
    camera_profile = StudioTools.camera_profile

    def __init__(self, window):
        self.w = window
        self.loading = False
        self.working = Combo(['sRGB', 'ProPhoto'])
        self.raw_mode = Combo(['legacy', 'as_shot', 'daylight', 'custom', 'sample'])


MATRICES = [
    ['good.json', {'matrix': [[1.1, 0.1, -0.2], [0, 1, 0], [0.05, -0.1, 1.05]]}],
    ['shape.json', {'matrix': [[1, 0], [0, 1]]}],
    ['large.json', {'matrix': [[11, 0, 0], [0, 1, 0], [0, 0, 1]]}],
    ['nan.json', {'matrix': [[None, 0, 0], [0, 1, 0], [0, 0, 1]]}],
    ['missing.json', {'values': [[1, 0, 0], [0, 1, 0], [0, 0, 1]]}],
    ['flags.json', {'matrix': [[True, 0, 0], [0, '1', 0], [0, 0, 1.5]]}],
]

# (step, value): photos 1 (the DNG, Test Camera) and 2 (a JPEG).
STUDIO_SCRIPT = [
    ['working', 'ProPhoto'], ['working', 'sRGB'],
    ['dcp', ['test-camera', 2]], ['dcp', ['synthetic_dual', 1]], ['dcp', ['test-camera', 1]],
    ['display'],
    ['mode', 'custom'], ['display'],
    ['temperature', [5000, 12.3]],
    ['mode', 'daylight'], ['display'],
    ['neutral', [0.6, 1, 0.8]], ['mode', 'sample'], ['display'],
    ['mode', 'custom'],
    ['mode', 'as_shot'],
    ['mode', 'legacy'], ['display'],
    ['mode', 'custom'],
    ['mode', 'legacy'],
    ['dcp', ['test-triple-illuminant', 1]], ['display'], ['mode', 'custom'], ['display'],
    ['clear'],
    ['mode', 'daylight'], ['display'], ['mode', 'custom'],
    ['matrix', 'good.json'], ['matrix', 'shape.json'], ['matrix', 'large.json'], ['matrix', 'nan.json'],
    ['matrix', 'missing.json'], ['matrix', 'flags.json'],
]

STUDIO_KEYS = ['working_space', 'raw_mode', 'raw_kelvin', 'raw_tint', 'raw_neutral', 'camera_matrix']


def raw_studio():
    folder = TEMP / 'studio'
    folder.mkdir(parents=True, exist_ok=True)
    for name, value in MATRICES:
        (folder / name).write_text(json.dumps(value), encoding='utf-8')
    info = {'make': 'Test', 'camera': 'Test Camera', 'format': 'DNG'}
    window = StudioWindow()
    window.catalog = StudioPhotos({1: {'path': str(DNG), 'info': info},
                                   2: {'path': 'Z:\\GrainyGolden\\raw\\x.jpg', 'info': {'format': 'JPEG'}}})
    tools = Studio(window)
    chosen = {}
    studio.QFileDialog.getOpenFileName = staticmethod(lambda *args: (chosen['path'], ''))
    sources = {}

    def source_for(settings):
        from luma.rawcolor import enabled
        if not enabled(settings):
            return None
        key = 'camera'
        if key not in sources:
            source, _ = load_image(DNG, 64, 'sRGB', raw_options={**defaults(), 'raw_mode': 'as_shot'})
            sources[key] = source
        return sources[key]

    steps = []
    for step in STUDIO_SCRIPT:
        WARNINGS.log.clear()
        source = source_for(window.settings)
        window.source = source if source is not None else object()
        kind = step[0]
        shown = None
        if kind == 'working':
            tools.working.setCurrentIndex(tools.working.findData(step[1]))
            tools.change_working()
        elif kind == 'dcp':
            name, ident = step[1]
            window.current_id = ident
            try:
                tools.apply_dcp_record(dcp_record(name))
            except ValueError as error:
                WARNINGS.log.append(['DCP 프로파일', str(error)])
            window.current_id = 1
        elif kind == 'display':
            for control in window.adjustments.values():
                control.raw = None
            source = source_for(window.settings)
            window.source = source if source is not None else object()
            tools.display_raw_white()
            shown = [window.adjustments['raw_kelvin'].raw, window.adjustments['raw_tint'].raw]
            if shown[0] is not None:
                shown = [shown[0] / 1, shown[1] / 10]
            else:
                shown = None
        elif kind == 'mode':
            source = source_for(window.settings)
            window.source = source if source is not None else object()
            tools.raw_mode.setCurrentIndex(tools.raw_mode.findData(step[1]))
            tools.change_raw_mode()
        elif kind == 'temperature':
            kelvin, tint = step[1]
            window.adjustments['raw_kelvin'].raw = round(kelvin * 1)
            window.adjustments['raw_tint'].raw = round(tint * 10)
            tools.raw_temperature_changed()
            window.commit()
        elif kind == 'neutral':
            window.set_setting('raw_neutral', step[1])
            window.commit()
        elif kind == 'clear':
            tools.clear_dcp()
        elif kind == 'matrix':
            chosen['path'] = str(folder / step[1])
            tools.camera_profile()
            window.commit()
        s = window.settings
        steps.append({'values': {k: s[k] for k in STUDIO_KEYS}, 'dcp': dcp_summary(s['dcp_profile']),
                      'commits': list(window.commits), 'warnings': list(WARNINGS.log), 'shown': shown})
    write_json(GOLDEN / 'app' / 'raw_studio' / 'case.json',
               {'info': info, 'matrices': MATRICES, 'script': STUDIO_SCRIPT, 'steps': steps})


if __name__ == '__main__':
    try:
        raw_profiles()
        raw_defaults_case()
        raw_studio()
    finally:
        shutil.rmtree(TEMP, ignore_errors=True)
