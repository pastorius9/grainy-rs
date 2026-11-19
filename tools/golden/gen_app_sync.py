"""Goldens of grainy-app's sync and copy settings (APP_PLAN package 12), made with the oracle's own code.

A small library is built with the Python Grainy's own `Catalog` (five photos with their own edits: three ordinary
files, two RAW files of different cameras, one with a lens profile). Scripted sessions then run the oracle's own
methods, unbound, on a stand-in for the window (the pattern of `gen_app.py`): `MainWindow.copy_edits` /
`paste_edits` / `copy_crop_changed` / `commit`, `LibraryManager.choose_copy` (the dialog is answered by a stand-in
`QDialog.exec`) / `sync_selected` / `toggle_auto_sync` / `after_commit` / `edit_photo`, `WorkflowTools.previous_edits`
and `photo_actions.reset_selected`.

Stored per scenario and checkpoint: the catalogue as the actions leave it (settings written as the keys that differ
from the defaults, the history rows, the undo preferences, the extras), the messages the window showed and the
errors the actions raised. The library itself is `golden/app/sync/start.sqlite`.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_sync.py
"""
import json
import shutil
import sqlite3
import tempfile
from copy import deepcopy
from datetime import datetime
from pathlib import Path

from common import GOLDEN, Case, np, write_json  # noqa: F401
from PySide6.QtWidgets import QApplication, QDialog
from luma import catalog as catalog_module, command_batch, manager as manager_module, photo_actions
from luma.app import MainWindow
from luma.catalog import Catalog
from luma.engine import DEFAULTS, defaults, normalized
from luma.manager import EDIT_GROUPS, LibraryManager
from luma.workflow_ui import WorkflowTools

STAMP = '2026-11-13T01:02:03.456789+00:00'
TOKEN = '0123456789abcdef0123456789abcdef'
ROOT = 'Z:\\GrainySync'

app = QApplication.instance() or QApplication([])


class Clock(datetime):
    @classmethod
    def now(cls, tz=None):
        value = datetime.fromisoformat(STAMP)
        return value if tz else value.replace(tzinfo=None)


catalog_module.datetime = Clock
command_batch.datetime = Clock


class Nop:
    """A widget or timer the actions touch: every call does nothing and answers falsy."""

    def __getattr__(self, name):
        return Nop()

    def __call__(self, *args, **kwargs):
        return Nop()

    def __bool__(self):
        return False

    def __iter__(self):
        return iter(())


class Check:
    """A check box: `setChecked` emits `toggled` only when the state changes."""

    def __init__(self, on_toggle=None):
        self.on = False
        self.on_toggle = on_toggle

    def isChecked(self):
        return self.on

    def setChecked(self, value):
        value = bool(value)
        if value != self.on:
            self.on = value
            if self.on_toggle:
                self.on_toggle(value)


class Bar:
    """The status bar: every message is kept."""

    def __init__(self):
        self.messages = []

    def showMessage(self, text, *args):
        self.messages.append(text)


class Studio:
    upright_generation = 0

    def cancel_stroke(self):
        pass


class Answers:
    """The titles `choose_copy`'s dialog has ticked when it is accepted."""

    ticked = None


class AnsweringDialog(QDialog):
    """`QDialog` whose `exec` ticks the scripted check boxes and accepts."""

    def __init__(self, parent=None):
        super().__init__(None)

    def exec(self):
        from PySide6.QtWidgets import QCheckBox
        for box in self.findChildren(QCheckBox):
            box.setChecked(box.text() in Answers.ticked)
        return QDialog.DialogCode.Accepted


manager_module.QDialog = AnsweringDialog


class Manager:
    edit_photo = LibraryManager.edit_photo
    after_commit = LibraryManager.after_commit
    sync_selected = LibraryManager.sync_selected
    toggle_auto_sync = LibraryManager.toggle_auto_sync
    choose_copy = LibraryManager.choose_copy

    def __init__(self, window):
        self.w = window
        self.copy_keys = set(defaults()) - set(EDIT_GROUPS['크롭·회전']) - {'panels_off'}
        self.auto_sync = False
        self.syncing = False
        self.sync_action = Check()

    def refresh_thumbnails(self, ids):
        pass


