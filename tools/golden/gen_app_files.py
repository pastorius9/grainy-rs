"""Goldens of grainy-app work package 22 (file management and catalogue maintenance): golden/app/files_*.

* `files_steps`: a scripted session through the oracle's own methods `LibraryManager.rename`, `move`,
  `move_selected_folder`, `remove(True)`, `relink`, `duplicates`, `backup`, `restore`, `integrity`,
  `relink_folder`, `FolderPanel.locate` / `apply_many` (and `MainWindow.commit` for the edits between them), on a
  library of small generated PNG files in a temporary folder, with the boxes and file dialogs answered from the
  script and `send2trash` replaced by a stand-in that moves the file into a `bin` folder of the temporary folder
  (the system's recycle bin is never used). After each step: the catalogue tables (paths relative to the
  temporary folder, written `<ROOT>`), the files on disk with their contents' input names, the backups made (by
  kind), the boxes shown, the error, the open photo and the folder chosen in the tree.
* `files_optimize`: `catalog_maintenance.optimize` on the library after the script (the report's counts).
* `files_offline`: `manager.smart_previews` of one photo: the packet's pixels (float16) and info keys.
* `files_inputs`: the PNG files (as uint8 arrays of their bytes) both sides copy into their temporary folders.

The window and folder panel are stand-ins with the attributes those methods read. Nothing is written outside a
temporary folder and `golden/app`. Deterministic: the clocks are fixed, the backup names' random part is fixed,
and paths are stored relative to the temporary folder.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_files.py
"""
import io
import json
import shutil
import sqlite3
import sys
import tempfile
import types
from copy import deepcopy
from datetime import datetime
from pathlib import Path

from common import Case, np

from luma import catalog as catalog_module, catalog_maintenance, folder_panel as panel_module  # noqa: E402
from luma import library as library_module, manager as manager_module  # noqa: E402
from luma.app import MainWindow  # noqa: E402
from luma.catalog import Catalog  # noqa: E402
from luma.engine import defaults, normalized  # noqa: E402
from luma.folder_panel import FolderPanel  # noqa: E402
from luma.manager import EDIT_GROUPS, LibraryManager  # noqa: E402
from PIL import Image  # noqa: E402

STAMP = '2026-11-13T01:02:03.456789+00:00'


class Clock(datetime):
    @classmethod
    def now(cls, tz=None):
        value = datetime.fromisoformat(STAMP)
        return value if tz else value.replace(tzinfo=None)


catalog_module.datetime = Clock
library_module.datetime = Clock
panel_module.datetime = Clock
catalog_maintenance.datetime = Clock


class FixedId:
    hex = '0123456789abcdef0123456789abcdef'


panel_module.uuid4 = lambda: FixedId()
catalog_maintenance.uuid4 = lambda: FixedId()


class Nop:
    def __getattr__(self, name):
        return Nop()

    def __call__(self, *args, **kwargs):
        return Nop()

    def __bool__(self):
        return False

    def __iter__(self):
        return iter(())


# ---- the boxes ------------------------------------------------------------------------------------------------

class Answers:
    def __init__(self):
        self.queue = []
        self.shown = []

    def take(self):
        if not self.queue:
            raise AssertionError('a box opened that the script did not answer')
        return self.queue.pop(0)


ANSWERS = Answers()
ROOT = None


def rel(text):
    """A path or a text with paths, relative to the temporary folder."""
    if text is None:
        return None
    return str(text).replace(str(ROOT), '<ROOT>')


def full(text):
    return str(text).replace('<ROOT>', str(ROOT))


class ScriptedInput:
    @staticmethod
    def getText(parent, title, label, *args, text='', **kwargs):
        ANSWERS.shown.append(dict(box='text', title=title, label=label, text=rel(text)))
        answer = ANSWERS.take()
        return (full(answer), True) if answer is not None else ('', False)


class ScriptedFiles:
    @staticmethod
    def getExistingDirectory(parent, title, start='', *args, **kwargs):
        ANSWERS.shown.append(dict(box='folder', title=title))
        answer = ANSWERS.take()
        return full(answer) if answer is not None else ''

    @staticmethod
    def getOpenFileName(parent, title, start='', *args, **kwargs):
        ANSWERS.shown.append(dict(box='open', title=title))
        answer = ANSWERS.take()
        return (full(answer) if answer is not None else '', '')

    @staticmethod
    def getSaveFileName(parent, title, name='', *args, **kwargs):
        ANSWERS.shown.append(dict(box='save', title=title))
        answer = ANSWERS.take()
        return (full(answer) if answer is not None else '', '')


