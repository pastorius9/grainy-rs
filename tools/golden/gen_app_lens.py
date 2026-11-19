"""Goldens of grainy-app's lens correction (work package 21): `golden/app/lens_*`.

The oracle's own code runs headless, on a stand-in for the window (the few attributes `app.py` gives it):

* luma/studio.py `StudioTools.auto_lens`, `choose_lens`, `apply_lens`, `save_lens`, `load_lens` and the lens lines
  of `load_settings` (copied below as `load_labels`: the two labels, and the check boxes' state and enabling).
* luma/lens_dialog.py `LensDialog`, the real widget (offscreen Qt). `choose_lens` runs the dialog's `exec`, which
  here plays a script of the user's steps (pick a camera, pick a lens by name, type text, set a number, press
  보정 적용 or cancel) and records the dialog's state after every step.
* The camera and lens database is the committed lensfun extract of `crates/grainy-lens/tests/data` (the oracle's
  database functions are scoped to it, as `gen_lens.py` does), so the Rust test reads the same folder.

After every step: `lensfun` (an index into `profiles`), `lensfun_enabled`, the five check boxes (checked, enabled),
the seven manual values, `lens_profile`, the two labels, the commits, the status message, the message boxes
shown and, for a lens dialog, its states.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_lens.py
"""
import json
import tempfile
import xml.etree.ElementTree as ET
from copy import deepcopy
from pathlib import Path

from common import Case, ROOT, np
from luma import lens_dialog, optics, studio as studio_module
from luma.engine import defaults, normalized
from luma.i18n import tr as oracle_tr
from luma.panel_keys import switch_on
from luma.studio import StudioTools
import lensfunpy as lf

EXTRACT = ROOT / 'crates' / 'grainy-lens' / 'tests' / 'data'
MODELS = ['Nikon AF-S DX Nikkor 35mm f/1.8G', 'Canon EF 70-200mm f/2.8L IS USM',
          'Canon PowerShot G12 & compatibles (Standard)', 'Canon EF-M 22mm f/2 STM',
          'Canon PowerShot G3 & compatibles (Standard)', 'E 24mm f/1.8 ZA', 'Schneider 80mm Xenotar f/2.8']


# ---- the database: the committed extract -------------------------------------------------------------------------

def read_tree(path):
    return ET.parse(path, parser=ET.XMLParser(target=ET.TreeBuilder(insert_comments=True))).getroot()


_records = tuple(r for r in optics.lens_records() if r['model'] in MODELS)
_xml = '<lensdatabase version="1">' + ''.join(
    ''.join(ET.tostring(n, encoding='unicode') for n in read_tree(p)) for p in sorted(EXTRACT.glob('*.xml'))) + '</lensdatabase>'
_db = lf.Database(xml=_xml, load_common=False, load_bundled=False)
_cameras = tuple(sorted(_db.cameras, key=lambda c: (c.maker, c.model, c.variant or '')))
optics.database = lambda: _db
optics.cameras = lambda: _cameras
optics.lens_records = lambda: _records
lens_dialog.cameras = optics.cameras
lens_dialog.compatible_lenses = optics.compatible_lenses
lens_dialog.match_metadata = optics.match_metadata

from PySide6.QtWidgets import QApplication, QWidget  # noqa: E402

_qt = QApplication.instance() or QApplication([])


# ---- stand-ins ---------------------------------------------------------------------------------------------------

class Text:
    def __init__(self, text=''):
        self.value = text

    def setText(self, text):
        self.value = text


class Check:
    def __init__(self):
        self.checked = False
        self.enabled = True

    def setChecked(self, value):
        self.checked = bool(value)

    def setEnabled(self, value):
        self.enabled = bool(value)


BOXES = []
FILES = {}


class QMessageBox:
    @staticmethod
    def warning(parent, title, text):
        BOXES.append([title, text])


class QFileDialog:
    @staticmethod
    def getSaveFileName(parent, title, name, filter_):
        return FILES['save'], ''

    @staticmethod
    def getOpenFileName(parent, title, start, filter_):
        return FILES['open'], ''


studio_module.QMessageBox = QMessageBox
studio_module.QFileDialog = QFileDialog


