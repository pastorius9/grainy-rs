"""Goldens of the metadata package of grainy-app (APP_PLAN package 6): `golden/app/metadata*/`.

Run as the oracle's own methods on stand-ins for the window and the manager (nothing is reimplemented):

* `MainWindow.save_keywords` (the keywords field of the panel);
* `LibraryManager.metadata` (the metadata box: ten fields and the keywords; the box is a `QDialog` whose `exec`
  fills the line edits with the scripted answers and accepts), `shift_date` (the minutes box answers the script),
  `refresh_metadata` (its job runs at once) and `keyword_tree` (the real `QTreeWidget` is read back, and every item
  is double-clicked);
* the FILE section's text (`MainWindow.activate`'s `file_info.setText(...)` expression) for a spread of values.

The catalogue is built here from fixed rows (no files, no file times, no clocks: the paths are `Z:\\GrainyGolden\\...`
and the stored metadata is written by hand), so the output is deterministic. The refresh case uses a few tiny
files made here (with EXIF through Pillow) and stored in `golden/app/metadata_refresh/files/`.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_metadata.py
"""
import json
import shutil
import tempfile
from datetime import datetime
from pathlib import Path

from common import GOLDEN, Case, write_json
from luma import catalog as catalog_module
from luma import manager as manager_module
from luma.app import MainWindow
from luma.catalog import Catalog
from luma.manager import LibraryManager
from PIL import Image
from PIL.TiffImagePlugin import IFDRational
from PySide6.QtWidgets import QApplication, QDialog, QLabel, QLineEdit, QTreeWidget, QWidget

_qt = QApplication.instance() or QApplication([])

STAMP = '2026-11-13T01:02:03.456789+00:00'
ROOT = 'Z:\\GrainyGolden\\meta'


class Clock(datetime):
    @classmethod
    def now(cls, tz=None):
        value = datetime.fromisoformat(STAMP)
        return value if tz else value.replace(tzinfo=None)


catalog_module.datetime = Clock

FIELD_KEYS = ['title', 'caption', 'creator', 'copyright', 'location', 'city', 'country', 'DateTimeOriginal',
              'latitude', 'longitude']

# (file name, stored metadata, keywords, extras)
ROWS = [
    ('a.jpg', {'width': 6000, 'height': 4000, 'format': 'JPEG', 'size_mb': 12.5, 'camera': 'Canon EOS R5',
               'date': '2025:06:01 10:00:00'},
     '여행|일본, 필름',
     {'title': 'Garden', 'DateTimeOriginal': '2025:06:01 10:05:00', 'latitude': 37.5, 'longitude': '127.0',
      'workflow_note': 'keep me'}),
    ('b.jpg', {'width': 300, 'height': 200, 'format': 'JPEG', 'size_mb': 0.05, 'date': '2024:12:31 23:30:00'},
     '여행|일본|도쿄, Sunset', {'caption': 'dusk'}),
    ('c.png', {'width': 96, 'height': 64, 'format': 'PNG', 'size_mb': 0.01}, '', {}),
    ('d.jpg', {'width': 96, 'height': 64, 'format': 'JPEG', 'size_mb': 0.02, 'date': '2023:03:04 05:06:07'},
     '인물', {'DateTimeOriginal': '2023:03:04 05:07:08', 'copyright': '(c) W', 'creator': 'W'}),
    ('e.jpg', {'date': ''}, '여행|한국|서울, 여행|일본', {'DateTimeOriginal': ''}),
    ('f.jpg', {'date': '2020:02:29 12:00:00'}, 'Sunset', {'title': 5, 'altitude': 12.25, 'tags': ['x', 'y']}),
    ('g.jpg', {}, ' 가 , ,나|다 ', {'DateTimeOriginal': '9999:12:31 23:59:59'}),
    ('h.jpg', {}, '', {'DateTimeOriginal': 'garbage'}),
]