class Button:
    Yes = 'yes'
    No = 'no'

    def __or__(self, other):
        return self


class ScriptedMessages:
    class StandardButton:
        Yes = 'yes'
        No = 'no'

    @staticmethod
    def question(parent, title, text, *args, **kwargs):
        ANSWERS.shown.append(dict(box='question', title=title, text=rel(text)))
        return 'yes' if ANSWERS.take() else 'no'

    @staticmethod
    def information(parent, title, text, *args, **kwargs):
        ANSWERS.shown.append(dict(box='information', title=title, text=rel(text)))

    @staticmethod
    def warning(parent, title, text, *args, **kwargs):
        ANSWERS.shown.append(dict(box='warning', title=title, text=rel(text)))


class ScriptedPanelMessages(ScriptedMessages):
    class StandardButton:
        Yes = 'yes'
        No = 'no'

        def __class_getitem__(cls, item):
            return cls


class Flags(str):
    def __or__(self, other):
        return self


ScriptedPanelMessages.StandardButton.Yes = Flags('yes')
ScriptedPanelMessages.StandardButton.No = Flags('no')

manager_module.QInputDialog = ScriptedInput
manager_module.QFileDialog = ScriptedFiles
manager_module.QMessageBox = ScriptedMessages
panel_module.QFileDialog = ScriptedFiles
panel_module.QMessageBox = ScriptedPanelMessages
panel_module.QTimer = Nop()
panel_module.QApplication = types.SimpleNamespace(activeModalWidget=lambda: None, activePopupWidget=lambda: None)

TRASHED = []


def fake_send2trash(path):
    """The stand-in recycle bin: the file moves into `<ROOT>/bin` under a numbered name."""
    target = ROOT / 'bin' / f'{len(TRASHED) + 1:02d}-{Path(path).name}'
    target.parent.mkdir(exist_ok=True)
    shutil.move(str(path), str(target))
    TRASHED.append(rel(Path(path)))


sys.modules['send2trash'] = types.SimpleNamespace(send2trash=fake_send2trash)


# ---- the stand-in window ---------------------------------------------------------------------------------------

class Exclusions:
    def __init__(self):
        self.excluded = []

    def exclude(self, paths):
        self.excluded.extend(sorted(rel(p) for p in paths))

    def folder_removed(self):
        pass


class Manager:
    after_commit = LibraryManager.after_commit
    selected_photos = LibraryManager.selected_photos
    reload_current = LibraryManager.reload_current
    rename = LibraryManager.rename
    move = LibraryManager.move
    move_selected_folder = LibraryManager.move_selected_folder
    remove = LibraryManager.remove
    relink = LibraryManager.relink
    duplicates = LibraryManager.duplicates
    backup = LibraryManager.backup
    restore = LibraryManager.restore
    integrity = LibraryManager.integrity
    relink_folder = LibraryManager.relink_folder

    def __init__(self, window):
        self.w = window
        self.copy_keys = set(defaults()) - set(EDIT_GROUPS['크롭·회전']) - {'panels_off'}
        self.auto_sync = False
        self.syncing = False
        self.collection = None
        self.rules = {}
        self.dialogs = []

    def refresh_collections(self):
        pass


