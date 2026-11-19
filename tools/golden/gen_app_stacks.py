"""Goldens of grainy-app work package 16 (stacks, snapshots and virtual copies): golden/app/stacks_*.

* `stacks_steps`: scripted sessions through the oracle's own methods `LibraryManager.stack`, `unstack`,
  `toggle_stacks`, `snapshot`, `restore_snapshot`, `virtual_copy` and `WorkflowTools.stack_cover`, `stack_move`
  (and `MainWindow.commit` for the edits between them), on the library of `golden/app/library`, with the input boxes
  answered from the script; after each step the stack order, the listing `library_query.query` gives (stacks open
  or collapsed), the open photo's settings and the catalogue tables.

The window is a stand-in with the attributes those methods read (the same as gen_app.py's, whose own file is not
touched). Nothing is written outside a temporary folder and `golden/app`. Deterministic: the clock of the catalogue
is the library's fixed stamp, and no clock, process id or random id is stored.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_stacks.py
"""
import json
import shutil
import sqlite3
import tempfile
import threading
from copy import deepcopy
from datetime import datetime
from pathlib import Path

from common import GOLDEN, Case

from luma import catalog as catalog_module, library_query, manager as manager_module, workflow_data  # noqa: E402
from luma import workflow_ui as workflow_module  # noqa: E402
from luma.app import MainWindow  # noqa: E402
from luma.catalog import Catalog  # noqa: E402
from luma.engine import defaults, normalized  # noqa: E402
from luma.manager import EDIT_GROUPS, LibraryManager  # noqa: E402
from luma.workflow_ui import WorkflowTools  # noqa: E402

START = GOLDEN / 'app' / 'library' / 'start.sqlite'
STAMP = '2026-11-13T01:02:03.456789+00:00'


class Clock(datetime):
    @classmethod
    def now(cls, tz=None):
        value = datetime.fromisoformat(STAMP)
        return value if tz else value.replace(tzinfo=None)


catalog_module.datetime = Clock


class Nop:
    def __getattr__(self, name):
        return Nop()

    def __call__(self, *args, **kwargs):
        return Nop()

    def __bool__(self):
        return False

    def __iter__(self):
        return iter(())


class Answers:
    """The input boxes: `QInputDialog.getText/getItem` answer from the script."""

    def __init__(self):
        self.queue = []
        self.shown = []

    def take(self):
        if not self.queue:
            raise AssertionError('a box opened that the script did not answer')
        return self.queue.pop(0)


ANSWERS = Answers()


class ScriptedInput:
    @staticmethod
    def getText(parent, title, label, *args, text='', **kwargs):
        ANSWERS.shown.append(dict(box='text', title=title, label=label, text=text))
        return ANSWERS.take()

    @staticmethod
    def getItem(parent, title, label, items, *args, editable=True, **kwargs):
        ANSWERS.shown.append(dict(box='item', title=title, label=label, items=list(items), editable=editable))
        index = ANSWERS.take()
        if index is None:
            return '', False
        return items[index], True


manager_module.QInputDialog = ScriptedInput


class StackAction:
    """The check mark of 스택 접어서 보기."""

    def __init__(self):
        self.checked = False

    def isChecked(self):
        return self.checked


class Manager:
    after_commit = LibraryManager.after_commit
    edit_photo = LibraryManager.edit_photo
    stack = LibraryManager.stack
    unstack = LibraryManager.unstack
    toggle_stacks = LibraryManager.toggle_stacks
    snapshot = LibraryManager.snapshot
    restore_snapshot = LibraryManager.restore_snapshot
    virtual_copy = LibraryManager.virtual_copy

    def __init__(self, window):
        self.w = window
        self.copy_keys = set(defaults()) - set(EDIT_GROUPS['크롭·회전']) - {'panels_off'}
        self.auto_sync = False
        self.syncing = False
        self.collapsed = False
        self.stack_action = StackAction()

    def refresh_thumbnails(self, ids):
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
        self.refreshes = []
        self.manager = Manager(self)
        for name in ('commit_timer', 'photo_model', 'view', 'before_button', 'auto_tone_note', 'copy_crop', 'tabs',
                     'preview_timer', 'refine_timer'):
            setattr(self, name, Nop())

    def statusBar(self):
        return Nop()

    def refresh_history(self):
        pass

    def load_controls(self):
        pass

    def render(self):
        pass

    def refresh_lists(self, *args, ensure_current=False):
        self.refreshes.append(bool(ensure_current))

    def selected_ids(self):
        return list(self.selection) or ([self.current_id] if self.current_id else [])

    def activate(self, photo_id):
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


class Flow:
    stack_cover = WorkflowTools.stack_cover
    stack_move = WorkflowTools.stack_move

    def __init__(self, window):
        self.w = window


TABLES = {'photos': ['id', 'name', 'stack_id', 'virtual_source', 'rating', 'flag', 'label', 'imported', 'edits'],
          'snapshots': ['id', 'photo_id', 'name', 'edits', 'created'],
          'history': ['id', 'photo_id', 'label']}


def digest(text):
    """FNV-1a (64 bit) of the UTF-8 text: a short stand-in for a stored settings text."""
    h = 0xcbf29ce484222325
    for byte in text.encode('utf-8'):
        h = ((h ^ byte) * 0x100000001b3) & 0xFFFFFFFFFFFFFFFF
    return format(h, '016x')


def tables(db):
    out = {}
    for table, columns in TABLES.items():
        cols = ','.join('"' + c + '"' for c in columns)
        rows = db.execute(f'SELECT {cols} FROM {table} ORDER BY id').fetchall()
        out[table] = [dict(zip(columns, row)) for row in rows]
        for row in out[table]:
            if 'edits' in row:   # the stored settings text, as a short digest: the goldens stay small
                row['edits'] = digest(row['edits'])
    out['stack_orders'] = db.execute("SELECT value FROM preferences WHERE key='stack_orders'").fetchone()
    out['stack_orders'] = out['stack_orders'][0] if out['stack_orders'] else None
    return out