# Steps: select / open / keywords / metadata / shift
SCRIPT = [
    ['open', 1], ['keywords', 1, '여행|일본, 필름, new'],
    ['select', [1]], ['metadata', {'title': 'Garden 2', 'city': 'Tokyo'}, None],
    ['select', [2, 4, 6]], ['metadata', {'creator': 'Kim', 'latitude': '35.68', 'longitude': '139.69'}, '풍경, 여행|일본'],
    ['metadata', {'latitude': '91'}, None],
    ['metadata', {'longitude': '-180.5'}, None],
    ['metadata', {'longitude': 'abc'}, None],
    ['metadata', {'DateTimeOriginal': '2025-06-01'}, None],
    ['metadata', {'DateTimeOriginal': '2025:07:04 08:09:10'}, None],
    ['select', [1, 2, 3, 4, 5, 6]], ['shift', 540], ['shift', -60], ['shift', 0],
    ['select', [1, 4]], ['metadata', {'title': ''}, None],
    ['metadata', {}, None],
    ['select', [5]], ['metadata', {'DateTimeOriginal': '2024:02:29 23:59:59'}, None], ['shift', 1],
    ['shift', 525600],
    ['select', [6]], ['shift', -525600],
    ['select', [7]], ['shift', 1],
    ['select', [8, 1]], ['shift', 5],
    ['select', [3]], ['metadata', {'location': '서울 · 성수', 'country': 'KR'}, '풍경'],
    ['open', 2], ['keywords', 2, '  a ,, b|c '],
    ['select', [2, 3]], ['metadata', {}, '여행|한국'],
]


class Status:
    def __init__(self, log):
        self.log = log

    def showMessage(self, text, *args):
        self.log.append(text)


class Field:
    def __init__(self):
        self.value = ''

    def setText(self, text):
        self.value = text

    def text(self):
        return self.value


class Window(QWidget):
    save_keywords = MainWindow.save_keywords

    def __init__(self, catalog):
        super().__init__()
        self.catalog = catalog
        self.current_id = None
        self.selection = []
        self.keywords = Field()
        self.messages = []
        self.folder_filter = 'x'
        self.refreshes = []
        self.modes = []
        self.jobs = []

    def statusBar(self):
        return Status(self.messages)

    def selected_ids(self):
        return list(self.selection) or ([self.current_id] if self.current_id else [])

    def load_controls(self):
        pass

    def refresh_lists(self, *args, **kwargs):
        self.refreshes.append(kwargs)

    def set_mode(self, index):
        self.modes.append(index)

    def spawn(self, work, done, *args):
        done(work())


class Manager:
    metadata = LibraryManager.metadata
    shift_date = LibraryManager.shift_date
    selected_photos = LibraryManager.selected_photos
    refresh_metadata = LibraryManager.refresh_metadata
    keyword_tree = LibraryManager.keyword_tree

    def __init__(self, window):
        self.w = window
        self.rules = {}
        self.collection = None
        self.dialogs = []


class Answers:
    fields = {}
    keywords = None
    minutes = 0
    initial = None


class FakeDialog(QDialog):
    """The metadata box: `exec` reads the fields as built, writes the scripted answers and accepts."""

    def exec(self):
        edits = self.findChildren(QLineEdit)
        assert len(edits) == len(FIELD_KEYS) + 1, len(edits)
        Answers.initial = [e.text() for e in edits]
        for key, answer in Answers.fields.items():
            edits[FIELD_KEYS.index(key)].setText(answer)
        if Answers.keywords is not None:
            edits[-1].setText(Answers.keywords)
        return QDialog.DialogCode.Accepted


class FakeInput:
    @staticmethod
    def getInt(parent, title, label, value, low, high):
        assert (title, label, value, low, high) == ('촬영 시각 이동', '이동할 분 (예: +540 / -60)', 0, -525600, 525600)
        return Answers.minutes, True


manager_module.QDialog = FakeDialog
manager_module.QInputDialog = FakeInput


def build(folder, rows):
    c = Catalog(folder)
    c.register_folder(ROOT)
    for name, info, keywords, extras in rows:
        ident = c.add(ROOT + '\\' + name, settings=None, metadata=info, resolved=True)
        c.update(ident, keywords=keywords, extras=json.dumps(extras, ensure_ascii=False))
    return c


def projection(c):
    return [dict(id=p['id'], keywords=p['keywords'], extras=p['extras']) for p in c.photos()]


def full_rows(c):
    return [{k: p[k] for k in ('id', 'path', 'name', 'rating', 'flag', 'label', 'keywords', 'metadata', 'extras',
                               'stack_id', 'virtual_source')} for p in c.photos()]