class Window:
    commit = MainWindow.commit
    save_undo = MainWindow.save_undo
    set_setting = MainWindow.set_setting

    def __init__(self, catalog):
        self.catalog = catalog
        self.current_id = None
        self.settings = defaults()
        self.last_saved = defaults()
        self.undo_stack = []
        self.redo_stack = []
        self.selection = []
        self.source = object()
        self.render_version = 0
        self.folder_filter = None
        self.expanded_folders = set()
        self.folder_sync = Exclusions()
        self.import_busy = False
        self.import_scans = []
        self.export_running = False
        self.closing = False
        self.duplicate_groups = None
        self.manager = Manager(self)
        for name in ('commit_timer', 'photo_model', 'view', 'before_button', 'auto_tone_note', 'copy_crop', 'tabs',
                     'preview_timer', 'refine_timer', 'pool', 'preview_queue', 'thumbnail_render_pool',
                     'thumbnail_pool', 'library_cancel', 'browser_pool', 'source_cache', 'extras'):
            setattr(self, name, Nop())
        self.library_version = 0

    def statusBar(self):
        return Nop()

    def refresh_history(self):
        pass

    def refresh_presets(self):
        pass

    def save_keywords(self):
        pass

    def save_folder_state(self):
        pass

    def load_controls(self):
        pass

    def render(self):
        pass

    def refresh_lists(self, *args, ensure_current=False):
        pass

    def spawn(self, work, done, failed=None, pool=None):
        result = work()
        if isinstance(result, list) and (not result or isinstance(result[0], list)):
            self.duplicate_groups = [[p['id'] for p in rows] for rows in result]
            if result:                                       # the tree window: its rows are recorded instead
                return
        done(result)

    def selected_ids(self):
        return list(self.selection) or ([self.current_id] if self.current_id else [])

    def clear_active_photo(self):
        self.commit()
        self.current_id = None
        self.settings = self.last_saved = defaults()

    def activate(self, photo_id, force=False):
        if photo_id == self.current_id and not force:
            return
        self.commit()
        self.current_id = photo_id
        photo = self.catalog.photo(photo_id)
        self.settings = deepcopy(photo['settings'])
        self.last_saved = deepcopy(self.settings)
        state = self.catalog.preference(f'undo:{photo_id}', {})
        self.undo_stack = [normalized(s) for s in state.get('undo', [])]
        self.redo_stack = [normalized(s) for s in state.get('redo', [])]
        if self.selection != [photo_id] and photo_id not in self.selection:
            self.selection = [photo_id]


def panel_for(window):
    panel = FolderPanel.__new__(FolderPanel)
    panel.w = window
    panel.children = []
    panel.missing = set()
    panel.scanned = set()
    panel.signature = None
    panel.running = False
    panel.failed_moves = set()
    panel.recovery_running = False
    panel.recovery_signature = None
    from threading import Event
    panel.cancel = Event()
    panel.recovery_cancel = Event()
    window.folder_panel = panel
    window.maintenance_running = False
    return panel


# ---- the inputs and the library -----------------------------------------------------------------------------------

def png(width, height, seed):
    """A small deterministic PNG."""
    y, x = np.mgrid[0:height, 0:width]
    rgb = np.stack([(x * 9 + seed * 40) % 256, (y * 13 + seed * 70) % 256, ((x + y) * 5 + seed * 25) % 256], -1)
    stream = io.BytesIO()
    Image.fromarray(rgb.astype(np.uint8)).save(stream, format='PNG')
    return stream.getvalue()


INPUTS = {'one': png(24, 16, 1), 'two': png(24, 16, 2), 'three': png(20, 30, 3), 'four': png(32, 12, 4)}

# (relative path, input): `copy.png` has the bytes of `a1.png`.
FILES = [
    ('photos/A/a1.png', 'one'),
    ('photos/A/a2.png', 'two'),
    ('photos/A/sub/a3.png', 'three'),
    ('photos/B/b1.png', 'four'),
    ('photos/B/copy.png', 'one'),
    ('photos/C/c1.png', 'two'),
    ('spare/found.png', 'three'),
]
LIBRARY = ['photos/A/a1.png', 'photos/A/a2.png', 'photos/A/sub/a3.png', 'photos/B/b1.png', 'photos/B/copy.png',
           'photos/C/c1.png']


def build(root):
    for path, name in FILES:
        target = root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(INPUTS[name])
    (root / 'library').mkdir()
    (root / 'elsewhere').mkdir()
    catalog = Catalog(root / 'library')
    for path in LIBRARY:
        catalog.add(root / path, {}, {'width': 1, 'height': 1})
    catalog.virtual_copy(1)                                    # photo 7: a virtual copy of a1
    catalog.edit(2, normalized({'exposure': 0.5}), '노출')
    catalog.update_many([4], 'rating', 3)
    return catalog


TABLE_COLUMNS = ['id', 'path', 'name', 'virtual_source', 'rating', 'imported']
PREFERENCES = ('removed_folders', 'folder_panel', 'watch_folders', 'folder_identities', 'sync_excluded')


