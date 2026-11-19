"""Goldens of grainy-app work package 24 (import copy, watched folders and folder sync): golden/app/import_*.

* `import_steps`: a scripted session through the oracle's own code, on generated PNG files in a temporary folder:
  `import_copy.dialog(w)` (the real Qt dialog, offscreen; `QDialog.exec` fills its fields from the script and
  answers 복사하고 가져오기 or 취소, `QMessageBox.information` records the boxes), its job (`plan`, `execute`,
  verify, backup copy) and the import of the copies through `MainWindow.import_paths`, `import_next` and
  `apply_import_extras` (keywords and a develop preset); `FolderSync.scan` / `found` (등록한 폴더의 새 사진 자동으로
  가져오기) after a photo was removed from the catalogue and new files appeared; `ExtraTools.watch`,
  `watch_scan` and `unwatch` (현재 폴더 자동 가져오기 / 자동 가져오기 해제). After each step: the catalogue tables
  (paths relative to the temporary folder, written `<ROOT>`), every file on disk with the input its bytes are, the
  fields the dialog opened with, the problems and boxes shown, the status messages.
* `import_inputs`: the files (as uint8 arrays of their bytes) both sides write into their temporary folders, with
  their modification times.

The window is a stand-in QWidget with the attributes those methods read; jobs run at once (`spawn`), Qt's
zero-delay timers run in order after each step. Nothing is written outside a temporary folder and `golden/app`.
Deterministic: file times are fixed, the edit clock is fixed, and paths are stored relative to the temporary folder.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_import.py
"""
import io
import json
import os
import tempfile
from collections import deque
from datetime import datetime, timezone
from pathlib import Path
from threading import Event

from common import Case, np

from PySide6.QtWidgets import (QApplication, QCheckBox, QComboBox, QDialog, QFileDialog, QLineEdit,  # noqa: E402
                               QMessageBox, QWidget)

_qt = QApplication.instance() or QApplication([])

from luma import app as app_module, catalog as catalog_module  # noqa: E402
from luma import folder_sync as sync_module, import_copy  # noqa: E402
from luma.app import MainWindow  # noqa: E402
from luma.catalog import Catalog  # noqa: E402
from luma.engine import defaults, normalized  # noqa: E402
from luma.extras import ExtraTools  # noqa: E402
from luma.folder_sync import FolderSync  # noqa: E402
from PIL import Image  # noqa: E402

STAMP = '2026-11-13T01:02:03.456789+00:00'


class Clock(datetime):
    @classmethod
    def now(cls, tz=None):
        value = datetime.fromisoformat(STAMP)
        return value if tz else value.replace(tzinfo=None)


catalog_module.datetime = Clock

ROOT = None


def rel(text):
    """A path or a text with paths, relative to the temporary folder (path keys are lower case: `<root>`)."""
    if text is None:
        return None
    return str(text).replace(str(ROOT), '<ROOT>').replace(str(ROOT).lower(), '<root>')


def rel_value(value):
    if isinstance(value, str):
        return rel(value)
    if isinstance(value, list):
        return [rel_value(v) for v in value]
    if isinstance(value, dict):
        return {rel(k): rel_value(v) for k, v in value.items()}
    return value


def full(text):
    return str(text).replace('<ROOT>', str(ROOT))


class Nop:
    def __getattr__(self, name):
        return Nop()

    def __call__(self, *args, **kwargs):
        return Nop()

    def __bool__(self):
        return False


# ---- the boxes and Qt's timers ----------------------------------------------------------------------------------

BOXES = []
LATER = []
FORMS = deque()        # the attempts the next dialog answers
OPENED = []            # the fields each `exec` found
FOLDERS = deque()      # answers of QFileDialog.getExistingDirectory


def information(parent, title, text, *args, **kwargs):
    BOXES.append(dict(title=title, text=rel(text)))


def warning(parent, title, text, *args, **kwargs):
    BOXES.append(dict(title=title, text=rel(text)))


QMessageBox.information = staticmethod(information)
QMessageBox.warning = staticmethod(warning)


def existing_directory(parent, title, *args, **kwargs):
    BOXES.append(dict(folder=title))
    answer = FOLDERS.popleft()
    return full(answer) if answer else ''


QFileDialog.getExistingDirectory = staticmethod(existing_directory)


