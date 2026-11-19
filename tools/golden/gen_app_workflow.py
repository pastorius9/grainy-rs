"""Goldens of grainy-app's batch workflow (APP_PLAN package 18, the 사진 작업 menu), made with the oracle's own code.

A small library is built with the Python Grainy's own `Catalog`: ten generated photos (`golden/app/workflow/photos`:
a landscape-like picture with casts and exposures, a flat grey one, one with a film edge, two negatives of one
roll, one that is pure white, one whose file is gone) with their settings, metadata, extras and keywords. Scripted
sessions then run the oracle's own methods, unbound, on a stand-in for the window (the pattern of
`gen_app_sync.py`): `WorkflowTools.auto_tone` / `auto_color` / `auto_white` / `invert_negatives` / `invert_roll` /
`border_crop` / `update_process` / `match_exposure` / `save_metadata` / `apply_metadata` / `delete_metadata` /
`set_keyword` / `apply_keyword`, with `MainWindow.commit` / `refresh_command_edits` and `command_batch.apply`
underneath. The stand-in differs from the window in the places that are Qt widgets only: `command_auto_values`
computes with `photo_service.auto_values` (the oracle's own function, the same arguments the window passes) without
the progress box, `spawn` runs the work at once, `QProgressDialog` does nothing, and the dialogs are answered by the
script (the answers and what the dialogs showed are stored).

Stored per scenario and checkpoint: the catalogue as the actions leave it (settings written as the keys that differ
from the defaults, the history rows, the undo preferences, the extras, the keywords), the status messages, the
message boxes and the errors the actions raised. No clock, process id or random value is stored.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_workflow.py
"""
import json
import shutil
import sqlite3
import tempfile
import traceback
from copy import deepcopy
from datetime import datetime
from pathlib import Path
from threading import Event

from common import GOLDEN, Case, np, write_json  # noqa: F401
from PIL import Image
from PySide6.QtWidgets import QApplication, QCheckBox, QDialog, QLineEdit
from luma import catalog as catalog_module, command_batch, workflow_ui
from luma.app import MainWindow
from luma.catalog import Catalog
from luma.engine import DEFAULTS, defaults, normalized
from luma.workflow_ui import WorkflowTools

STAMP = '2026-11-13T01:02:03.456789+00:00'
TOKEN = '0123456789abcdef0123456789abcdef'
W, H = 120, 80

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


class Bar:
    def __init__(self):
        self.messages = []

    def showMessage(self, text, *args):
        self.messages.append(text)


class Studio:
    upright_generation = 0

    def cancel_stroke(self):
        pass


def scrubbed(text):
    """A load failure names a temporary file: its text is the system's own, not stored."""
    return '<load error>' if 'grainy-app-workflow-golden' in text else text


class Boxes:
    """The message boxes and error boxes the actions opened (their text), the dialogs' scripted answers and what
    the dialogs showed."""
    texts = []
    shown = []
    answers = {}


# ---- the dialogs the script answers -----------------------------------------------------------------------------------

class FakeSignal:
    def connect(self, *a, **k):
        pass

    def disconnect(self, *a, **k):
        pass


class FakeProgress:
    """`QProgressDialog` does nothing here: the work runs at once."""

    def __init__(self, *args, **kwargs):
        self.canceled = FakeSignal()

    def __getattr__(self, name):
        return lambda *a, **k: None


class FormDialog(QDialog):
    """`save_metadata`'s dialog: records what it shows, then ticks and fills what the script says."""

    def __init__(self, parent=None):
        super().__init__(None)

    def exec(self):
        boxes = self.findChildren(QCheckBox)
        lines = self.findChildren(QLineEdit)
        name, values = lines[0], lines[1:]
        Boxes.shown.append({'title': self.windowTitle(), 'name': name.text(),
                            'rows': [[b.text(), b.isChecked(), v.text()] for b, v in zip(boxes, values)]})
        answer = Boxes.answers.get('form')
        if answer is None:
            return QDialog.DialogCode.Rejected
        name.setText(answer.get('name', ''))
        for box, value in zip(boxes, values):
            if answer.get('ticked') is not None:
                box.setChecked(box.text() in answer['ticked'])
            if box.text() in answer.get('texts', {}):
                value.setText(answer['texts'][box.text()])
        return QDialog.DialogCode.Accepted