class Flow:
    previous_edits = WorkflowTools.previous_edits
    records = WorkflowTools.records
    apply_edits = WorkflowTools.apply_edits

    def __init__(self, window):
        self.w = window
        self.previous_id = None


class Window:
    """The state the actions read, with the window's own methods."""

    commit = MainWindow.commit
    save_undo = MainWindow.save_undo
    undo = MainWindow.undo
    redo = MainWindow.redo
    apply_restored = MainWindow.apply_restored
    set_setting = MainWindow.set_setting
    copy_edits = MainWindow.copy_edits
    paste_edits = MainWindow.paste_edits
    copy_crop_changed = MainWindow.copy_crop_changed
    refresh_command_edits = MainWindow.refresh_command_edits

    def __init__(self, catalog):
        self.catalog = catalog
        self.current_id = None
        self.settings = defaults()
        self.last_saved = defaults()
        self.undo_stack = []
        self.redo_stack = []
        self.selection = []
        self.copied = None
        self.source = object()
        self.render_version = 0
        self.bar = Bar()
        self.studio = Studio()
        self.manager = Manager(self)
        self.workflow = Flow(self)
        self.copy_crop = Check(self.copy_crop_changed)
        for name in ('commit_timer', 'photo_model', 'view', 'before_button', 'auto_tone_note', 'tabs',
                     'preview_timer', 'refine_timer'):
            setattr(self, name, Nop())

    def statusBar(self):
        return self.bar

    def save_keywords(self):
        pass

    def end_crop(self):
        pass

    def refresh_history(self):
        pass

    def load_controls(self):
        pass

    def refresh_lists(self, *args, **kwargs):
        pass

    def render(self):
        pass

    def selected_ids(self):
        """The list's selection, in list order (the library's default order is the photo id)."""
        return sorted(self.selection) or ([self.current_id] if self.current_id else [])

    def activate(self, photo_id):
        """`MainWindow.activate` up to its background load: the stored settings and undo stacks, `previous_id`."""
        if photo_id == self.current_id:
            return
        self.commit()
        if self.current_id is not None and self.current_id != photo_id:
            self.workflow.previous_id = self.current_id
        self.current_id = photo_id
        photo = self.catalog.photo(photo_id)
        self.settings = deepcopy(photo['settings'])
        self.last_saved = deepcopy(self.settings)
        state = self.catalog.preference(f'undo:{photo_id}', {})
        self.undo_stack = [normalized(s) for s in state.get('undo', [])]
        self.redo_stack = [normalized(s) for s in state.get('redo', [])]
        self.selection = [photo_id]


# ---- the library --------------------------------------------------------------------------------------------------

LENS = {'maker': 'Sigma', 'name': 'Sigma 35mm f/1.4 DG HSM | Art', 'crop': 1.0, 'camera': '', 'xml': '',
        'focal': 35.0, 'aperture': 2.8}

PHOTOS = [   # (file, info, edits as (setting, value) lists, one list per commit)
    ('a.png', {'format': 'png', 'width': 24, 'height': 16},
     [[('exposure', 0.4)], [('contrast', 15.0)], [('panels_off', ['effects']), ('vignette', -20.0)]]),
    ('b.png', {'format': 'png', 'width': 24, 'height': 16, 'lens': 'Canon EF 50mm f/1.8 STM'},
     [[('saturation', 12.0)], [('film_look', 'kodak-portra-400')]]),
    ('c.png', {'format': 'png', 'width': 24, 'height': 16},
     [[('monochrome', True)], [('lensfun', LENS), ('lensfun_enabled', True)], [('temperature', 12.0)]]),
    ('r1.nef', {'format': 'nef', 'make': 'NIKON', 'camera': 'Nikon Z 6', 'width': 24, 'height': 16},
     [[('raw_mode', 'sample'), ('raw_neutral', [0.5, 0.4, 0.6])], [('exposure', 0.2)]]),
    ('r2.nef', {'format': 'nef', 'make': 'Canon', 'camera': 'Canon EOS R5', 'width': 24, 'height': 16},
     [[('exposure', -0.3)]]),
]