class Catalog:
    def __init__(self, info):
        self.info = info

    def photo(self, ident):
        return {'info': deepcopy(self.info)}


class Window(QWidget):
    """MainWindow's attributes and methods the lens correction uses (app.py set_setting, commit, render)."""

    def __init__(self, changes, info):
        super().__init__()
        self.settings = normalized({**defaults(), **changes})
        self.last_saved = deepcopy(self.settings)
        self.current_id = 1
        self.source = np.zeros((8, 8, 3), np.float32)
        self.catalog = Catalog(info)
        self.history = []
        self.messages = []

    def statusBar(self):
        return self

    def showMessage(self, text):
        self.messages.append(text)

    def commit(self, label='보정 변경'):
        if self.current_id and self.settings != self.last_saved:
            self.history.append(label)
            self.last_saved = deepcopy(self.settings)

    def set_setting(self, key, value):
        if self.source is None:
            return
        self.settings[key] = deepcopy(value)
        switch_on(self.settings, (key,))

    def load_controls(self):
        self.studio.load_labels()

    def render(self):
        pass


class Studio:
    lens_keys = StudioTools.lens_keys

    def __init__(self, window):
        self.w = window
        window.studio = self
        self.lens_switches = {key: Check() for key in
                              ('lensfun_enabled', 'lensfun_distortion', 'lensfun_tca', 'lensfun_vignette', 'lensfun_scale')}
        self.auto_lens_label = Text(oracle_tr('자동 렌즈 프로파일 없음'))
        self.lens_label = Text(oracle_tr('렌즈 프로파일 없음'))

    def load_labels(self):
        # StudioTools.load_settings: the lines of the lens box.
        s = self.w.settings
        self.lens_label.setText(s['lens_profile'] or oracle_tr('렌즈 프로파일 없음'))
        self.auto_lens_label.setText(optics.description(s.get('lensfun')) if s.get('lensfun') else oracle_tr('자동 렌즈 프로파일 없음'))
        for key, control in self.lens_switches.items():
            caps = (s.get('lensfun') or {}).get('capabilities', [])
            capability = {'lensfun_distortion': 'distortion', 'lensfun_tca': 'tca', 'lensfun_vignette': 'vignette'}.get(key)
            control.setChecked(s[key])
            control.setEnabled(bool(s.get('lensfun')) and (capability is None or capability in caps))


for _name in ('apply_lens', 'auto_lens', 'choose_lens', 'save_lens', 'load_lens'):
    setattr(Studio, _name, getattr(StudioTools, _name))


# ---- the dialog, scripted ----------------------------------------------------------------------------------------

PROFILES = []
SCRIPTS = []
DIALOGS = []


def profile_index(profile):
    if not profile:
        return None
    if profile not in PROFILES:
        PROFILES.append(deepcopy(profile))
    return PROFILES.index(profile)


def spin(box):
    return [box.value(), box.minimum(), box.maximum()]


def dialog_state(dialog):
    return dict(
        camera=dialog.camera.currentIndex(), camera_text=dialog.camera.currentText(), cameras=dialog.camera.count(),
        lens=dialog.lens.currentIndex(), lens_text=dialog.lens.currentText(),
        lenses=[dialog.lens.itemText(i) for i in range(dialog.lens.count())],
        crop=spin(dialog.crop), focal=spin(dialog.focal), aperture=spin(dialog.aperture), distance=spin(dialog.distance),
        status=dialog.status.text(), result=profile_index(dialog.result_profile))


def find(combo, text):
    for i in range(combo.count()):
        if text in combo.itemText(i):
            return i
    raise ValueError(text)


def play(dialog, script):
    states = [dialog_state(dialog)]
    accepted = 0
    for step in script:
        kind, *args = step
        if kind == 'camera':
            dialog.camera.setCurrentIndex(find(dialog.camera, args[0]))
        elif kind == 'camera_text':
            dialog.camera.setEditText(args[0])
        elif kind == 'lens':
            dialog.lens.setCurrentIndex(find(dialog.lens, args[0]))
        elif kind == 'lens_text':
            dialog.lens.setEditText(args[0])
        elif kind == 'set':
            getattr(dialog, args[0]).setValue(args[1])
        elif kind == 'apply':
            dialog.apply()
            accepted = 1 if dialog.result_profile else 0
            states.append(dialog_state(dialog))
            if accepted:
                break      # accept() ends exec()
            continue
        elif kind == 'cancel':
            accepted = 0
        else:
            raise ValueError(kind)
        states.append(dialog_state(dialog))
    return accepted, states