class Inputs:
    """`QInputDialog.getItem` / `getText` answered by the script."""

    @staticmethod
    def getItem(parent, title, label, items, editable=False):
        Boxes.shown.append({'item_box': [title, label, list(items)]})
        choice = Boxes.answers.get('item')
        return (choice, True) if choice is not None else ('', False)

    @staticmethod
    def getText(parent, title, label, text=''):
        Boxes.shown.append({'text_box': [title, label, text]})
        answer = Boxes.answers.get('text')
        return (answer, True) if answer is not None else ('', False)


workflow_ui.QProgressDialog = FakeProgress
workflow_ui.QDialog = FormDialog
workflow_ui.QInputDialog = Inputs


# ---- the window --------------------------------------------------------------------------------------------------------

class Manager:
    after_commit = staticmethod(lambda previous: None)
    copy_keys = set()

    def __init__(self, window):
        self.w = window

    def run(self, fn):
        """`LibraryManager.run`: an exception is a message box."""
        try:
            fn()
        except Exception as error:
            Boxes.texts.append(scrubbed(str(error)))

    def refresh_thumbnails(self, ids):
        pass


class Window:
    """The state the actions read, with the window's own methods."""

    commit = MainWindow.commit
    save_undo = MainWindow.save_undo
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
        self.source = object()
        self.render_version = 0
        self.bar = Bar()
        self.studio = Studio()
        self.manager = Manager(self)
        self.cancel_next = False
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
        return sorted(self.selection) or ([self.current_id] if self.current_id else [])

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

    def command_auto_values(self, records, compute=None, name='자동톤'):
        """`MainWindow.command_auto_values` without the progress box: the same call of `photo_service.auto_values`
        and the same messages."""
        from luma import photo_service
        from luma.engine import auto_tone_settings, load_image
        compute = compute or (lambda source, settings: auto_tone_settings(source, settings)[0])
        cancel = Event()
        if self.cancel_next:
            cancel.set()
            self.cancel_next = False
        try:
            values, _ = photo_service.auto_values(records, compute, self.catalog.directory, cancel=cancel,
                                                  strict=True, loader=lambda *a, **k: load_image(*a, **k))
        except Exception:
            raise ValueError('일부 사진을 읽거나 계산할 수 없습니다. 변경한 사진은 없습니다.')
        if values is None:
            raise ValueError(f'일괄 {name}을 취소했습니다. 변경한 사진은 없습니다.')
        return values

    def spawn(self, work, done, failed=None):
        """Runs the work at once (the window's thread pool)."""
        try:
            result = work()
        except Exception:
            failed(traceback.format_exc())
            return
        done(result)

    def show_error(self, message):
        """`MainWindow.show_error`: the box shows the last line of the message."""
        self.statusBar().showMessage('처리하지 못했습니다. 자세한 내용을 확인해 주세요.')
        Boxes.texts.append(scrubbed(message.strip().splitlines()[-1]))


# ---- the library -------------------------------------------------------------------------------------------------------

def scene():
    """A small landscape-like picture, linear-ish values 0..1: sky gradient, a sun, a dark building with windows."""
    y, x = np.mgrid[0:H, 0:W].astype(np.float32)
    u, v = x / (W - 1), y / (H - 1)
    top, bottom = np.array([.30, .50, .85], np.float32), np.array([.80, .70, .55], np.float32)
    image = (1 - v)[..., None] * top + v[..., None] * bottom
    image = image + (np.exp(-(np.hypot(x - 90, y - 20) / 9) ** 2) * .55)[..., None] * np.array([1, .9, .6], np.float32)
    image[44:76, 16:54] = np.array([.07, .07, .09], np.float32)
    for column in range(20, 50, 8):
        image[50:58, column:column + 4] = np.array([.9, .8, .4], np.float32)
    image[60:80, :] = image[60:80, :] * (.55 + .45 * np.sin(u[60:80, :, None] * 40) ** 2)
    return np.clip(image, 0, 1)


def encode(image):
    return (np.clip(image, 0, 1) * 255 + .5).astype(np.uint8)