def scripted_exec(dialog):
    """The copy dialog: record the fields it opened with, fill them from the script, answer."""
    edits = dialog.findChildren(QLineEdit)
    combos = dialog.findChildren(QComboBox)
    checks = dialog.findChildren(QCheckBox)
    source, destination, template, backup, keywords = edits[:5]
    subfolders, preset = combos[:2]
    use_backup, skip = checks[:2]
    OPENED.append(dict(source=rel(source.text()), destination=rel(destination.text()), template=template.text(),
                       backup=rel(backup.text()), keywords=keywords.text(), subfolders=subfolders.currentData(),
                       preset=preset.currentData(), use_backup=use_backup.isChecked(), skip=skip.isChecked()))
    form = FORMS.popleft()
    if not form.get('accept', True):
        return QDialog.DialogCode.Rejected
    source.setText(full(form['source']))
    destination.setText(full(form['destination']))
    template.setText(form['template'])
    backup.setText(full(form['backup']))
    keywords.setText(form['keywords'])
    subfolders.setCurrentIndex(subfolders.findData(form['subfolders']))
    preset.setCurrentIndex(preset.findData(form['preset']))
    use_backup.setChecked(form['use_backup'])
    skip.setChecked(form['skip'])
    return QDialog.DialogCode.Accepted


QDialog.exec = scripted_exec


class Timer:
    @staticmethod
    def singleShot(delay, call):
        LATER.append(call)


app_module.QTimer = Timer


def run_later():
    while LATER:
        LATER.pop(0)()


class StatusBar:
    def __init__(self):
        self.messages = []

    def showMessage(self, text, *args):
        self.messages.append(rel(text))


# ---- the stand-in window ---------------------------------------------------------------------------------------

class Window(QWidget):
    import_paths = MainWindow.import_paths
    import_next = MainWindow.import_next
    apply_import_extras = MainWindow.apply_import_extras

    def __init__(self, catalog):
        super().__init__()
        self.catalog = catalog
        self.status = StatusBar()
        self.import_queue = deque()
        self.import_busy = False
        self.import_errors = []
        self.import_scans = 0
        self.import_pending_paths = set()
        self.import_completion_errors = {}
        self.import_quiet = set()
        self.import_extras = {}
        self.import_cancel = Event()
        self.import_refresh_timer = Nop()
        self.import_cancel_button = Nop()
        self.photo_model = Nop()
        self.thumbnail_pool = None
        self.export_running = False
        self.maintenance_running = False
        self.command_running = False
        self.closing = False
        self.current_id = None
        self.visible_ids = []
        self.folder_filter = None
        self.all_presets = catalog.presets()
        self.errors = []

    def statusBar(self):
        return self.status

    def spawn(self, work, done, failed=None, pool=None):
        try:
            result = work()
        except Exception as error:   # noqa: BLE001 - the oracle's `failed`
            if failed:
                failed(error)
            return
        done(result)

    def show_error(self, error):
        self.errors.append(rel(str(error)))

    def refresh_lists(self, *args, **kwargs):
        pass

    def refresh_folder_tree(self):
        pass

    def activate(self, *args, **kwargs):
        pass


def folder_sync_for(window):
    sync = FolderSync.__new__(FolderSync)
    sync.w = window
    sync.running = False
    sync.cancel = Event()
    sync.last = 0.
    sync.revision = 0
    sync.auto = set()
    sync.skipped = {}
    sync.excluded = set(window.catalog.preference(sync_module.EXCLUDED, []) or [])
    sync.pool = None
    window.folder_sync = sync
    return sync


def extras_for(window):
    tools = ExtraTools.__new__(ExtraTools)
    tools.w = window
    tools.dialogs = []
    tools.watch_paths = window.catalog.preference('watch_folders', [])
    tools.timer = Nop()
    window.extras = tools
    return tools


# ---- the inputs ------------------------------------------------------------------------------------------------

def png(width, height, seed, date=None, model=None):
    """A small deterministic PNG, with EXIF capture time and camera when given."""
    y, x = np.mgrid[0:height, 0:width]
    rgb = np.stack([(x * 9 + seed * 40) % 256, (y * 13 + seed * 70) % 256, ((x + y) * 5 + seed * 25) % 256], -1)
    stream = io.BytesIO()
    exif = Image.Exif()
    if model:
        exif[271] = 'Maker'
        exif[272] = model
    if date:
        exif.get_ifd(0x8769)[36867] = date
    Image.fromarray(rgb.astype(np.uint8)).save(stream, format='PNG', exif=exif.tobytes() if (date or model) else b'')
    return stream.getvalue()


