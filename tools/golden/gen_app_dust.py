"""Goldens of grainy-app's dust detection dialogs (APP_PLAN package 25), made with the oracle's own code.

Three small photos with dust spots and a hair (`golden/app/dust/*.png`, drawn here) and one photo in another folder
are added to a library built with the Python Grainy's own `Catalog`. Then the oracle's `DustDialog` and
`DustBatchDialog` run headless (offscreen Qt) on a stand-in for the window: jobs run at once (`spawn` calls the work
and its callback), `QTimer.singleShot` of the batch dialog runs at once, the review dialog's `exec` runs a scripted
review and accepts, `QMessageBox.question` answers Yes. The clocks, the session ids and the detection time are
fixed. Every scripted step stores what the dialog shows (texts, check states, enabled controls), the spots, the
operation and review a dialog answers, the catalogue rows (settings as the keys that differ from the defaults,
history, preferences) and the dust review tables (payloads unpacked).

Paths are stored with the photos' folder as `$ROOT`; file times are fixed (`MTIME_NS`) so the source signatures
compare equal.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_dust.py
"""
import json
import os
import shutil
import tempfile
import zlib
from copy import deepcopy
from datetime import datetime
from pathlib import Path

from common import GOLDEN, Case, np  # noqa: F401
import cv2
from PySide6.QtCore import Qt
from PySide6.QtWidgets import QApplication, QDialog, QMessageBox, QWidget
from luma import catalog as catalog_module, command_batch, dust, dust_dialog, dust_batch_dialog, dust_store
from luma.app import MainWindow
from luma.catalog import Catalog
from luma.engine import DEFAULTS, defaults, normalized

STAMP = '2026-11-13T01:02:03.456789+00:00'
TOKEN = '0123456789abcdef0123456789abcdef'
MTIME_NS = 1_700_000_000_000_000_000
OUT = GOLDEN / 'app' / 'dust'

app = QApplication.instance() or QApplication([])


class Clock(datetime):
    @classmethod
    def now(cls, tz=None):
        value = datetime.fromisoformat(STAMP)
        return value if tz else value.replace(tzinfo=None)


class Ids:
    """`uuid4()` with a counter: session n has the id `n` as 32 hex digits."""
    count = 0

    def __call__(self):
        Ids.count += 1
        return type('Uuid', (), {'hex': f'{Ids.count:032x}'})()


catalog_module.datetime = Clock
command_batch.datetime = Clock
dust_store.datetime = Clock
dust_store.uuid4 = Ids()
dust.perf_counter = lambda: 0.0


class Immediate:
    """`QTimer` of the batch dialog: `singleShot(0, f)` runs `f` now."""

    @staticmethod
    def singleShot(_ms, function):
        function()


dust_batch_dialog.QTimer = Immediate
dust_batch_dialog.QMessageBox.question = staticmethod(lambda *a, **k: QMessageBox.StandardButton.Yes)


# ---- the photos -----------------------------------------------------------------------------------------------------

def draw(kind):
    """A small 8-bit photo: a smooth colour gradient with dark and bright dust and, for some, a hair."""
    h, w = 120, 160
    y, x = np.mgrid[:h, :w].astype(np.float32)
    base = {'a': (0.55, 0.45, 0.35), 'b': (0.35, 0.5, 0.6), 'c': (0.6, 0.6, 0.5), 'd': (0.4, 0.4, 0.4)}[kind]
    image = np.zeros((h, w, 3), np.float32)
    for c in range(3):
        image[..., c] = base[c] + (x / w - 0.5) * 0.12 + (y / h - 0.5) * 0.06 * (c + 1)
    image = np.clip(image * 255 + 0.5, 0, 255).astype(np.uint8)
    spots = {
        'a': [(30, 30, 3, 20), (100, 40, 2, 235), (60, 90, 4, 15), (130, 95, 2, 240), (20, 100, 2, 25)],
        'b': [(40, 50, 3, 230), (120, 30, 2, 20), (90, 80, 3, 18)],
        'c': [(50, 60, 2, 20), (110, 70, 3, 235)],
        'd': [(80, 60, 3, 20)],
    }[kind]
    for cx, cy, r, v in spots:
        cv2.circle(image, (cx, cy), r, (v, v, v), -1)
    if kind == 'a':
        cv2.line(image, (10, 70), (150, 78), (30, 30, 30), 1)
    if kind == 'c':
        cv2.line(image, (20, 20), (60, 110), (230, 230, 230), 1)
    return image