def tree_of(manager):
    """The keyword tree dialog as built: (depth, text, count, path) rows, and what a double click on each item did."""
    dialog = manager.dialogs[-1]
    tree = dialog.findChild(QTreeWidget)
    header = [tree.headerItem().text(0), tree.headerItem().text(1)]
    rows = []
    items = []

    def walk(item, depth):
        rows.append([depth, item.text(0), item.text(1), item.data(0, 0x0100)])
        items.append(item)
        for i in range(item.childCount()):
            walk(item.child(i), depth + 1)

    for i in range(tree.topLevelItemCount()):
        walk(tree.topLevelItem(i), 0)
    clicks = []
    w = manager.w
    for item in items:
        w.refreshes.clear()
        w.modes.clear()
        w.folder_filter = 'x'
        manager.collection = 7
        tree.itemDoubleClicked.emit(item, 0)
        clicks.append(dict(path=item.data(0, 0x0100), rules=manager.rules, collection=manager.collection,
                           folder_filter=w.folder_filter, refreshes=list(w.refreshes), modes=list(w.modes)))
    footer = [lab.text() for lab in dialog.findChildren(QLabel)]
    return dict(title=dialog.windowTitle(), header=header, rows=rows, clicks=clicks, footer=footer)


def run_step(manager, step):
    w = manager.w
    w.messages.clear()
    record = dict(step=step, error=None, initial=None)
    action = step[0]
    try:
        if action == 'open':
            w.current_id = step[1]
            w.selection = [step[1]]
        elif action == 'select':
            w.selection = list(step[1])
        elif action == 'keywords':
            w.current_id = step[1]
            w.keywords.setText(step[2])
            w.save_keywords()
        elif action == 'metadata':
            Answers.fields = step[1]
            Answers.keywords = step[2]
            Answers.initial = None
            try:
                manager.metadata()
            finally:
                record['initial'] = Answers.initial
        elif action == 'shift':
            Answers.minutes = step[1]
            manager.shift_date()
        else:
            raise ValueError(action)
    except Exception as error:
        record['error'] = f'{type(error).__name__}: {error}'
    record['status'] = list(w.messages)
    return record


# ---- the scripted session --------------------------------------------------------------------------------------

with tempfile.TemporaryDirectory(prefix='grainy-app-metadata-', ignore_cleanup_errors=True) as temporary:
    work = Path(temporary).resolve()
    c = build(work / 'Library', ROWS)
    start = full_rows(c)
    w = Window(c)
    manager = Manager(w)
    manager.keyword_tree()
    start_tree = tree_of(manager)
    manager.dialogs.clear()
    steps = []
    for step in SCRIPT:
        record = run_step(manager, step)
        record['rows'] = projection(c)
        steps.append(record)
    final = full_rows(c)
    manager.keyword_tree()
    final_tree = tree_of(manager)
    c.close()

with Case('app', 'metadata') as case:
    case.json(root=ROOT, rows=[dict(name=n, metadata=json.dumps(i), keywords=k, extras=json.dumps(e, ensure_ascii=False))
                               for n, i, k, e in ROWS],
              fields=FIELD_KEYS, start=start, steps=steps, final=final, start_tree=start_tree, final_tree=final_tree)

# ---- the FILE section's text -----------------------------------------------------------------------------------

INFOS = {
    'camera': {'width': 6000, 'height': 4000, 'format': 'JPEG', 'size_mb': 12.34, 'make': 'Canon', 'camera': 'EOS R5',
               'shutter': '1/250', 'aperture': 2.8, 'iso': 400.0, 'focal_length': 50.0, 'lens': 'RF 24-70mm F2.8',
               'date': '2025:06:01 10:00:00', 'latitude': 37.5, 'longitude': -122.25,
               'file_fingerprint': {'path': 'x', 'size': 1}, 'offline_fingerprint': 'y'},
    'floats': {'a': 0.1, 'b': 1e-05, 'c': 1e16, 'd': 123456789.123, 'e': -0.0, 'f': 100.0, 'g': 0.0001, 'h': 0.00001234,
               'i': 1.5e300, 'j': 12345678901234567890.0, 'k': 1e15, 'l': 123456.789e-10, 'm': 2.5e-7, 'n': 1e22,
               'o': 3, 'p': -7, 'q': 18446744073709551615},
    'others': {'t': True, 'f': False, 'n': None, 'list': [1, 'a', None, 2.5, [True]], 'dict': {'k': 'v', 'n': 1},
               'quote': "it's", 'both': 'say "it\'s"', 'dq': 'x"y', 'unicode': '필름 · 한글', 'newline': 'a\nb',
               'backslash': 'C:\\x', 'empty': '', 'tab': 'a\tb'},
    'empty': {},
    'only_hidden': {'file_fingerprint': {}, 'offline_fingerprint': 1},
}
cases = []
for name, info in INFOS.items():
    text = json.dumps(info)
    loaded = json.loads(text)
    path = 'Z:\\GrainyGolden\\meta\\한글 photo.jpg'
    shown = path + '\n\n' + '\n'.join(f'{k}: {v}' for k, v in loaded.items() if k not in ('file_fingerprint', 'offline_fingerprint'))
    cases.append(dict(name=name, path=path, info=text, text=shown))