def fake_exec(self):
    script = SCRIPTS.pop(0)
    opened = dialog_state(self)
    accepted, states = play(self, script)
    DIALOGS.append(dict(script=script, opened=opened, states=states[1:], accepted=bool(accepted)))
    return accepted


lens_dialog.LensDialog.exec = fake_exec


# ---- the scenarios -----------------------------------------------------------------------------------------------

CANON_5D2 = {'make': 'Canon', 'camera': 'Canon EOS 5D Mark II', 'lens': 'Canon EF 70-200mm f/2.8L IS USM',
             'focal_length': 100, 'aperture': 4, 'subject_distance': 3.5, 'source_orientation': 6}
NIKON_NO_LENS = {'make': 'NIKON CORPORATION', 'camera': 'NIKON D7000', 'focal_length': 35, 'aperture': 2}
SONY = {'make': 'Sony', 'camera': 'NEX-5', 'lens': 'E 24mm f/1.8 ZA', 'focal_length': 24, 'aperture': 2.8}
FILM = {'format': 'jpg'}

LENS_FILE = {'type': 'luma-lens', 'version': 1, 'name': 'Wide fix',
             'settings': {'distortion': 12.5, 'distortion_k2': -3, 'distortion_k3': 0, 'lens_vignette': 40,
                          'ca_red': 1.25, 'ca_blue': -1.25, 'defringe': 20}}