NAMES = {'a': 'roll/a.png', 'b': 'roll/b.png', 'c': 'roll/c.png', 'd': 'other/d.png'}


def write_photos(root):
    for kind, name in NAMES.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        if not (OUT / f'{kind}.png').exists():
            OUT.mkdir(parents=True, exist_ok=True)
            cv2.imwrite(str(OUT / f'{kind}.png'), draw(kind)[..., ::-1])
        shutil.copyfile(OUT / f'{kind}.png', path)
        os.utime(path, ns=(MTIME_NS, MTIME_NS))


EDITS = {1: [('exposure', 0.3)], 2: [('contrast', 10.0)], 3: [], 4: []}


# ---- the stand-in window --------------------------------------------------------------------------------------------

class Nop:
    def __getattr__(self, name):
        return Nop()

    def __call__(self, *args, **kwargs):
        return Nop()

    def __bool__(self):
        return False


class Bar:
    def __init__(self):
        self.messages = []

    def showMessage(self, text, *args):
        self.messages.append(text)


class Target:
    profile = None

    def __init__(self):
        self.changed = Nop()


class Display:
    def watch(self, window):
        return Target()

    def release(self, target):
        pass


class Window(QWidget):
    commit = MainWindow.commit
    save_undo = MainWindow.save_undo
    set_setting = MainWindow.set_setting
    refresh_command_edits = MainWindow.refresh_command_edits

    def __init__(self, catalog):
        super().__init__()
        self.catalog = catalog
        self.current_id = None
        self.load_version = 0
        self.settings = defaults()
        self.last_saved = defaults()
        self.undo_stack = []
        self.redo_stack = []
        self.selection = []
        self.folder_filter = None
        self.bar = Bar()
        self.display_color = Display()
        self.errors = []
        self.source = object()
        self.render_version = 0
        for name in ('commit_timer', 'photo_model', 'studio', 'before_button', 'view', 'manager', 'history_list',
                     'tabs', 'auto_tone_note', 'preview_timer', 'refine_timer'):
            setattr(self, name, Nop())

    def statusBar(self):
        return self.bar

    def refresh_history(self):
        pass

    def load_controls(self):
        pass

    def render(self):
        pass

    def selected_ids(self):
        return list(self.selection) or ([self.current_id] if self.current_id else [])

    def spawn(self, work, ready, failed=None):
        try:
            result = work()
        except Exception as error:   # the window's job_finished
            if failed:
                failed(str(error))
            else:
                self.errors.append(str(error))
            return
        ready(result)

    def activate(self, photo_id):
        self.commit()
        self.current_id = photo_id
        self.load_version += 1
        photo = self.catalog.photo(photo_id)
        self.settings = deepcopy(photo['settings'])
        self.last_saved = deepcopy(self.settings)
        state = self.catalog.preference(f'undo:{photo_id}', {})
        self.undo_stack = [normalized(s) for s in state.get('undo', [])]
        self.redo_stack = [normalized(s) for s in state.get('redo', [])]
        self.selection = [photo_id]


def build_library(folder, root):
    c = Catalog(folder)
    for kind, name in NAMES.items():
        c.add(str(root / name), settings=None, metadata={'format': 'png', 'width': 160, 'height': 120})
    window = Window(c)
    for ident, changes in EDITS.items():
        window.activate(ident)
        for key, value in changes:
            window.set_setting(key, value)
        window.commit()
    window.commit()
    return c, window


# ---- what a step stores ---------------------------------------------------------------------------------------------

class Rooted:
    root = ''