def build_library(folder):
    c = Catalog(folder)
    c.register_folder(ROOT)
    for name, info, _ in PHOTOS:
        c.add(ROOT + '\\' + name, settings=None, metadata=info)
    window = Window(c)
    for index, (_, _, commits) in enumerate(PHOTOS, 1):
        window.activate(index)
        for changes in commits:
            for key, value in changes:
                window.set_setting(key, value)
            window.commit()
    window.commit()
    c.db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
    return c


# ---- what a checkpoint stores ---------------------------------------------------------------------------------------

def sparse(settings):
    """The settings as the keys that differ from the defaults (Python equality), plus how many keys there are."""
    changed = {k: v for k, v in settings.items() if k not in DEFAULTS or v != DEFAULTS[k]}
    return {'keys': len(settings), 'changed': changed}


def project(db):
    """The catalogue's rows the actions write, settings sparse: photos, history and preferences."""
    photos = {}
    for ident, edits, extras in db.execute('SELECT id,edits,extras FROM photos ORDER BY id'):
        photos[str(ident)] = {'settings': sparse(json.loads(edits)), 'extras': json.loads(extras or '{}')}
    history = [{'id': i, 'photo': p, 'label': label, 'settings': sparse(json.loads(edits)), 'created': created}
               for i, p, label, edits, created in db.execute(
                   'SELECT id,photo_id,label,edits,created FROM history ORDER BY id')]
    preferences = {}
    for key, value in db.execute('SELECT key,value FROM preferences ORDER BY key'):
        value = json.loads(value)
        if key.startswith('undo:') and isinstance(value, dict) and 'undo' in value:
            extra = {k: v for k, v in value.items() if k not in ('undo', 'redo')}
            value = {'undo': [sparse(s) for s in value['undo']], 'redo': [sparse(s) for s in value['redo']], **extra}
        preferences[key] = value
    return {'photos': photos, 'history': history, 'preferences': preferences}


# ---- the scripted sessions ------------------------------------------------------------------------------------------

ALL = [title for title in EDIT_GROUPS]


def run_step(window, step):
    """One scripted action through the oracle's own methods. Returns the error text, or None."""
    action, a = step[0], step[1:]
    try:
        if action == 'open':
            window.activate(a[0])
        elif action == 'set':
            window.set_setting(a[0], a[1])
        elif action == 'commit':
            window.commit(*a)
        elif action == 'select':
            window.selection = list(a[0])
        elif action == 'copy':
            window.copy_edits()
        elif action == 'paste':
            window.paste_edits()
        elif action == 'choose':
            Answers.ticked = list(a[0])
            window.manager.choose_copy()
        elif action == 'copy_crop':
            window.copy_crop.setChecked(a[0])
        elif action == 'sync':
            window.manager.sync_selected()
        elif action == 'auto':
            window.manager.sync_action.on = a[0]
            window.manager.toggle_auto_sync()
        elif action == 'previous':
            window.workflow.previous_edits()
        elif action == 'reset_selected':
            photo_actions.reset_selected(window)
        elif action == 'undo':
            window.undo()
        elif action == 'redo':
            window.redo()
        else:
            raise AssertionError(action)
    except Exception as error:   # the oracle's menu runner shows these in a message box
        return str(error)
    return None


def run_script(folder, script):
    """Runs `script` on a copy of the library in `folder`; the checkpoints it names are stored."""
    c = Catalog(folder)
    window = Window(c)
    steps = []
    checkpoints = {}
    for step in script:
        if step[0] == 'check':
            window.commit()
            c.db.commit()
            checkpoints[step[1]] = project(c.db)
            continue
        before = len(window.bar.messages)
        error = run_step(window, step)
        messages = window.bar.messages[before:]
        steps.append({'step': step, 'messages': messages, 'error': error,
                      'copy_keys': sorted(window.manager.copy_keys), 'copy_crop': window.copy_crop.on,
                      'previous': window.workflow.previous_id})
    window.commit()
    c.db.commit()
    final = project(c.db)
    c.close()
    return steps, checkpoints, final


CROPPED = [('crop', [0.1, 0.1, 0.8, 0.9]), ('rotation', 1), ('straighten', 3.0)]