def changed(settings):
    base = defaults()
    return {k: v for k, v in settings.items() if k not in base or v != base[k]}


def run_script(name, script):
    folder = work / name
    folder.mkdir()
    shutil.copyfile(START, folder / 'catalog.sqlite')
    catalog = Catalog(folder)
    window = Window(catalog)
    manager, flow = window.manager, Flow(window)
    window.flow = flow
    steps = []
    previous_tables = None
    for step in script:
        action, a = step[0], step[1:]
        ANSWERS.queue.clear()
        ANSWERS.shown.clear()
        window.refreshes.clear()
        error = None
        try:
            if action == 'open':
                window.activate(a[0])
            elif action == 'select':
                window.selection = list(a[0])
            elif action == 'set':
                window.set_setting(a[0], a[1])
            elif action == 'commit':
                window.commit()
            elif action == 'stack':
                manager.stack()
            elif action == 'unstack':
                manager.unstack()
            elif action == 'toggle':
                manager.stack_action.checked = not manager.stack_action.checked
                manager.toggle_stacks()
            elif action == 'cover':
                flow.stack_cover()
            elif action == 'move':
                flow.stack_move(a[0])
            elif action == 'snapshot':                       # the typed name, or None: the box cancelled
                ANSWERS.queue.append((a[0], a[0] is not None))
                manager.snapshot()
            elif action == 'restore':                        # the chosen row, or None: the box cancelled
                if a[0] != 'none_saved':
                    ANSWERS.queue.append(a[0])
                manager.restore_snapshot()
            elif action == 'virtual_copy':
                manager.virtual_copy()
            else:
                raise ValueError(action)
        except Exception as exc:   # noqa: BLE001 - the oracle's `run` shows the message in a box
            error = str(exc)
        catalog.db.commit()
        catalog.db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
        copy = work / f'{name}-query.sqlite'
        shutil.copyfile(folder / 'catalog.sqlite', copy)
        options = dict(search='', folder=None, recursive=True, mode='all', rules={}, collection=None,
                       collapsed=manager.collapsed, stack_orders=catalog.preference('stack_orders', {}),
                       sort='imported', descending=False)
        result = library_query.query(copy, options, threading.Event())
        now_tables = tables(sqlite3.connect(folder / 'catalog.sqlite'))
        order = workflow_data.stack_order(catalog, window.current_id) if window.current_id else []
        steps.append(dict(
            step=step, error=error, collapsed=manager.collapsed, refreshes=list(window.refreshes),
            boxes=list(ANSWERS.shown), ids=result['ids'], total=result['total'], current=window.current_id,
            current_order=order, changed=changed(window.settings),
            snapshots=[dict(id=s['id'], name=s['name']) for s in catalog.snapshots(window.current_id)]
            if window.current_id else [],
            **({} if now_tables == previous_tables else {'tables': now_tables})))
        previous_tables = now_tables
    catalog.close()
    return steps


SCRIPT = [
    # snapshots
    ['open', 1],
    ['set', 'exposure', 0.5],
    ['snapshot', 'A 노출'],                                  # commits the pending change first, then saves
    ['set', 'contrast', 20.0],
    ['snapshot', '  두번째  '],                              # the name is stripped
    ['snapshot', '   '],                                     # a blank name: nothing
    ['snapshot', None],                                      # cancelled
    ['restore', 1],                                          # the older one (newest first)
    ['restore', None],                                       # cancelled
    ['set', 'saturation', -10.0],
    ['restore', 0],                                          # commits the pending change, then restores
    ['open', 2],
    ['restore', 'none_saved'],                               # no snapshot: the oracle's error
    ['snapshot', '같은 이름'],
    ['snapshot', '같은 이름'],
    ['open', 1],
    # stacks (photos 4, 5, 6 are stacked already, 6 first)
    ['select', [1, 2, 3]],
    ['stack'],
    ['select', [7]],
    ['stack'],                                               # one photo: a stack of one
    ['select', [2]],
    ['unstack'],
    ['open', 3],
    ['cover'],
    ['open', 1],
    ['move', 1],
    ['move', -1],
    ['move', -5],                                            # clamped to the front
    ['move', 9],                                             # clamped to the back
    ['open', 2],
    ['cover'],                                               # not stacked: nothing
    ['move', 1],
    ['open', 5],
    ['move', -1],
    ['toggle'],                                              # 스택 접어서 보기
    ['open', 6],
    ['toggle'],
    ['toggle'],
    ['select', [4, 5, 6]],
    ['unstack'],                                             # the saved order stays behind, unused
    ['toggle'],
    ['select', [4, 5]],
    ['stack'],
    ['open', 4],
    ['cover'],
    ['toggle'],
    ['toggle'],                                              # open again: every photo can be opened
    # virtual copies
    ['open', 1],
    ['select', [1, 2]],
    ['virtual_copy'],
    ['open', 8],
    ['select', [8]],                                         # a copy of a virtual copy refers to the original
    ['virtual_copy'],
    ['open', 3],
    ['set', 'exposure', 1.0],
    ['select', [3]],
    ['virtual_copy'],                                        # commits the open photo's pending change first
]

with tempfile.TemporaryDirectory(prefix='grainy-app-stacks-', ignore_cleanup_errors=True) as temporary:
    work = Path(temporary).resolve()
    steps = run_script('main', SCRIPT)
    with Case('app', 'stacks_steps') as case:
        case.json(stamp=STAMP, steps=steps, script=json.dumps(SCRIPT, ensure_ascii=False))