def rooted(value):
    """Every string with the photos' folder replaced by `$ROOT`; floats rounded to 9 digits."""
    if isinstance(value, dict):
        return {str(k): rooted(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [rooted(v) for v in value]
    if isinstance(value, str):
        return value.replace(Rooted.root, '$ROOT')
    if isinstance(value, (np.floating, float)):
        return round(float(value), 9)
    if isinstance(value, np.integer):
        return int(value)
    return value


def sparse(settings):
    changed = {k: v for k, v in settings.items() if k not in DEFAULTS or v != DEFAULTS[k]}
    return {'keys': len(settings), 'changed': changed}


def project(db):
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
        if key == 'undo:dust_batch':
            value = {**value, 'records': [{**r, 'before': sparse(r['before']), 'after': sparse(r['after']),
                                           'undo_before': {k: [sparse(s) for s in v] for k, v in r['undo_before'].items()},
                                           'undo_after': {k: [sparse(s) for s in v] for k, v in r['undo_after'].items()}}
                                          for r in value['records']]}
        preferences[key] = value
    sessions = [dict(zip(('id', 'version', 'target_key', 'scope', 'snapshot', 'options', 'created', 'updated',
                          'current_index', 'revision'), row))
                for row in db.execute('SELECT id,version,target_key,scope,snapshot,options,created,updated,'
                                      'current_index,revision FROM dust_sessions ORDER BY id')]
    for s in sessions:
        s['snapshot'] = json.loads(s['snapshot'])
        s['options'] = json.loads(s['options'])
        s['target_key'] = len(s['target_key'])
    entries = []
    for session, ordinal, photo, path, state, payload in db.execute(
            'SELECT session_id,ordinal,photo_id,path,state,payload FROM dust_entries ORDER BY session_id,ordinal'):
        value = json.loads(zlib.decompress(payload).decode('utf-8'))
        value['record']['settings'] = sparse(value['record']['settings'])
        entries.append({'session': session, 'ordinal': ordinal, 'photo': photo, 'path': path, 'state': state,
                        'payload': value})
    return rooted({'photos': photos, 'history': history, 'preferences': preferences,
                   'sessions': sessions, 'entries': entries})


def checked(item):
    return item.checkState() == Qt.CheckState.Checked


def dialog_state(d):
    names = ('sensitivity', 'minimum', 'maximum', 'polarity', 'detailed', 'soft', 'suppress_grain', 'reduce_patterns',
             'scratches', 'scratch_width', 'scan_button', 'stop_button', 'add_mode', 'stroke_mode', 'scale',
             'more_button', 'apply_button')
    return rooted({
        'title': d.windowTitle(),
        'options': {'sensitivity': d.sensitivity.value(), 'minimum': d.minimum.value(), 'maximum': d.maximum.value(),
                    'polarity': d.polarity.currentData(), 'detailed': d.detailed.isChecked(),
                    'reduce_patterns': d.reduce_patterns.isChecked(), 'soft': d.soft.isChecked(),
                    'suppress_grain': d.suppress_grain.isChecked(), 'scratches': d.scratches.isChecked(),
                    'scratch_width': d.scratch_width.value()},
        'enabled': {n: getattr(d, n).isEnabled() for n in names},
        'list': [[d.list.item(i).text(), checked(d.list.item(i))] for i in range(d.list.count())],
        'current': d.list.currentRow(),
        'count': d.count.text(),
        'apply': d.apply_button.text(),
        'more': [not d.more_button.isHidden(), d.more_button.text()],
        'status': d.status.text(),
        'scale': d.scale.value(),
        'manual_size': d.manual_size.value(),
        'method': d.repair_method.currentData(),
        'add_mode': d.add_mode.isChecked(),
        'stroke_mode': d.stroke_mode.isChecked(),
        'view_current': d.view.current,
        'checked': sorted(d.view.checked),
        'spots': d.view.spots,
        'pending': d.pending_spots,
        'has_result': d.result is not None,
        'preview_note': d.preview_note.text(),
    })


def batch_state(b):
    names = ('work_selector', 'new_button', 'forget_button', 'save_button', 'scope', 'sensitivity', 'minimum', 'maximum',
             'polarity', 'detailed', 'soft', 'suppress_grain', 'reduce_patterns', 'scratches', 'scratch_width',
             'scan_button', 'stop_button', 'retry_button', 'review_button', 'next_button', 'apply_button',
             'undo_button', 'redo_button')
    rows = []
    for i in range(b.list.topLevelItemCount()):
        item = b.list.topLevelItem(i)
        rows.append([item.text(c) for c in range(4)] + [item.toolTip(0), item.toolTip(1)])
    return rooted({
        'rows': rows,
        'current': b.current_index(),
        'scope': b.scope.currentData(),
        'scope_note': b.scope_note.text(),
        'summary': b.summary.text(),
        'status': b.status.text(),
        'save_note': b.save_note.text(),
        'scan_button': b.scan_button.text(),
        'stop_button': b.stop_button.text(),
        'apply_button': b.apply_button.text(),
        'enabled': {n: getattr(b, n).isEnabled() for n in names},
        'save_visible': not b.save_button.isHidden(),
        'works': [[b.work_selector.itemText(i), b.work_selector.itemData(i)] for i in range(b.work_selector.count())],
        'work': b.work_selector.currentIndex(),
        'session': b.session_id,
        'revision': b.revision,
        'progress': [b.progress.minimum(), b.progress.maximum(), b.progress.value()],
        'snapshot': b.snapshot,
        'states': [e['state'] for e in b.entries],
        'options': {'sensitivity': b.sensitivity.value(), 'maximum': b.maximum.value(),
                    'scratches': b.scratches.isChecked()},
    })


# ---- the scripted review of the batch dialog --------------------------------------------------------------------

class Review:
    script = []
    states = []


def scripted_exec(self):
    """`DustDialog.exec` of a batch review: detect (cached), run the script, accept."""
    self.scan()
    Review.states.append(dialog_state(self))
    for step in Review.script:
        run_dialog_step(self, step)
    self.apply_selection()
    Review.states.append(dialog_state(self))
    return QDialog.DialogCode.Accepted


dust_dialog.DustDialog.exec = scripted_exec


def run_dialog_step(d, step):
    action, a = step[0], step[1:]
    if action == 'toggle':
        d.toggle_candidate(a[0])
    elif action == 'row':
        d.list.setCurrentRow(a[0])
    elif action == 'scale':
        d.scale.setValue(a[0])
    elif action == 'all':
        d.select_all(a[0])
    elif action == 'add':
        d.add_candidate(a[0], a[1])
    elif action == 'size':
        d.manual_size.setValue(a[0])
    elif action == 'stroke':
        d.add_stroke(a[0])
    elif action == 'method':
        d.repair_method.setCurrentIndex(d.repair_method.findData(a[0]))
    elif action == 'sensitivity':
        d.sensitivity.setValue(a[0])
    elif action == 'scratches':
        d.scratches.setChecked(a[0])
    elif action == 'detailed':
        d.detailed.setChecked(a[0])
    elif action == 'scan':
        d.scan()
    elif action == 'show_spots':
        d.show_spots.setChecked(a[0])
    else:
        raise AssertionError(action)


# ---- the single photo dialog ----------------------------------------------------------------------------------------

PREFERENCE = {'sensitivity': 55, 'minimum': 3, 'maximum': 30, 'polarity': 'both', 'detailed': False,
              'scratches': True, 'scratch_width': 8, 'unknown': 1}

SINGLE = [
    ['toggle', 0], ['row', 1], ['scale', 150], ['all', False], ['all', True], ['toggle', 2],
    ['add', 0.25, 0.5], ['size', 6], ['stroke', [[0.1, 0.1], [0.2, 0.15], [0.3, 0.12]]], ['method', 'smooth'],
]

captured = []
_qimage = dust_dialog.qimage


def capture(rgb, *args, **kwargs):
    captured.append(np.clip(np.asarray(rgb, np.float32) * 255 + 0.5, 0, 255).astype(np.uint8))
    return _qimage(rgb, *args, **kwargs)


dust_dialog.qimage = capture


def single(work):
    c, w = open_library(work)
    c.save_preference('dust_detection_options', PREFERENCE)
    w.activate(1)
    d = dust_dialog.DustDialog(w)
    states = {'initial': dialog_state(d)}
    d.scan()
    states['scanned'] = dialog_state(d)
    steps = []
    for step in SINGLE:
        run_dialog_step(d, step)
        steps.append({'step': step, 'state': dialog_state(d)})
    # The repair preview and the emphasis view (`start_preview` work, captured as the 8-bit picture).
    captured.clear()
    d.after.setChecked(True)
    d.start_preview()
    d.emphasis.setChecked(True)
    d.strength.setValue(70)
    d.start_preview()
    after, emphasis = captured[-2], captured[-1]
    d.apply_selection()
    operation = rooted(d.operation)
    review = rooted(d.review_state)
    # studio.detect_dust after the dialog accepted.
    label = f'먼지 {len(d.operation["spots"])}개 제거'
    values = deepcopy(w.settings['retouch'])
    values.append(d.operation)
    w.set_setting('retouch', values)
    w.commit(label)
    c.db.commit()
    result = {'states': states, 'steps': steps, 'operation': operation, 'review': review,
              'label': label, 'project': project(c.db)}
    # Changed options clear the list; a new detection with them.
    d2 = dust_dialog.DustDialog(w)
    d2.scan()
    second = {'scanned': dialog_state(d2)}
    run_dialog_step(d2, ['scratches', False])
    second['changed'] = dialog_state(d2)
    run_dialog_step(d2, ['sensitivity', 90])
    run_dialog_step(d2, ['detailed', True])
    run_dialog_step(d2, ['scan'])
    second['rescanned'] = dialog_state(d2)
    d2.reject()
    c.db.commit()
    result['second'] = second
    result['second_project'] = project(c.db)
    c.close()
    return result, after, emphasis


# ---- the batch dialog ------------------------------------------------------------------------------------------------

def open_library(work):
    c = Catalog(work / 'library')
    return c, Window(c)


def batch(work):
    c, w = open_library(work)
    w.activate(1)
    w.selection = [1, 3]
    out = []

    def keep(name, b, extra=None):
        c.db.commit()
        entry = {'name': name, 'state': batch_state(b), 'messages': list(w.bar.messages), 'errors': list(w.errors)}
        if extra:
            entry.update(extra)
        out.append(entry)

    def keep_tables(name):
        c.db.commit()
        out.append({'name': name, 'project': project(c.db)})

    b = dust_batch_dialog.DustBatchDialog(w)
    keep('opened', b)
    b.scope.setCurrentIndex(1)
    keep('folder_scope', b)
    b.start()
    keep('detected', b)
    keep_tables('detected_tables')
    b.list.setCurrentItem(b.list.topLevelItem(0))
    Review.script = [['toggle', 0], ['add', 0.75, 0.25], ['method', 'texture']]
    Review.states = []
    b.review_selected()
    keep('reviewed_first', b, {'review': Review.states})
    Review.script = []
    Review.states = []
    b.review_next()
    keep('reviewed_next', b, {'review': Review.states})
    keep_tables('reviewed_tables')
    b.apply()
    keep('applied', b)
    keep_tables('applied_tables')
    b.restore(False)
    keep('undone', b)
    keep_tables('undone_tables')
    b.restore(True)
    keep('redone', b)
    keep_tables('redone_tables')
    b.done(0)
    keep_tables('closed_tables')
    # Opened again on the folder: the saved work is loaded.
    w.folder_filter = str((work / 'photos' / 'roll'))
    b2 = dust_batch_dialog.DustBatchDialog(w)
    keep('reopened', b2)
    # Photo 3 changed outside the dialog: its review fails, a new detection recaptures it.
    edited = deepcopy(c.photo(3)['settings'])
    edited['exposure'] = 0.5
    c.edit(3, edited, '바깥 변경')
    b2.list.setCurrentItem(b2.list.topLevelItem(0))
    b2.review_next()
    keep('changed_source', b2)
    b2.retry()
    keep('retried', b2)
    b2.start()
    keep('detected_again', b2)
    keep_tables('detected_again_tables')
    b2.forget_work()
    keep('forgotten', b2)
    keep_tables('forgotten_tables')
    b2.done(0)
    c.close()
    return out


def main():
    with tempfile.TemporaryDirectory(prefix='grainy-dust-golden-', ignore_cleanup_errors=True) as temporary:
        work_root = Path(temporary).resolve()
        results = {}
        pictures = {}
        for name in ('single', 'batch'):
            work = work_root / name
            root = work / 'photos'
            write_photos(root)
            Rooted.root = str(root)
            c, _ = build_library(work / 'library', root)
            c.db.commit()
            start = project(c.db)
            c.close()
            Ids.count = 0
            if name == 'single':
                results[name], pictures['after'], pictures['emphasis'] = single(work)
            else:
                results[name] = batch(work)
            results[name + '_start'] = start
        with Case('app', 'dust') as case:
            case.array('after', pictures['after'])
            case.array('emphasis', pictures['emphasis'])
            case.json(stamp=STAMP, mtime_ns=MTIME_NS, names=NAMES, edits={str(k): v for k, v in EDITS.items()},
                      preference=PREFERENCE, single_script=SINGLE, **results)


main()
