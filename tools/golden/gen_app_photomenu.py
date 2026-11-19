"""Goldens of grainy-app work package 14 (photo context menu and photo actions): golden/app/photomenu/.

* `menus`: the oracle's own `photo_actions.build_menu` run on a stand-in for the window over a real `Catalog`, for
  several states (one photo, a mixed selection, equal values, an export running, a smart collection selected, no
  photo). Stored per state: the menu as the `QMenu` holds it (titles, the action data keys, enabled, checked, the
  separators and submenus, in order).
* `sessions`: scripted sessions through `photo_actions.transform_selected`, `edit_selected` (흑백 / 컬러 전환),
  `reset_selected` and `WorkflowTools.camera_orientation` (and `MainWindow.commit`, `undo` between them); stored per
  checkpoint: the catalogue rows the actions write (settings as the keys that differ from the defaults, the history,
  the undo preferences, the extras), and per step the messages the window showed.

The window is a stand-in with the attributes those functions read (the pattern of gen_app_sync.py, whose file is not
touched). The library is `golden/app/photomenu/start.sqlite`. Deterministic: the catalogue's clock is a fixed stamp;
no clock, process id or random id is stored.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_photomenu.py
"""
import json
import shutil
import sqlite3
import tempfile
from copy import deepcopy
from datetime import datetime
from pathlib import Path

from common import GOLDEN, Case, np, write_json  # noqa: F401
from PySide6.QtWidgets import QApplication, QMenu
from luma import catalog as catalog_module, command_batch, photo_actions, quick_export
from luma.app import MainWindow
from luma.catalog import Catalog
from luma.engine import DEFAULTS, defaults, normalized
from luma.manager import LibraryManager
from luma.workflow_ui import WorkflowTools

STAMP = '2026-11-13T01:02:03.456789+00:00'
TOKEN = '0123456789abcdef0123456789abcdef'
ROOT = 'Z:\\GrainyMenu'

app = QApplication.instance() or QApplication([])


class Clock(datetime):
    @classmethod
    def now(cls, tz=None):
        value = datetime.fromisoformat(STAMP)
        return value if tz else value.replace(tzinfo=None)


catalog_module.datetime = Clock
command_batch.datetime = Clock


class Nop:
    """A widget, timer or callback the actions touch: every call does nothing and answers falsy."""

    def __getattr__(self, name):
        return Nop()

    def __call__(self, *args, **kwargs):
        return Nop()

    def __bool__(self):
        return False

    def __iter__(self):
        return iter(())


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


class Manager:
    """What the menu and the actions read of `LibraryManager`; the callbacks it hands out are never run."""
    edit_photo = LibraryManager.edit_photo

    def __init__(self, window):
        self.w = window
        self.collapsed = False
        self.collection = None
        self.stack_action = Nop()

    def __getattr__(self, name):
        return Nop()

    def refresh_thumbnails(self, ids):
        pass


class Flow:
    camera_orientation = WorkflowTools.camera_orientation
    records = WorkflowTools.records
    apply_edits = WorkflowTools.apply_edits

    def __init__(self, window):
        self.w = window
        self.previous_id = None
        self.cancel = None

    def __getattr__(self, name):
        return Nop()


class Window:
    """The state the actions and the menu read, with the window's own methods."""

    commit = MainWindow.commit
    save_undo = MainWindow.save_undo
    undo = MainWindow.undo
    redo = MainWindow.redo
    apply_restored = MainWindow.apply_restored
    set_setting = MainWindow.set_setting
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
        self.export_running = False
        self.source = object()
        self.render_version = 0
        self.bar = Bar()
        self.studio = Studio()
        self.manager = Manager(self)
        self.workflow = Flow(self)
        for name in ('commit_timer', 'photo_model', 'view', 'before_button', 'auto_tone_note', 'tabs',
                     'preview_timer', 'refine_timer', 'compare', 'preview_queue'):
            setattr(self, name, Nop())

    def __getattr__(self, name):
        return Nop()

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
        self.current_id = photo_id
        photo = self.catalog.photo(photo_id)
        self.settings = deepcopy(photo['settings'])
        self.last_saved = deepcopy(self.settings)
        state = self.catalog.preference(f'undo:{photo_id}', {})
        self.undo_stack = [normalized(s) for s in state.get('undo', [])]
        self.redo_stack = [normalized(s) for s in state.get('redo', [])]
        self.selection = [photo_id]


# ---- the library --------------------------------------------------------------------------------------------------

CROP = [0.1, 0.1, 0.8, 0.9]