def picture(kind):
    base = scene()
    if kind == 'dusk':
        return np.clip(.15 + (base - .15) * .6, 0, 1) * np.array([.85, 1.0, 1.25], np.float32)
    if kind == 'warm':
        return base * np.array([1.12, 1.0, .78], np.float32)
    if kind == 'flat':
        return np.full((H, W, 3), .5, np.float32)
    if kind == 'bordered':
        image = base.copy()
        image[:4, :] = 0
        image[-4:, :] = 0
        image[:, :4] = 0
        image[:, -4:] = 0
        return image
    if kind == 'plain':
        return base
    if kind in ('neg1', 'neg2'):
        scene_ = base if kind == 'neg1' else base[:, ::-1] * .85
        return np.clip((1 - scene_ * .9) * np.array([.92, .62, .40], np.float32), 0, 1)
    if kind == 'old':
        return base * .8
    if kind == 'white':
        return np.ones((H, W, 3), np.float32)
    raise AssertionError(kind)


EXPOSURE = {'shutter': '1/125', 'iso': 100, 'aperture': '4'}

# (file, picture, settings given to Catalog.add, metadata, extras, keywords)
PHOTOS = [
    ('dusk.png', 'dusk', {'exposure': .3}, {'shutter': '1/60', 'iso': 400, 'aperture': '2.8'},
     {'creator': 'Sample Creator', 'copyright': '2026 Sample', 'title': 'Dusk'}, '풍경, 저녁'),
    ('warm.png', 'warm', {}, {'shutter': '1/250', 'iso': 200, 'aperture': '5.6'}, {}, ''),
    ('flat.png', 'flat', {}, {'camera': 'Test Camera'}, {}, ''),
    ('bordered.png', 'bordered', {'contrast': 12.0}, dict(EXPOSURE), {}, '스캔'),
    ('plain.png', 'plain', {}, {'shutter': 0.004, 'iso': 100, 'aperture': 8}, {}, ''),
    ('neg1.png', 'neg1', {}, {}, {}, '필름'),
    ('neg2.png', 'neg2', {}, {}, {}, '필름'),
    ('old.png', 'old', {'tone_version': 1, 'detail_version': 2, 'saturation': 8.0}, dict(EXPOSURE), {}, ''),
    ('gone.png', 'plain', {}, dict(EXPOSURE), {}, ''),
    ('white.png', 'white', {}, {}, {}, ''),
]


def build_library(work):
    photos = work / 'photos'
    photos.mkdir()
    out = GOLDEN / 'app' / 'workflow' / 'photos'
    shutil.rmtree(out, ignore_errors=True)
    out.mkdir(parents=True)
    built = work / 'built'
    c = Catalog(built)
    for name, kind, settings, info, extras, keywords in PHOTOS:
        Image.fromarray(encode(picture(kind))).save(photos / name)
        if name != 'gone.png':
            shutil.copyfile(photos / name, out / name)
        ident = c.add(photos / name, settings=normalized(settings), metadata={'format': 'png', 'width': W, 'height': H, **info})
        with c.db:
            c.db.execute('UPDATE photos SET extras=?,keywords=? WHERE id=?', (json.dumps(extras), keywords, ident))
    (photos / 'gone.png').unlink()
    c.db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
    c.close()
    return built, photos


# ---- what a checkpoint stores --------------------------------------------------------------------------------------------

def sparse(settings):
    changed = {k: v for k, v in settings.items() if k not in DEFAULTS or v != DEFAULTS[k]}
    return {'keys': len(settings), 'changed': changed}


def project(db):
    photos = {}
    for ident, edits, extras, keywords in db.execute('SELECT id,edits,extras,keywords FROM photos ORDER BY id'):
        photos[str(ident)] = {'settings': sparse(json.loads(edits)), 'extras': json.loads(extras or '{}'), 'keywords': keywords}
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


# ---- the scripted sessions -------------------------------------------------------------------------------------------------