def tables(db):
    rows = db.execute(f'SELECT {",".join(TABLE_COLUMNS)} FROM photos ORDER BY id').fetchall()
    photos = [dict(zip(TABLE_COLUMNS, r)) for r in rows]
    for p in photos:
        p['path'] = rel(p['path'])
    roots = sorted(rel(r[0]) for r in db.execute('SELECT path FROM folder_roots'))
    prefs = {}
    for key in PREFERENCES:
        row = db.execute('SELECT value FROM preferences WHERE key=?', (key,)).fetchone()
        prefs[key] = json.loads(rel(row[0])) if row else None
    return dict(photos=photos, folder_roots=roots, preferences=prefs,
                history=[list(r) for r in db.execute('SELECT id,photo_id,label FROM history ORDER BY id')])


def tree(root):
    """Every file below the root (but the library's own files) with the input its bytes are."""
    names = {v: k for k, v in INPUTS.items()}
    out = []
    for path in sorted(root.rglob('*')):
        if path.is_file() and 'library' not in path.relative_to(root).parts[:1]:
            out.append([path.relative_to(root).as_posix(), names.get(path.read_bytes(), 'other')])
    return out


def backups(root):
    folder = root / 'library' / 'backups'
    kinds = {}
    if folder.is_dir():
        for path in folder.iterdir():
            kind = path.name.split('-2026')[0]
            kinds[kind] = kinds.get(kind, 0) + 1
    return kinds


def run_script(script):
    catalog = build(ROOT)
    window = Window(catalog)
    manager = window.manager
    panel = panel_for(window)
    steps = []
    previous = None
    for step in script:
        action, a = step[0], step[1:]
        ANSWERS.queue.clear()
        ANSWERS.shown.clear()
        TRASHED.clear()
        window.duplicate_groups = None
        error = None
        try:
            if action == 'open':
                window.activate(a[0])
                window.selection = [a[0]]
            elif action == 'select':
                window.selection = list(a[0])
            elif action == 'set':
                window.set_setting(a[0], a[1])
            elif action == 'folder':
                window.folder_filter = full(a[0]) if a[0] else None
            elif action == 'fs_move':                       # the person moves a file or folder outside Grainy
                shutil.move(full(a[0]), full(a[1]))
            elif action == 'rating':                        # a change to undo by a restore
                catalog.update_many(list(a[0]), 'rating', a[1])
            elif action == 'rename':
                ANSWERS.queue.append(a[0])
                manager.rename()
            elif action == 'move':
                ANSWERS.queue.append(a[0])
                manager.move()
            elif action == 'move_folder':
                ANSWERS.queue.append(a[0])
                manager.move_selected_folder()
            elif action == 'trash':
                ANSWERS.queue.append(a[0])
                manager.remove(True)
            elif action == 'relink':
                ANSWERS.queue.append(a[0])
                manager.relink()
            elif action == 'duplicates':
                manager.duplicates()
            elif action == 'backup':
                ANSWERS.queue.append(a[0])
                manager.backup()
            elif action == 'restore':
                ANSWERS.queue.extend([a[0], a[1]])
                manager.restore()
            elif action == 'integrity':
                manager.integrity()
            elif action == 'locate':                         # FolderPanel.locate(source) with the new place and 예/아니요
                ANSWERS.queue.extend([a[1], a[2]])
                panel.locate(full(a[0]))
            elif action == 'relink_folder':
                ANSWERS.queue.extend([a[0], a[1]])
                manager.relink_folder()
            else:
                raise ValueError(action)
        except Exception as exc:   # noqa: BLE001 - the oracle's `run` shows the message in a box
            error = rel(str(exc))
        catalog.db.commit()
        now = tables(catalog.db)
        steps.append(dict(
            step=step, error=error, boxes=list(ANSWERS.shown), trashed=list(TRASHED), current=window.current_id,
            folder=rel(window.folder_filter), duplicates=window.duplicate_groups, files=tree(ROOT),
            backups=backups(ROOT), excluded=list(window.folder_sync.excluded),
            **({} if now == previous else {'tables': now})))
        previous = now
    return catalog, steps