with Case('app', 'metadata_info') as case:
    case.json(cases=cases)

# ---- the EXIF refresh ------------------------------------------------------------------------------------------

REFRESH = GOLDEN / 'app' / 'metadata_refresh'
FILES = REFRESH / 'files'
if FILES.exists():
    shutil.rmtree(FILES)
FILES.mkdir(parents=True, exist_ok=True)


def exif(make, model, date, lens, exposure, f_number, iso, focal, gps):
    e = Image.Exif()
    e[271] = make
    e[272] = model
    sub = e.get_ifd(0x8769)
    sub[33434] = IFDRational(*exposure)    # rationals, as cameras write them
    sub[33437] = IFDRational(*f_number)
    sub[34855] = iso
    sub[37386] = IFDRational(*focal)
    sub[36867] = date
    sub[42036] = lens
    if gps:
        g = e.get_ifd(0x8825)
        g[1] = 'S'
        g[2] = (IFDRational(33, 1), IFDRational(51, 1), IFDRational(54, 1))
        g[3] = 'E'
        g[4] = (IFDRational(151, 1), IFDRational(12, 1), IFDRational(36, 1))
    return e


picture = Image.new('RGB', (8, 6), (90, 120, 150))
picture.save(FILES / 'canon.jpg', 'JPEG', quality=90, exif=exif('Canon', 'EOS R5', '2025:06:01 10:00:00', 'RF 24-70mm', (1, 250), (28, 10), 400, (50, 1), True))
picture.save(FILES / 'sony.png', 'PNG', exif=exif('Sony', 'ILCE-7M4', '2024:01:02 03:04:05', 'FE 35mm', (1, 2), (18, 10), 100, (35, 1), False))
picture.save(FILES / 'plain.jpg', 'JPEG', quality=90)
(FILES / 'broken.jpg').write_bytes(b'this is not a picture')
(FILES / 'clip.mp4').write_bytes(b'video')

REFRESH_ROWS = [   # (file, stored metadata)
    ('canon.jpg', {'width': 8, 'height': 6, 'format': 'JPEG', 'camera': 'OLD CAMERA', 'extra': 'kept', 'iso': 1.0}),
    ('sony.png', {'width': 8, 'height': 6, 'format': 'PNG'}),
    ('plain.jpg', {'width': 8, 'height': 6, 'format': 'JPEG', 'date': 'stays'}),
    ('broken.jpg', {'width': 1}),
    ('clip.mp4', {'width': 2}),
    ('missing.jpg', {'width': 3}),
]
with tempfile.TemporaryDirectory(prefix='grainy-app-metadata-refresh-', ignore_cleanup_errors=True) as temporary:
    work = Path(temporary).resolve()
    folder = work / 'files'
    shutil.copytree(FILES, folder)
    c = Catalog(work / 'Library')
    c.register_folder(str(folder))
    for name, info in REFRESH_ROWS:
        c.add(str(folder / name), settings=None, metadata=info, resolved=True)
    w = Window(c)
    manager = Manager(w)
    manager.refresh_metadata()
    result = [dict(name=p['name'], metadata=p['metadata']) for p in c.photos()]
    messages = list(w.messages)
    refreshes = len(w.refreshes)
    c.close()
with Case('app', 'metadata_refresh') as case:
    case.json(rows=[dict(name=n, metadata=json.dumps(i)) for n, i in REFRESH_ROWS], result=result, messages=messages,
              refreshes=refreshes)