OLD = datetime(2026, 8, 1, 12, 0, 0, tzinfo=timezone.utc).timestamp()     # the same day in every time zone

INPUTS = {
    'one': png(24, 16, 1, '2026:09:25 10:00:00', 'Cam/One'),
    'two': png(24, 16, 2, '2026:09:25 09:00:00', 'Cam/One'),
    'three': png(20, 30, 3, '2026:09:26 08:00:00', 'Other'),
    'four': png(32, 12, 4),
    'five': png(16, 16, 5, '2026:09:25 10:00:00', 'Cam/One'),
    'late': png(18, 14, 6, '2026:10:02 07:30:00', 'Cam/One'),
    'inbox': png(14, 18, 7),
    'blocker': png(10, 10, 8),
    'sidecar': b'<x:xmpmeta xmlns:x="adobe:ns:meta/"/>\n',
    'notes': b'not a photo\n',
}

# (relative path, input): every file starts with the time OLD.
FILES = [
    ('card/DCIM/100/IMG_0001.png', 'one'),
    ('card/DCIM/100/IMG_0002.png', 'two'),
    ('card/DCIM/100/IMG_0002.xmp', 'sidecar'),
    ('card/DCIM/101/IMG_0003.png', 'three'),
    ('card/DCIM/101/IMG_0004.png', 'four'),
    ('card/DCIM/101/notes.txt', 'notes'),
    ('card/DCIM/102/IMG_0001.png', 'five'),
    ('photos/2026/2026-09-25/IMG_0002.png', 'blocker'),       # exists already: the copy takes another name
    ('lib/existing/IMG_0003.png', 'three'),
    ('inbox/in1.png', 'inbox'),
    ('inbox/sub/in2.png', 'four'),
]
EXISTING = [('lib/existing/IMG_0003.png', {'date': '2026:09:26 08:00:00', 'width': 20, 'height': 30})]
PRESET = ('따뜻하게', {'exposure': 0.3, 'contrast': 12, 'crop': [0.1, 0.1, 0.9, 0.9], 'rotation': 1})


def build(root):
    for path, name in FILES:
        target = root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(INPUTS[name])
        os.utime(target, (OLD, OLD))
    (root / 'library').mkdir()
    catalog = Catalog(root / 'library')
    for path, metadata in EXISTING:
        catalog.add(root / path, {}, metadata)
    catalog.save_preset(PRESET[0], normalized(PRESET[1]))
    return catalog


# ---- what is recorded ------------------------------------------------------------------------------------------

PREFERENCES = ('import_copy', 'watch_folders', 'sync_excluded', 'removed_folders', 'auto_sync_folders')
DEFAULTS = defaults()


def tables(catalog):
    db = catalog.db
    photos = []
    for row in db.execute('SELECT id,path,name,keywords,rating,virtual_source FROM photos ORDER BY id'):
        record = catalog.photo(row['id'])
        changed = {k: v for k, v in record['settings'].items() if v != DEFAULTS.get(k)}
        meta = json.loads(db.execute('SELECT metadata FROM photos WHERE id=?', (row['id'],)).fetchone()[0] or '{}')
        photos.append(dict(id=row['id'], path=rel(row['path']), name=row['name'], keywords=row['keywords'],
                           rating=row['rating'], virtual_source=row['virtual_source'], settings=changed,
                           date=meta.get('date'), size_mb=meta.get('size_mb')))
    roots = [rel(r) for r in catalog.folder_roots()]
    prefs = {}
    for key in PREFERENCES:
        row = db.execute('SELECT value FROM preferences WHERE key=?', (key,)).fetchone()
        prefs[key] = rel_value(json.loads(row[0])) if row else None
    history = [list(r) for r in db.execute('SELECT id,photo_id,label FROM history ORDER BY id')]
    return dict(photos=photos, folder_roots=roots, preferences=prefs, history=history)


def tree(root):
    names = {v: k for k, v in INPUTS.items()}
    out = []
    for path in sorted(root.rglob('*'), key=lambda p: p.relative_to(root).as_posix().lower()):
        if path.is_file() and path.relative_to(root).parts[0] != 'library':
            out.append([path.relative_to(root).as_posix(), names.get(path.read_bytes(), 'other')])
    return out