PHOTOS = [   # (file, info, edits as (setting, value) lists, one list per commit)
    ('a.png', {'format': 'png', 'width': 24, 'height': 16},
     [[('exposure', 0.3)], [('rotation', 1), ('crop', CROP)]]),
    ('b.png', {'format': 'png', 'width': 24, 'height': 16},
     [[('flip', True)], [('contrast', 12.0)]]),
    ('c.png', {'format': 'png', 'width': 24, 'height': 16},
     [[('monochrome', True)], [('rotation', 2), ('flip', True), ('crop', CROP), ('panels_off', ['effects'])]]),
    ('r1.nef', {'format': 'nef', 'make': 'NIKON', 'camera': 'Nikon Z 6', 'width': 24, 'height': 16},
     [[('raw_mode', 'sample'), ('raw_neutral', [0.5, 0.4, 0.6])], [('exposure', 0.2), ('rotation', 3)]]),
    ('d.png', {'format': 'png', 'width': 24, 'height': 16}, [[('saturation', 8.0)]]),
    ('e.png', {'format': 'png', 'width': 24, 'height': 16}, []),
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
    c.update_many([1, 2], 'rating', 4)
    c.update_many([3], 'rating', 2)
    c.update_many([5, 6], 'rating', 5)
    c.update_many([1, 5], 'flag', 1)
    c.update_many([4], 'flag', -1)
    c.update_many([2, 3], 'label', '빨강')
    c.update_many([5, 6], 'label', '파랑')
    c.stack([3, 4])
    plain = c.add_collection('여행', None, None)
    c.collection_add(plain, [1, 2])
    smart = c.add_collection('좋은 사진', {'rating_min': 4}, None)
    c.snapshot(3, '첫 버전')
    c.db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
    return c, plain, smart


# ---- the menu as data -----------------------------------------------------------------------------------------------

def dump_menu(menu):
    result = []
    for action in menu.actions():
        if action.isSeparator():
            result.append('sep')
        elif action.menu() is not None:
            result.append({'sub': True, 'key': action.data() or '', 'title': action.text(), 'enabled': action.isEnabled(),
                           'children': dump_menu(action.menu())})
        else:
            tip = action.toolTip()
            result.append({'sub': False, 'key': action.data() or '', 'title': action.text(), 'enabled': action.isEnabled(),
                           'checked': action.isChecked() if action.isCheckable() else None,
                           'tip': None if tip.rstrip('…. ') == action.text().rstrip('…. ') else tip})
    return result


photo_actions.QMenu = lambda parent: QMenu()

PREVIOUS_EXPORT = {'folder': 'Z:\\Exports', 'format': 'JPEG', 'quality': 90, 'longest': 2048, 'color_space': 'sRGB',
                   'keep_metadata': False, 'naming': '{stem}', 'grayscale': True, 'dither': True}


def menu_state(catalog, plain, smart, spec):
    window = Window(catalog)
    window.current_id = spec.get('current')
    window.selection = list(spec.get('selection', []))
    window.copied = {'exposure': 0.5} if spec.get('copied') else None
    window.export_running = bool(spec.get('exporting'))
    window.manager.collapsed = bool(spec.get('collapsed'))
    window.manager.collection = {'plain': plain, 'smart': smart}.get(spec.get('collection'))
    window.workflow.previous_id = spec.get('previous')
    if spec.get('previous_export'):
        catalog.save_preference(quick_export.PREFERENCE, PREVIOUS_EXPORT)
    else:
        catalog.db.execute('DELETE FROM preferences WHERE key=?', (quick_export.PREFERENCE,))
    menu = photo_actions.build_menu(window)
    return dump_menu(menu)


MENUS = {
    'single': {'current': 1, 'selection': [1]},
    'mixed': {'current': 3, 'selection': [1, 2, 3, 4], 'copied': True, 'previous': 5, 'previous_export': True},
    # Collapsed stacks show only the cover (photo 3) of the stack of photos 3 and 4.
    'collapsed': {'current': 3, 'selection': [1, 2, 3], 'collapsed': True, 'previous': 1},
    'in_plain': {'current': 2, 'selection': [1, 2], 'collection': 'plain', 'copied': True},
    'equal': {'current': 5, 'selection': [5, 6], 'previous': 5},
    'exporting': {'current': 2, 'selection': [1, 2], 'exporting': True, 'collection': 'smart', 'previous_export': True},
    'plain_view': {'current': 6, 'selection': [6], 'previous': 1, 'copied': True},
}


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

def run_step(window, step):
    """One scripted action through the oracle's own functions. Returns the error text, or None."""
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
        elif action == 'rotate_right':
            photo_actions.transform_selected(window, turns=1)
        elif action == 'rotate_left':
            photo_actions.transform_selected(window, turns=-1)
        elif action == 'flip':
            photo_actions.transform_selected(window, flip=True)
        elif action == 'flip_vertical':
            photo_actions.transform_selected(window, turns=2, flip=True)
        elif action == 'camera':
            window.workflow.camera_orientation()
        elif action == 'mono':
            photo_actions.edit_selected(window, lambda v: v.update(monochrome=a[0]), '흑백 / 컬러 전환')
        elif action == 'reset':
            photo_actions.reset_selected(window)
        elif action == 'undo':
            window.undo()
        else:
            raise AssertionError(action)
    except Exception as error:   # the oracle's menu runner shows these in a message box
        return str(error)
    return None


def run_script(folder, script):
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
        steps.append({'step': step, 'messages': window.bar.messages[before:], 'error': error,
                      'upright_generation': window.studio.upright_generation})
    window.commit()
    c.db.commit()
    final = project(c.db)
    c.close()
    return steps, checkpoints, final


MAIN = [
    # Turns on a selection that holds the open photo (with a pending change) and others.
    ['open', 1], ['select', [1, 2, 3]], ['set', 'exposure', 0.9], ['rotate_right'], ['check', 'rotate_right'],
    ['rotate_left'], ['rotate_left'], ['check', 'rotate_left_twice'],
    ['flip'], ['check', 'flip'],
    ['flip_vertical'], ['check', 'flip_vertical'],
    # 카메라 방향으로 복원: only the rotated or flipped photos, the crop with them.
    ['select', [1, 2, 3, 4, 5]], ['camera'], ['check', 'camera'],
    ['undo'], ['check', 'camera_undo_open'],
    # Nothing is rotated now (photo 5 and 6): nothing is written.
    ['open', 5], ['select', [5, 6]], ['camera'], ['check', 'camera_nothing'],
    # 흑백으로 전환 / 컬러로 전환.
    ['open', 3], ['select', [3, 4, 5]], ['mono', True], ['check', 'mono'],
    ['mono', False], ['check', 'colour'],
    ['mono', False], ['check', 'colour_again'],
    # Another photo, alone.
    ['open', 6], ['select', [6]], ['rotate_right'], ['check', 'other_photo'],
    # 선택 사진 보정 초기화 and the open photo's undo afterwards.
    ['open', 1], ['select', [1, 3, 5]], ['reset'], ['check', 'reset'],
    ['undo'], ['check', 'reset_undo_open'],
    # A single photo: the selection is the open photo.
    ['open', 2], ['select', []], ['rotate_left'], ['check', 'single'],
]

with tempfile.TemporaryDirectory(prefix='grainy-app-photomenu-golden-', ignore_cleanup_errors=True) as temporary:
    work = Path(temporary).resolve()
    built = work / 'built'
    built.mkdir()
    library, plain_id, smart_id = build_library(built)
    library.close()
    out = GOLDEN / 'app' / 'photomenu'
    out.mkdir(parents=True, exist_ok=True)
    start = out / 'start.sqlite'
    start.unlink(missing_ok=True)
    shutil.copyfile(built / 'catalog.sqlite', start)
    db = sqlite3.connect(start)
    db.execute('PRAGMA journal_mode=DELETE')
    db.close()

    # The menus.
    menus = {}
    menu_dir = work / 'menus'
    menu_dir.mkdir()
    shutil.copyfile(start, menu_dir / 'catalog.sqlite')
    c = Catalog(menu_dir)
    for name, spec in MENUS.items():
        menus[name] = {'spec': spec, 'menu': menu_state(c, plain_id, smart_id, spec)}
    c.close()
    empty_dir = work / 'empty'
    empty_dir.mkdir()
    empty = Catalog(empty_dir)
    menus['none'] = {'spec': {'empty': True}, 'menu': menu_state(empty, None, None, {})}
    empty.close()

    # The sessions.
    session = work / 'main'
    session.mkdir()
    shutil.copyfile(start, session / 'catalog.sqlite')
    steps, checkpoints, final = run_script(session, MAIN)

    begin = sqlite3.connect(start)
    start_project = project(begin)
    begin.close()
    with Case('app', 'photomenu') as case:
        case.json(stamp=STAMP, token=TOKEN, plain_collection=plain_id, smart_collection=smart_id,
                  previous_export=PREVIOUS_EXPORT, previous_export_key=quick_export.PREFERENCE,
                  start=start_project, menus=menus,
                  session={'script': MAIN, 'steps': steps, 'checkpoints': checkpoints, 'final': final})