def run_step(window, step):
    """One scripted action through the oracle's own methods (menu actions run through `manager.run`)."""
    action, a = step[0], step[1:]
    flow = window.workflow
    if action == 'open':
        window.activate(a[0])
    elif action == 'set':
        window.set_setting(a[0], a[1])
    elif action == 'commit':
        window.commit(*a)
    elif action == 'select':
        # The window's selection always holds the open photo: the first of the list is opened, the rest join it.
        window.activate(a[0][0])
        window.selection = list(a[0])
    elif action == 'pref':
        window.catalog.save_preference(a[0], a[1])
    elif action == 'cancel_next':
        window.cancel_next = True
    elif action in ('auto_tone', 'auto_color', 'auto_white', 'invert_negatives', 'invert_roll', 'border_crop',
                    'update_process', 'match_exposure', 'apply_metadata', 'delete_metadata', 'set_keyword',
                    'apply_keyword', 'save_metadata'):
        Boxes.answers = {}
        if action == 'save_metadata':
            Boxes.answers = {'form': a[0] if a else None}
        elif action in ('apply_metadata', 'delete_metadata'):
            Boxes.answers = {'item': a[0]}
        elif action == 'set_keyword':
            Boxes.answers = {'text': a[0]}
        elif action == 'apply_keyword':
            Boxes.answers = {'text': a[0] if a else None}
        window.manager.run(getattr(flow, action))
    else:
        raise AssertionError(action)


def run_script(folder, script):
    c = Catalog(folder)
    window = Window(c)
    window.workflow = WorkflowTools.__new__(WorkflowTools)
    window.workflow.w = window
    window.workflow.previous_id = None
    window.workflow.cancel = None
    window.workflow.windows = []
    steps = []
    checkpoints = {}
    for step in script:
        if step[0] == 'check':
            window.commit()
            c.db.commit()
            checkpoints[step[1]] = project(c.db)
            continue
        before = len(window.bar.messages)
        Boxes.texts = []
        Boxes.shown = []
        run_step(window, step)
        steps.append({'step': step, 'messages': window.bar.messages[before:], 'boxes': Boxes.texts, 'shown': Boxes.shown})
    c.close()
    return steps, checkpoints


FORM_ALL = ['제목', '설명', '촬영자', '저작권', '장소', '도시', '국가', '위도', '경도', '촬영 시각', '키워드']

BATCH = [
    # The open photo has a pending edit: it is stored first (one history row), then the batch (one more).
    ['open', 1], ['set', 'contrast', 20.0], ['select', [1, 2, 3]], ['auto_tone'], ['check', 'tone'],
    ['select', [1, 2, 4]], ['auto_color'], ['check', 'color'],
    ['pref', 'auto_color_amount', 60], ['select', [2, 5]], ['auto_color'], ['check', 'color_60'],
    ['select', [1, 2, 5]], ['auto_white'], ['check', 'white'],
    ['select', [10]], ['auto_white'], ['check', 'white_refused'],
    # All or nothing: one photo that cannot be read, a cancel.
    ['select', [4, 9]], ['auto_tone'], ['check', 'tone_unreadable'],
    ['select', [4, 5]], ['cancel_next'], ['auto_tone'], ['check', 'tone_cancelled'],
    ['select', [4, 5]], ['cancel_next'], ['invert_negatives'], ['check', 'negative_cancelled'],
]

NEGATIVES = [
    ['select', [6, 7]], ['invert_negatives'], ['check', 'negatives'],
    ['select', [6, 7]], ['invert_roll'], ['check', 'roll'],
    ['select', [6]], ['invert_roll'], ['check', 'roll_one'],
    ['select', [6, 7, 1]], ['invert_roll'], ['check', 'roll_three'],
]

BORDER = [
    ['select', [4, 5, 1]], ['border_crop'], ['check', 'crop'],
    ['select', [5, 1]], ['border_crop'], ['check', 'crop_none'],
    ['select', [1, 9]], ['border_crop'], ['check', 'crop_unreadable'],
]

PROCESS = [
    # The open photo is one of them and has a pending edit.
    ['open', 8], ['set', 'contrast', -15.0], ['select', [8, 1, 5]], ['update_process'], ['check', 'process'],
    ['select', [8, 1]], ['update_process'], ['check', 'process_again'],
]