SCENARIOS = {
    # The photo's metadata names the camera and the lens: 렌즈 자동 선택 applies the profile; the check boxes.
    'auto': (CANON_5D2, {}, [
        ['auto'],
        ['switch', 'lensfun_distortion', False],
        ['switch', 'lensfun_distortion', True],
        ['switch', 'lensfun_scale', False],
        ['switch', 'lensfun_enabled', False],
        ['switch', 'lensfun_enabled', True],
        ['set', 'exposure', 0.4],
        ['auto'],
    ]),
    # Automatic selection fails (no lens name): the reason is the status and the dialog opens; cancelled, then
    # a lens chosen after the camera.
    'no_lens': (NIKON_NO_LENS, {}, [
        ['auto', [['cancel']]],
        ['auto', [['lens', 'Nikkor'], ['apply']]],
        ['switch', 'lensfun_tca', False],
    ]),
    # Nothing is known about the photo: pick everything by hand (lens first: its crop factor is taken, then a
    # camera), with mistakes on the way.
    'by_hand': (FILM, {}, [
        ['choose', [['apply'],
                    ['lens', 'Xenotar'],
                    ['camera_text', 'Nik'], ['apply'],
                    ['camera', 'NEX-5'], ['apply'],
                    ['lens', 'E 24mm'], ['set', 'focal', 30], ['set', 'aperture', 0], ['apply'],
                    ]],
        ['choose', [['camera', 'Canon EOS 5D Mark II'], ['lens', '70-200'], ['set', 'focal', 10],
                    ['set', 'focal', 135.123], ['set', 'aperture', 5.678], ['set', 'distance', 0.001],
                    ['set', 'crop', 1.2345], ['apply']]],
        ['choose', [['lens', 'Schneider'], ['set', 'focal', 80], ['set', 'aperture', 8], ['cancel']]],
        ['choose', [['lens', 'Schneider'], ['set', 'focal', 80], ['set', 'aperture', 8], ['apply']]],
    ]),
    # A lens without camera data and an aperture left unknown: only the capabilities the data supports.
    'capabilities': (SONY, {}, [
        ['auto'],
        ['switch', 'lensfun_vignette', False],
        ['switch', 'lensfun_distortion', False],
        ['choose', [['camera', 'PowerShot G12'], ['lens', 'G12'], ['set', 'aperture', 0], ['apply']]],
        ['auto'],
    ]),
    # A profile in the settings: the dialog opens on it (its lens, crop, focal, aperture, distance); a profile
    # whose lens the database does not have is kept in the list as a saved profile.
    'current': (CANON_5D2, {}, [
        ['auto'],
        ['choose', [['set', 'aperture', 2.8], ['apply']]],
        ['set_profile', 'unknown_lens'],
        ['choose', [['apply']]],
        ['choose', [['lens', 'Canon EF 70-200'], ['apply']]],
    ]),
    # The profile of another photo, copied: not the photo's own camera.
    'film_current': (FILM, {}, [
        ['set_profile', 'film'],
        ['choose', [['apply']]],
        ['auto'],
    ]),
    # The manual lens: sliders, saving, loading (valid files and every refusal).
    'manual': (FILM, {'distortion': 7.0, 'ca_red': -2.0, 'defringe': 30}, [
        ['save', 'lens.luma-lens.json'],
        ['save', 'Some lens.json'],
        ['load', 'a.json', LENS_FILE],
        ['load', 'b.json', {**LENS_FILE, 'name': ''}],
        ['load', 'c.json', {**LENS_FILE, 'name': None}],
        ['load', 'd.json', {**LENS_FILE, 'name': 5}],
        ['load', 'e.json', {'type': 'something', 'settings': LENS_FILE['settings']}],
        ['load', 'f.json', {'type': 'luma-lens'}],
        ['load', 'g.json', {**LENS_FILE, 'settings': {**LENS_FILE['settings'], 'distortion': 101}}],
        ['load', 'h.json', {**LENS_FILE, 'settings': {**LENS_FILE['settings'], 'defringe': 'abc'}}],
        ['load', 'i.json', {**LENS_FILE, 'settings': {**LENS_FILE['settings'], 'defringe': '12.5', 'ca_red': True}}],
        ['load', 'j.json', {**LENS_FILE, 'settings': {**LENS_FILE['settings'], 'defringe': None}}],
        ['load', 'k.json', {**LENS_FILE, 'settings': {'distortion': -100}}],
        ['load', 'm.json', {**LENS_FILE, 'settings': {**LENS_FILE['settings'], 'distortion_k2': [1]}}],
        ['load', 'n.json', {**LENS_FILE, 'settings': {**LENS_FILE['settings'], 'distortion_k3': ' 3 '}}],
        ['load', 'o.json', {**LENS_FILE, 'settings': {**LENS_FILE['settings'], 'lens_vignette': "it's"}}],
        ['load', 'p.json', LENS_FILE],
        ['set', 'distortion', 1.0], ['commit'],
        ['load', 'q.json', LENS_FILE],
    ]),
}


def named_profile(name):
    """Profiles written into the settings as a copy would leave them."""
    record = next(r for r in _records if r['model'] == 'Canon EF 70-200mm f/2.8L IS USM')
    if name == 'film':
        return optics.make_profile(next(r for r in _records if r['model'] == 'Schneider 80mm Xenotar f/2.8'),
                                   1.0, 80, 0, 1000, 1, '')
    if name == 'unknown_lens':
        profile = optics.make_profile(record, 1.0, 100, 4, 3.5, 1, 'Canon EOS 5D Mark II')
        xml = profile['xml'].replace('Canon EF 70-200mm f/2.8L IS USM', 'Canon EF 70-200mm f/2.8L IS USM (saved)')
        profile.update(xml=xml, name='Canon EF 70-200mm f/2.8L IS USM (saved)')
        return profile
    raise ValueError(name)


def state(window, studio):
    s = window.settings
    return dict(
        lensfun=profile_index(s.get('lensfun')), lensfun_enabled=s['lensfun_enabled'],
        switches={k: [c.checked, c.enabled] for k, c in studio.lens_switches.items()},
        values={k: s[k] for k in studio.lens_keys}, lens_profile=s['lens_profile'],
        auto_label=studio.auto_lens_label.value, lens_label=studio.lens_label.value,
        history=list(window.history), status=window.messages[-1] if window.messages else None,
        boxes=deepcopy(BOXES), exposure=s['exposure'])