SCRIPT = [
    ['open', 1],
    ['set', 'exposure', 0.25],                                # a pending change: stored by the first command
    ['select', [1, 2]],
    ['rename', '{stem}_{n:03d}'],                             # a1 → a1_001, a2 → a2_002; the virtual copy follows
    ['rename', 'x/{stem}'],                                   # a folder part: refused
    ['rename', None],                                         # cancelled
    ['open', 5],
    ['rename', 'b1'],                                         # copy.png → b1.png exists already: refused
    ['rename', '{name}'],                                     # an unknown field: KeyError
    ['open', 6],
    ['move', '<ROOT>\\photos\\B'],                           # c1.png into B
    ['move', None],                                           # cancelled
    ['open', 4],
    ['move', '<ROOT>\\photos\\A'],
    ['move', '<ROOT>\\photos\\A'],                           # moved already: nothing
    ['folder', None],
    ['move_folder', '<ROOT>\\photos\\Z'],                    # no folder chosen: refused
    ['folder', '<ROOT>\\photos\\A'],
    ['move_folder', '<ROOT>\\photos\\A\\sub\\inner'],        # into itself: refused
    ['move_folder', '<ROOT>\\photos\\Renamed'],
    ['move_folder', None],
    ['open', 3],
    ['fs_move', '<ROOT>\\photos\\Renamed\\sub\\a3.png', '<ROOT>\\spare\\a3-moved.png'],
    ['relink', '<ROOT>\\spare\\a3-moved.png'],
    ['relink', '<ROOT>\\library\\catalog.sqlite'],          # not a photo
    ['duplicates'],
    ['integrity'],
    ['backup', '<ROOT>\\elsewhere\\saved.sqlite'],
    ['rating', [1, 2, 3], 5],
    ['restore', '<ROOT>\\elsewhere\\saved.sqlite', False],  # 아니요
    ['restore', '<ROOT>\\elsewhere\\saved.sqlite', True],   # the ratings come back
    ['restore', '<ROOT>\\library\\catalog.sqlite', True],   # the catalogue itself: refused
    ['fs_move', '<ROOT>\\photos\\B', '<ROOT>\\photos\\B2'],  # a folder renamed outside Grainy
    ['integrity'],
    ['folder', None],
    ['relink_folder', None, None],                            # no folder chosen: refused
    ['folder', '<ROOT>\\photos\\B'],
    ['relink_folder', '<ROOT>\\photos\\B2', False],          # 아니요
    ['locate', '<ROOT>\\photos\\B', '<ROOT>\\photos\\Renamed', True],   # no photos there: refused
    ['locate', '<ROOT>\\photos\\B', '<ROOT>\\photos\\B2', True],
    ['folder', None],                                         # every photo listed again
    ['open', 1],
    ['select', [1]],
    ['trash', False],                                         # 아니요
    ['trash', True],                                          # a1_001.png to the bin; the virtual copy 7 goes too
    ['open', 2],
    ['fs_move', '<ROOT>\\photos\\Renamed\\a2_002.png', '<ROOT>\\spare\\gone.png'],
    ['select', [2, 5]],
    ['trash', True],                                          # a2 is missing (only its row goes), copy.png to the bin
]

with tempfile.TemporaryDirectory(prefix='grainy-app-files-', ignore_cleanup_errors=True) as temporary:
    ROOT = Path(temporary).resolve()
    catalog, steps = run_script(SCRIPT)
    report = catalog_maintenance.optimize(ROOT / 'library')
    optimize = {k: report[k] for k in ('converted', 'examined', 'total', 'profiles', 'cancelled', 'integrity')}
    optimize['backups'] = backups(ROOT)
    with Case('app', 'files_steps') as case:
        case.json(stamp=STAMP, steps=steps, script=json.dumps(SCRIPT, ensure_ascii=False), optimize=optimize,
                  files=FILES, library=LIBRARY)
    catalog.close()

with Case('app', 'files_inputs') as case:
    for name, data in INPUTS.items():
        case.array(name, np.frombuffer(data, np.uint8))
    case.json(names=list(INPUTS))

with tempfile.TemporaryDirectory(prefix='grainy-app-files-', ignore_cleanup_errors=True) as temporary:
    ROOT = Path(temporary).resolve()
    catalog = build(ROOT)
    window = Window(catalog)
    window.activate(3)
    window.selection = [3]
    window.set_setting('exposure', 0.4)
    window.commit()
    window.manager.smart_previews = LibraryManager.smart_previews.__get__(window.manager)
    window.manager.smart_previews()
    with np.load(ROOT / 'library' / 'previews' / '3.npz', allow_pickle=False) as data:
        pixels = data['pixels']
        info = json.loads(str(data['info']))
    with Case('app', 'files_offline') as case:
        case.array('pixels', pixels.astype(np.float32))
        case.json(dtype=str(pixels.dtype), shape=list(pixels.shape), keys=sorted(info),
                  width=info.get('width'), height=info.get('height'), working_space=info.get('working_space'))
    catalog.close()