MAIN = [
    # A lens profile that does not fit the other photo's lens: nothing is written (paste, sync, previous).
    ['open', 3], ['copy'], ['open', 2], ['select', [2]], ['paste'], ['check', 'lens_refused_paste'],
    ['open', 3], ['select', [3, 2]], ['sync'], ['check', 'lens_refused_sync'],
    ['open', 3], ['open', 2], ['select', [2]], ['previous'], ['check', 'lens_refused_previous'],
    # Paste with the default copy set: crop and rotation stay as they are.
    ['open', 1], *[['set', *change] for change in CROPPED], ['commit'], ['copy'],
    ['open', 2], ['select', [2, 3]], ['paste'], ['check', 'paste_default'],
    # 붙여넣기에 크롭·회전도 포함: the ticked crop group travels.
    ['open', 1], ['choose', ALL], ['open', 2], ['select', [2, 3]], ['paste'], ['check', 'paste_with_crop'],
    # Two groups only: a RAW photo takes its colour settings from its file whatever is pasted.
    ['open', 1], ['set', 'exposure', 1.1], ['set', 'saturation', 40.0], ['commit'], ['choose', ['빛', '색상']],
    ['open', 4], ['select', [4, 5]], ['paste'], ['check', 'paste_raw'],
    # 동기화: the open photo's ticked groups to the other selected photos.
    ['open', 4], ['set', 'exposure', 0.9], ['set', 'contrast', -10.0], ['commit'], ['select', [4, 5, 1]], ['sync'],
    ['check', 'sync'],
    # Auto Sync: a changed value and a switched panel follow the selection while it is on.
    ['auto', True], ['open', 1], ['select', [1, 2, 3]], ['set', 'exposure', -0.5], ['commit'],
    ['set', 'panels_off', ['basic']], ['commit'], ['set', 'contrast', 22.0], ['set', 'texture', 12.0], ['commit'],
    ['check', 'auto_sync'],
    ['auto', False], ['select', [1, 2, 3]], ['set', 'contrast', 5.0], ['commit'], ['check', 'auto_sync_off'],
    # 이전 사진 보정 붙이기: the photo left last to the selection, as one batch.
    ['open', 1], ['open', 2], ['select', [2, 3, 4]], ['previous'], ['check', 'previous'],
    ['undo'], ['check', 'previous_undo_open'],
    # 선택 사진 보정 초기화.
    ['open', 1], ['select', [1, 3, 5]], ['reset_selected'], ['check', 'reset'],
    ['undo'], ['check', 'reset_undo_open'],
]

NO_PREVIOUS = [
    ['open', 1], ['select', [1, 2]], ['previous'], ['check', 'no_previous'],
]

with tempfile.TemporaryDirectory(prefix='grainy-app-sync-golden-', ignore_cleanup_errors=True) as temporary:
    work = Path(temporary).resolve()
    built = work / 'built'
    built.mkdir()
    build_library(built).close()
    out = GOLDEN / 'app' / 'sync'
    out.mkdir(parents=True, exist_ok=True)
    start = out / 'start.sqlite'
    start.unlink(missing_ok=True)
    shutil.copyfile(built / 'catalog.sqlite', start)
    db = sqlite3.connect(start)
    db.execute('PRAGMA journal_mode=DELETE')
    db.close()

    results = {}
    for name, script in (('main', MAIN), ('no_previous', NO_PREVIOUS)):
        session = work / name
        session.mkdir()
        shutil.copyfile(start, session / 'catalog.sqlite')
        steps, checkpoints, final = run_script(session, script)
        results[name] = {'script': script, 'steps': steps, 'checkpoints': checkpoints, 'final': final}

    begin = sqlite3.connect(start)
    start_project = project(begin)
    begin.close()
    with Case('app', 'sync') as case:
        case.json(stamp=STAMP, token=TOKEN, groups={k: list(v) for k, v in EDIT_GROUPS.items()},
                  default_copy_keys=sorted(set(defaults()) - set(EDIT_GROUPS['크롭·회전']) - {'panels_off'}),
                  start=start_project, sessions=results)