def step(window, studio, folder, item):
    kind, *args = item
    dialogs = None
    if kind == 'auto':
        SCRIPTS.append(args[0] if args else [])
        before = len(DIALOGS)
        studio.auto_lens()
        SCRIPTS.clear()
        dialogs = DIALOGS[before:]
    elif kind == 'choose':
        SCRIPTS.append(args[0])
        before = len(DIALOGS)
        studio.choose_lens()
        dialogs = DIALOGS[before:]
    elif kind == 'switch':
        # The check box toggles itself, then `toggled` calls set_setting; the commit timer commits.
        studio.lens_switches[args[0]].setChecked(args[1])
        window.set_setting(args[0], args[1])
        window.commit()
    elif kind == 'set':
        window.set_setting(args[0], args[1])
    elif kind == 'commit':
        window.commit()
    elif kind == 'set_profile':
        window.set_setting('lensfun', named_profile(args[0]))
        window.set_setting('lensfun_enabled', True)
        window.commit('렌즈 자동 보정')
        window.load_controls()
    elif kind == 'save':
        path = folder / args[0]
        FILES['save'] = str(path)
        studio.save_lens()
        return dict(file=path.read_bytes().decode('utf-8'))
    elif kind == 'load':
        path = folder / args[0]
        # json.dumps writes NaN as the literal the oracle's json.loads reads back.
        path.write_text(json.dumps(args[1]), encoding='utf-8', newline='\n')
        FILES['open'] = str(path)
        studio.load_lens()
    else:
        raise ValueError(kind)
    return dict(dialogs=dialogs) if dialogs is not None else {}


for name, (info, changes, script) in SCENARIOS.items():
    PROFILES.clear()
    BOXES.clear()
    with tempfile.TemporaryDirectory() as tmp:
        folder = Path(tmp)
        window = Window(changes, info)
        studio = Studio(window)
        studio.load_labels()
        states = []
        for item in script:
            extra = step(window, studio, folder, item)
            states.append({**state(window, studio), **extra})
        with Case('app', f'lens_{name}') as case:
            case.json(info=info, changes=changes, script=script, states=states, profiles=PROFILES)

# The numbers of the number boxes, as the oracle's spin boxes round and bound them.
with Case('app', 'lens_spin') as case:
    class Probe:
        pass
    from PySide6.QtWidgets import QDoubleSpinBox
    rows = []
    for low, high, decimals in [(.1, 20, 3), (.1, 5000, 2), (0, 128, 2), (.01, 100000, 2)]:
        box = QDoubleSpinBox()
        box.setRange(low, high)
        box.setDecimals(decimals)
        values = []
        for value in [0, -5, .05, .0049999, .005, .0051, 1.2345, 1.2355, 2.675, 135.123, 1e9, 1.6000000238418579, 49.995, 70.0, 0.1234]:
            box.setValue(value)
            values.append([value, box.value()])
        ranges = []
        for lo, hi in [(70.0, 200.0), (24.0, 24.0), (.1, 5000), (17.5, 40.25)]:
            box.setValue(50)
            box.setRange(lo, hi)
            ranges.append([lo, hi, box.minimum(), box.maximum(), box.value()])
        rows.append(dict(low=low, high=high, decimals=decimals, values=values, ranges=ranges))
    case.json(rows=rows)


# ---- a develop with the profile the commands set ---------------------------------------------------------------

from common import inputs, settings  # noqa: E402
from luma import engine  # noqa: E402

DEVELOP = {
    # The profile of the photo's own metadata (all three corrections, a portrait photo), then the same without
    # the empty-edge fit and the chromatic aberration.
    'canon': (CANON_5D2, {}),
    'canon_no_scale': (CANON_5D2, {'lensfun_scale': False, 'lensfun_tca': False}),
    # A lens with an unknown aperture (no vignetting), a manual lens on top.
    'sony': ({**SONY, 'aperture': 0}, {'distortion': 10.0, 'ca_red': 5.0, 'lens_vignette': 30.0, 'defringe': 20.0}),
}
for name, (info, extra) in DEVELOP.items():
    profile, reason = optics.automatic_profile(info)
    assert profile, reason
    s = settings(lensfun=profile, lensfun_enabled=True, **extra)
    image = inputs()['colours']
    with Case('app', f'lens_develop_{name}') as case:
        case.array('develop', engine.develop(image, normalized(deepcopy(s))))
        case.json(info=info, extra=extra)