def form(source='<ROOT>\\card', destination='<ROOT>\\photos', subfolders='year/date', template='{stem}',
         use_backup=False, backup='', keywords='', preset='', skip=True, accept=True):
    return dict(source=source, destination=destination, subfolders=subfolders, template=template,
                use_backup=use_backup, backup=backup, keywords=keywords, preset=preset, skip=skip, accept=accept)


SCRIPT = [
    ['copy', [
        form(template='{name}'),                                          # an unknown field
        form(template='{stem}/x'),                                        # not a file name
        form(source='<ROOT>\\nowhere'),                                   # no such folder
        form(destination='  '),                                           # no destination
        form(destination='<ROOT>\\card\\DCIM\\out'),                      # inside the source
        form(use_backup=True, backup=''),                                 # no backup folder
        form(use_backup=True, backup='<ROOT>\\photos'),                   # backup = destination
        form(use_backup=True, backup='<ROOT>\\backup', keywords='trip, card , trip', preset=PRESET[0]),
    ]],
    ['copy', [form(accept=False)]],                                       # opened with the saved fields, 취소
    ['copy', [form(source='<ROOT>\\card\\DCIM\\100', subfolders='date', template='{date}_{time}_{camera}_{n:02d}',
                   skip=False)]],
    ['copy', [form(subfolders='none', keywords='again')]],                # the copies are duplicates now
    ['remove', 'IMG_0004.png'],                                           # taken out of the catalogue, file stays
    ['write', 'photos/2026/2026-10-02/late.png', 'late'],                 # a new photo in a registered folder
    ['sync'],
    ['sync'],                                                             # nothing new
    ['folder', '<ROOT>\\inbox'],
    ['watch', None],
    ['watch', None],                                                      # registered already
    ['write', 'inbox/sub/in3.png', 'one'],
    ['watch_scan'],
    ['folder', None],
    ['watch', '<ROOT>\\photos'],                                          # asked with the folder dialog
    ['unwatch'],
]


def run_script():
    catalog = build(ROOT)
    window = Window(catalog)
    sync = folder_sync_for(window)
    tools = extras_for(window)
    steps = []
    for step in SCRIPT:
        action, a = step[0], step[1:]
        BOXES.clear()
        OPENED.clear()
        window.status.messages.clear()
        window.errors.clear()
        if action == 'copy':
            FORMS.extend(a[0])
            import_copy.dialog(window)
            assert not FORMS, 'the dialog took fewer attempts than the script gave'
        elif action == 'remove':
            ids = [r[0] for r in catalog.db.execute('SELECT id,path FROM photos') if Path(r[1]).name == a[0]]
            paths = [catalog.photo(i)['path'] for i in ids]
            catalog.remove_photos(ids)
            sync.exclude(paths)
        elif action == 'write':
            target = ROOT / a[0]
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(INPUTS[a[1]])
            os.utime(target, (OLD, OLD))
        elif action == 'sync':
            sync.scan()
        elif action == 'folder':
            window.folder_filter = full(a[0]) if a[0] else None
        elif action == 'watch':
            if a[0] is not None:
                FOLDERS.append(a[0])
            tools.watch()
        elif action == 'watch_scan':
            tools.watch_scan()
        elif action == 'unwatch':
            tools.unwatch()
        else:
            raise ValueError(action)
        run_later()
        catalog.db.commit()
        steps.append(dict(step=step, opened=list(OPENED), boxes=list(BOXES), messages=list(window.status.messages),
                          errors=list(window.errors), tables=tables(catalog), files=tree(ROOT),
                          watch_paths=[rel(p) for p in tools.watch_paths],
                          excluded=sorted(rel(k) for k in sync.excluded), auto=sorted(rel(k) for k in sync.auto)))
    return catalog, steps


with tempfile.TemporaryDirectory(prefix='grainy-app-import-', ignore_cleanup_errors=True) as temporary:
    ROOT = Path(temporary).resolve()
    catalog, steps = run_script()
    with Case('app', 'import_steps') as case:
        case.json(stamp=STAMP, steps=steps, script=json.dumps(SCRIPT, ensure_ascii=False), files=FILES,
                  existing=EXISTING, preset=[PRESET[0], PRESET[1]], old=OLD)
    catalog.close()

with Case('app', 'import_inputs') as case:
    for name, data in INPUTS.items():
        case.array(name, np.frombuffer(data, np.uint8))
    case.json(names=list(INPUTS))