EXPOSURE_STEPS = [
    ['open', 1], ['select', [1, 2, 3, 4]], ['match_exposure'], ['check', 'exposure_missing'],
    ['select', [1]], ['match_exposure'], ['check', 'exposure_alone'],
    ['select', [1, 2, 4, 5]], ['match_exposure'], ['check', 'exposure'],
    ['open', 5], ['select', [5, 8]], ['match_exposure'], ['check', 'exposure_from_5'],
]

METADATA = [
    ['open', 1], ['select', [1]],
    ['save_metadata', {'name': ''}], ['check', 'preset_no_name'],
    ['save_metadata', {'name': 'Empty', 'ticked': []}], ['check', 'preset_nothing'],
    ['save_metadata', {'name': 'Bad GPS', 'ticked': ['위도'], 'texts': {'위도': 'north'}}],
    ['save_metadata', {'name': 'Far GPS', 'ticked': ['경도'], 'texts': {'경도': '181'}}],
    ['save_metadata', {'name': 'Bad time', 'ticked': ['촬영 시각'], 'texts': {'촬영 시각': '2026-11-13'}}],
    ['check', 'preset_invalid'],
    ['save_metadata'], ['check', 'preset_cancelled'],
    ['save_metadata', {'name': '  Default  '}], ['check', 'preset_default'],
    ['save_metadata', {'name': 'Place', 'ticked': ['장소', '도시', '국가', '위도', '경도', '촬영 시각', '키워드'],
                       'texts': {'장소': 'Harbour', '도시': 'Sample City', '국가': 'Sampleland', '위도': '35.5',
                                 '경도': '-139.25', '촬영 시각': '2026:11:13 10:20:30', '키워드': '여행, 항구'}}],
    ['check', 'preset_place'],
    ['select', [2, 3]], ['apply_metadata', 'Place'], ['check', 'apply_place'],
    ['select', [4]], ['apply_metadata', 'Default'], ['check', 'apply_default'],
    ['apply_metadata', None], ['check', 'apply_cancelled'],
    ['delete_metadata', 'Default'], ['check', 'deleted'],
    ['delete_metadata', 'Place'], ['apply_metadata', 'Place'], ['check', 'none_left'],
]

KEYWORDS = [
    ['open', 1], ['select', [1, 2]],
    ['apply_keyword', None], ['check', 'asked_cancelled'],
    ['apply_keyword', 'a, b'], ['check', 'asked_comma'],
    ['apply_keyword', '   '], ['check', 'asked_blank'],
    ['apply_keyword', ' 여행|일본 '], ['check', 'asked'],
    ['select', [2, 3]], ['apply_keyword'], ['check', 'applied'],
    ['set_keyword', None], ['set_keyword', '풍경'], ['check', 'set'],
    ['select', [1, 2, 3]], ['apply_keyword'], ['check', 'applied_again'],
    ['set_keyword', 'x, y'], ['check', 'set_refused'],
]

with tempfile.TemporaryDirectory(prefix='grainy-app-workflow-golden-', ignore_cleanup_errors=True) as temporary:
    work = Path(temporary).resolve()
    built, photos = build_library(work)
    out = GOLDEN / 'app' / 'workflow'
    out.mkdir(parents=True, exist_ok=True)

    results = {}
    scripts = (('batch', BATCH), ('negatives', NEGATIVES), ('border', BORDER), ('process', PROCESS),
               ('exposure', EXPOSURE_STEPS), ('metadata', METADATA), ('keywords', KEYWORDS))
    for name, script in scripts:
        session = work / name
        shutil.copytree(built, session)
        steps, checkpoints = run_script(session, script)
        results[name] = {'script': script, 'steps': steps, 'checkpoints': checkpoints}

    begin = sqlite3.connect(built / 'catalog.sqlite')
    start = project(begin)
    begin.close()
    library = [{'name': name, 'settings': settings, 'info': {'format': 'png', 'width': W, 'height': H, **info},
                'extras': extras, 'keywords': keywords, 'file': name != 'gone.png'}
               for name, _, settings, info, extras, keywords in PHOTOS]
    with Case('app', 'workflow') as case:
        case.json(stamp=STAMP, token=TOKEN, library=library, start=start, form_fields=FORM_ALL, sessions=results)
