"""Goldens of grainy-app work package 29 (Lightroom import): golden/app/lightroom_*.

* `lightroom_import`: scenarios of luma/lightroom_import.py `import_catalog` (with `adobe_history.transfer` when
  the edits are converted) on small Lightroom catalogues built with the oracle's own test fixture
  (tests/test_lightroom_import.py `fixture_catalog` and `insert` on the real 15.5.1 table definitions of
  tests/fixtures/lightroom-15.5.1-schema.json) plus the changes of the oracle's own tests. Each scenario stores
  its recipe (the CREATE statements and every insert/update in order, so the Rust test builds the same .lrcat
  with plain SQLite), the steps (an import's result dict, `report_text` of it and of the stored summary, as
  `extras.lightroom_report` shows it, an error's text) and every catalogue table after each step.
* `lightroom_flow`: `lightroom_import_dialog.begin_import` driven headless on a stand-in window (jobs run at
  once, the review box answered from the script): the maintenance window's texts, the review box (title, text,
  check box, buttons), the boxes shown at the end and the tables; then `ExtraTools.lightroom_report` and
  `import_lightroom`'s refusal while a job runs.
* `lightroom_inspect`: `inspect_catalog` counts, and the refusal of a catalogue without Lightroom's tables.

The photos' folder is a fixed path that does not exist (`C:\\Grainy Lightroom Test\\Photos`): the import only
records paths, so the stored digests do not depend on the temporary folder. The Lightroom catalogue and the
Grainy library live in a temporary folder (written `<ROOT>` in the goldens). Deterministic: the clocks are fixed,
the run id is fixed and the fixture's `id_global` values are numbered.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_lightroom.py
"""
import base64
import hashlib
import json
import sqlite3
import sys
import tempfile
import traceback
from datetime import datetime, timedelta
from pathlib import Path
from threading import Event

from common import Case, ORACLE

sys.path.insert(0, str(ORACLE))
from luma import adobe_history, catalog as catalog_module, lightroom_import  # noqa: E402
from luma.catalog import Catalog  # noqa: E402
from luma.lightroom_import import import_catalog, inspect_catalog, report_text  # noqa: E402
from tests import test_lightroom_import as fixture  # noqa: E402

STAMP = '2026-11-19T01:02:03.456789+00:00'
PHOTOS = r'C:\Grainy Lightroom Test\Photos'


class Clock(datetime):
    """The catalogue's clock: always STAMP."""
    @classmethod
    def now(cls, tz=None):
        value = datetime.fromisoformat(STAMP)
        return value if tz else value.replace(tzinfo=None)


class Ticking(datetime):
    """The importer's clock: STAMP plus one second more at every call (numbered from 0 in each scenario), so
    the runs of one library have different `created` times; `now()` without a zone is the same wall clock."""
    calls = 0

    @classmethod
    def now(cls, tz=None):
        value = datetime.fromisoformat(STAMP) + timedelta(seconds=cls.calls)
        cls.calls += 1
        return value if tz else value.replace(tzinfo=None)


class FixedId:
    def __init__(self, n):
        self.hex = f'{n:032x}'


class FixedUuid:
    """`uuid.uuid4()` numbered from 1 in each scenario (the run ids)."""
    count = 0

    @classmethod
    def uuid4(cls):
        cls.count += 1
        return FixedId(cls.count)


lightroom_import.datetime = Ticking
lightroom_import.uuid = FixedUuid
adobe_history.datetime = Ticking
catalog_module.datetime = Clock

ROOT = None


def rel(value):
    """Paths of the temporary folder written `<ROOT>`, inside strings, lists and dicts."""
    if isinstance(value, str):     # also inside stored JSON text, where backslashes are doubled
        for root in (str(ROOT), str(ROOT).casefold()):      # casefolded in path source keys
            value = value.replace(root.replace('\\', '\\\\'), '<ROOT>').replace(root, '<ROOT>')
        return value
    if isinstance(value, list):
        return [rel(v) for v in value]
    if isinstance(value, dict):
        return {rel(k): rel(v) for k, v in value.items()}
    return value


# ---- the recipe of a Lightroom catalogue -----------------------------------------------------------------------

class Recipe:
    def __init__(self):
        self.ops = []
        self.counter = 0

    def insert(self, db, table, **values):
        columns = list(db.execute(f'PRAGMA table_info({table})'))
        if any(c[1] == 'id_global' and c[3] for c in columns) and 'id_global' not in values:
            self.counter += 1
            values['id_global'] = f'fixture-{self.counter:04d}'
        self.ops.append(['insert', table, values])
        ORIGINAL_INSERT(db, table, **values)

    def sql(self, db, statement, params=()):
        self.ops.append(['sql', statement, list(params)])
        db.execute(statement, params)


ORIGINAL_INSERT = fixture.insert
SCHEMA = json.loads((ORACLE / 'tests' / 'fixtures' / 'lightroom-15.5.1-schema.json').read_text(encoding='utf-8'))


def build_source(recipe, path, change=None):
    """`fixture_catalog(path, PHOTOS)` with the inserts recorded, then `change(db, recipe)`."""
    fixture.insert = recipe.insert
    try:
        fixture.fixture_catalog(path, Path(PHOTOS))
    finally:
        fixture.insert = ORIGINAL_INSERT
    if change:
        with sqlite3.connect(path) as db:
            change(db, recipe)
        db.close()


def plain(value):
    """A cell for the golden: bytes as base64; a long text (settings, archived rows) as its SHA-256 and start."""
    if isinstance(value, bytes):
        return {'base64': base64.b64encode(value).decode('ascii')}
    if isinstance(value, str) and len(value) > 160:
        return {'sha256': hashlib.sha256(value.encode('utf-8')).hexdigest(), 'head': value[:80]}
    return value


def tables(db):
    """Every table of the library: rows as dicts, sorted by their JSON text, paths relative."""
    out = {}
    names = [r[0] for r in db.execute(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")]
    for name in names:
        cursor = db.execute(f'SELECT * FROM "{name}"')
        columns = [d[0] for d in cursor.description]
        rows = [{c: plain(rel(v)) for c, v in zip(columns, row)} for row in cursor]
        if name == 'preview_state':      # a trigger writes random tokens (randomblob): only their presence counts
            rows = [{**r, 'token': '<token>'} for r in rows]
        rows.sort(key=lambda r: json.dumps(r, ensure_ascii=False, sort_keys=True, separators=(',', ':')))
        out[name] = rows
    return out


def backups(directory):
    folder = Path(directory) / 'backups'
    return sorted(p.name for p in folder.iterdir()) if folder.is_dir() else []


def stored_report(db):
    exists = db.execute("SELECT 1 FROM sqlite_master WHERE name='lightroom_import_runs'").fetchone()
    row = db.execute('SELECT summary FROM lightroom_import_runs ORDER BY created DESC LIMIT 1').fetchone() if exists else None
    return report_text(json.loads(row[0])) if row else None


def step_record(dest, label, result=None, error=None):
    return dict(step=label, result=rel(result), report=rel(report_text(result)) if result else None,
                stored=rel(stored_report(dest.db)), error=rel(error), backups=backups(dest.directory),
                tables=tables(dest.db))


# ---- the changes of the oracle's tests ---------------------------------------------------------------------------

def optin(db, r):            # test_optin_current_history_snapshots_and_persistent_undo
    r.sql(db, 'DELETE FROM Adobe_libraryImageDevelopHistoryStep')
    r.sql(db, 'UPDATE Adobe_imageDevelopSettings SET text=?', ('s={Exposure2012=1.25,Blacks2012=-20,CameraProfile="Adobe Color"}',))
    for i, value in enumerate((0, .5, 1.25)):
        r.insert(db, 'Adobe_libraryImageDevelopHistoryStep', id_local=10+i, image=100, name=f'노출 {value}',
                 dateCreated=100+i, text=f's={{Exposure2012={value},Blacks2012=-20,CameraProfile="Adobe Color"}}')
    r.insert(db, 'Adobe_libraryImageDevelopHistoryStep', id_local=20, image=101, dateCreated=50.5, text='s={Contrast2012=30}')
    r.sql(db, 'UPDATE Adobe_libraryImageDevelopSnapshot SET text=?', ('s={Exposure2012=-.25,ConvertToGrayscale=true}',))
    r.insert(db, 'Adobe_libraryImageDevelopSnapshot', id_local=2, image=100, name='이미 반영',
             text='s={AlreadyApplied=true}')
    r.insert(db, 'Adobe_imageDevelopSettings', id_local=2, image=101, text='s={Saturation=-100,Vibrance=20}')


def gap(db, r):              # test_history_gap_clears_undo_continuity_and_cancel_is_atomic
    r.sql(db, 'DELETE FROM Adobe_libraryImageDevelopHistoryStep')
    for i, text in enumerate(('s={Exposure2012=0}', 'not_lua()', 's={Exposure2012=.5}')):
        r.insert(db, 'Adobe_libraryImageDevelopHistoryStep', id_local=10+i, image=100, dateCreated=i, text=text)


def broken(db, r):           # test_cycles_delimiters_bad_paths_and_broken_virtuals_reported, and more paths
    r.sql(db, 'UPDATE AgLibraryKeyword SET parent=2 WHERE id_local=1')
    r.sql(db, 'UPDATE AgLibraryCollection SET parent=2 WHERE id_local=1')
    r.sql(db, 'UPDATE Adobe_images SET masterImage=999 WHERE id_local=99')
    r.insert(db, 'AgLibraryKeyword', id_local=3, name='one,two')
    r.insert(db, 'AgLibraryKeywordImage', id_local=3, image=100, tag=3)
    r.insert(db, 'AgLibraryKeyword', id_local=4, name='고아', parent=77)
    r.insert(db, 'AgLibraryKeywordImage', id_local=4, image=101, tag=4)
    r.insert(db, 'AgLibraryFile', id_local=12, folder=2, baseName='../bad', extension='jpg')
    r.insert(db, 'Adobe_images', id_local=102, rootFile=12)
    # A second root written the macOS way round (forward slashes, a trailing one), a RAW file in upper case,
    # a video, a relative root and a name that already ends with its extension.
    r.insert(db, 'AgLibraryRootFolder', id_local=5, absolutePath=PHOTOS.replace('\\', '/') + '/Second/')
    r.insert(db, 'AgLibraryFolder', id_local=6, rootFolder=5, pathFromRoot='2026/day 1/')
    r.insert(db, 'AgLibraryFile', id_local=13, folder=6, baseName='DSC_0001', extension='NEF')
    r.insert(db, 'Adobe_images', id_local=103, rootFile=13, rating=9, pick=-3, colorLabels='Purple',
             captureTime='2026-09-21T08:00:00.5+09:00', touchTime=86400.25, orientation='BC')
    r.insert(db, 'AgLibraryFile', id_local=14, folder=6, baseName='clip', extension='mp4')
    r.insert(db, 'Adobe_images', id_local=104, rootFile=14)
    r.insert(db, 'AgLibraryRootFolder', id_local=7, absolutePath='relative/folder')
    r.insert(db, 'AgLibraryFolder', id_local=8, rootFolder=7, pathFromRoot='')
    r.insert(db, 'AgLibraryFile', id_local=15, folder=8, baseName='lost', extension='jpg')
    r.insert(db, 'Adobe_images', id_local=105, rootFile=15)
    r.insert(db, 'AgLibraryFile', id_local=16, folder=2, baseName='named.TIF', extension='tif')
    r.insert(db, 'Adobe_images', id_local=106, rootFile=16, colorLabels='Custom', captureTime='not a date')
    r.insert(db, 'AgLibraryCollectionImage', id_local=7, image=555, collection=2)
    r.insert(db, 'AgLibraryFolderStackImage', id_local=3, image=103, stack=2, position=1)
    r.insert(db, 'AgLibraryCollectionStackImage', id_local=1, collection=2, collapsed=0, image=99, position=1, stack=1)
    r.insert(db, 'AgLibraryKeywordSynonym', id_local=1, keyword=1, name='trip')


def smart(db, r):            # test_dynamic_smart_import_updates_and_unsupported_rules_remain_whole
    r.insert(db, 'AgLibraryCollection', id_local=5, name='Unknown compound', creationId='com.adobe.ag.library.smart_collection')
    r.insert(db, 'AgLibraryCollectionContent', id_local=5, collection=5, owningModule='ag.library.smart_collection',
             content='s={{criteria="rating",operation=">=",value=1},{criteria="unknown",operation="==",value=1}}')
    r.insert(db, 'AgLibraryCollectionImage', id_local=5, image=100, collection=5)
    r.insert(db, 'AgLibraryCollection', id_local=6, name='No rules', creationId='com.adobe.ag.library.smart_collection')
    r.insert(db, 'AgInternedExifCameraModel', id_local=1, value='Nikon D3S')
    r.insert(db, 'AgInternedExifCameraSN', id_local=1, value='TEST-123')
    r.insert(db, 'AgInternedExifLens', id_local=1, value='50mm f/1.4')
    r.insert(db, 'AgInternedIptcCity', id_local=1, value='서울')
    r.insert(db, 'AgInternedIptcJobIdentifier', id_local=1, value='JOB-7')
    r.sql(db, 'UPDATE AgHarvestedIptcMetadata SET cityRef=1,jobIdentifierRef=1')
    r.sql(db, 'UPDATE AgHarvestedExifMetadata SET cameraModelRef=1,cameraSNRef=1,lensRef=1,isoSpeedRating=800')


def bulk(db, r):             # 300 history rows: the progress of 'Lightroom 기록 읽는 중' is reported
    for i in range(300):
        r.insert(db, 'Adobe_libraryImageDevelopHistoryStep', id_local=10+i, image=100, text=str(i))


def no_provider(db, r):      # no Adobe_storeProviderID: the source key is the path
    r.sql(db, 'DELETE FROM Adobe_variablesTable')


# ---- scenarios -----------------------------------------------------------------------------------------------

def scenario(name, change, steps):
    """Builds the source with `change`, then runs `steps(source, dest, record)`."""
    FixedUuid.count = 0
    Ticking.calls = 0
    recipe = Recipe()
    folder = ROOT / name
    folder.mkdir()
    source = folder / 'fixture.lrcat'
    build_source(recipe, source, change)
    built = len(recipe.ops)
    dest = Catalog(folder / 'luma')
    records = [step_record(dest, 'start')]
    try:
        steps(source, dest, recipe, records)
    finally:
        dest.close()
    return dict(name=name, recipe=recipe.ops, built=built, steps=changes_only(records))


def changes_only(records):
    """A step keeps in `tables` only the tables that differ from the step before (the first step: all)."""
    previous = {}
    for record in records:
        if 'tables' not in record:
            continue
        now = record['tables']
        record['tables'] = {k: v for k, v in now.items() if previous.get(k) != v}
        record['table_names'] = sorted(now)
        previous = now
    return records


def run_import(dest, records, label, source, **kwargs):
    try:
        result = import_catalog(source, dest.directory, **kwargs)
        records.append(step_record(dest, label, result))
    except Exception as error:   # noqa: BLE001 - the window shows the message
        records.append(step_record(dest, label, error=f'{type(error).__name__}: {error}'))


def plain_steps(source, dest, recipe, records):
    run_import(dest, records, 'import', source)
    ident = dest.photos()[0]['id']
    dest.edit(ident, {'exposure': .75})
    dest.update(ident, rating=5, keywords='keep mine')
    records.append(step_record(dest, 'edited'))
    run_import(dest, records, 'again', source)
    with sqlite3.connect(source) as db:
        recipe.sql(db, 'UPDATE Adobe_images SET rating=1 WHERE id_local=100')
    db.close()
    records.append(dict(step='source changed', recipe_length=len(recipe.ops)))
    run_import(dest, records, 'changed', source)


def existing_steps(source, dest, recipe, records):
    ident = dest.add(Path(PHOTOS) / 'photo0.jpg', settings={'exposure': 2})
    dest.update(ident, rating=5, keywords='Luma', label='초록')
    records.append(step_record(dest, 'added'))
    run_import(dest, records, 'import', source)


def convert_steps(source, dest, recipe, records):
    run_import(dest, records, 'import', source, convert_edits=True)
    master = next(p for p in dest.photos() if p['path'].endswith('photo0.jpg') and p['virtual_source'] is None)
    dest.edit(master['id'], {'exposure': 2.5})
    dest.save_preference(f'undo:{master["id"]}', {'undo': [{'exposure': 1}], 'redo': []})
    records.append(step_record(dest, 'edited'))
    run_import(dest, records, 'again', source, convert_edits=True)
    run_import(dest, records, 'without conversion', source)


def once(**kwargs):
    def steps(source, dest, recipe, records):
        run_import(dest, records, 'import', source, **kwargs)
    return steps


def cancel_at(stage):
    def steps(source, dest, recipe, records):
        dest.add(Path(PHOTOS) / 'keep.jpg')
        records.append(step_record(dest, 'added'))
        cancel = Event()

        def progress(status):
            if status['stage'] == stage:
                cancel.set()
        run_import(dest, records, 'import', source, cancel=cancel, progress=progress)
        records[-1]['pending'] = sorted(p.name for p in Path(dest.directory).glob('backups/*.pending'))
    return steps


def failing_steps(source, dest, recipe, records):
    def progress(status):
        if status['stage'] == '컬렉션과 스택 정리 중':
            raise OSError('simulated disk failure')
    run_import(dest, records, 'import', source, progress=progress)
    run_import(dest, records, 'itself', Path(dest.directory) / 'catalog.sqlite')


SCENARIOS = [
    ('plain', None, plain_steps),
    ('existing', None, existing_steps),
    ('convert', optin, convert_steps),
    ('gap', gap, once(convert_edits=True)),
    ('broken', broken, once()),
    ('broken_convert', broken, once(convert_edits=True)),
    ('smart', smart, once()),
    ('no_provider', no_provider, once()),
    ('cancel_backup', None, cancel_at('안전 백업 만드는 중')),
    ('cancel_reading', bulk, cancel_at('Lightroom 기록 읽는 중')),
    ('cancel_photos', None, cancel_at('사진 분류 가져오는 중')),
    ('cancel_collections', None, cancel_at('컬렉션과 스택 정리 중')),
    ('failing', None, failing_steps),
]


# ---- the window's flow ------------------------------------------------------------------------------------------

def flow():
    from PySide6.QtCore import QTimer
    from PySide6.QtWidgets import QApplication, QMessageBox, QWidget
    from luma import extras, lightroom_import_dialog
    app = QApplication.instance() or QApplication([])

    FixedUuid.count = 0
    Ticking.calls = 0
    recipe = Recipe()
    folder = ROOT / 'flow'
    folder.mkdir()
    source = folder / 'fixture.lrcat'
    build_source(recipe, source, optin)
    dest = Catalog(folder / 'luma')

    shown, reviews, answers = [], [], []

    class Holder:
        pass

    class Window(QWidget):
        def __init__(self):
            super().__init__()
            self.catalog = dest
            self.maintenance_running = False
            self.photo_model = Holder()
            self.photo_model.check_timer = QTimer(self)
            self.photo_model.check_timer.setInterval(1000)
            self.photo_model.check_timer.start()
            self.extras = Holder()
            self.extras.timer = QTimer(self)
            self.manager = Holder()
            self.manager.refresh_collections = lambda: shown.append(['refresh_collections'])
            self.import_busy = self.import_scans = self.export_running = False
            self.jobs = {}
            self.commits = 0

        def commit(self):
            self.commits += 1

        def save_keywords(self):
            pass

        def refresh_lists(self):
            shown.append(['refresh_lists'])

        def show_error(self, message):
            shown.append(['error', rel(message.strip().splitlines()[-1])])

        def spawn(self, work, done, failed=None):
            try:
                result = work()
            except Exception:   # noqa: BLE001
                (failed or self.show_error)(traceback.format_exc())
                return
            done(result)

    def information(parent, title, text):
        shown.append(['information', title, rel(text)])

    def exec_(box):
        dialog = window.extras.lightroom_dialog
        reviews.append(dict(title=box.windowTitle(), text=box.text(), check=box.checkBox().text(),
                            checked=box.checkBox().isChecked(),
                            yes=box.button(QMessageBox.StandardButton.Yes).text(),
                            no=box.button(QMessageBox.StandardButton.No).text(),
                            default_no=box.defaultButton() is box.button(QMessageBox.StandardButton.No),
                            label=dialog.label.text(), window=dialog.windowTitle(), note=dialog.note.text(),
                            running=window.maintenance_running))
        answer = answers.pop(0)
        box.checkBox().setChecked(answer['convert'])
        if answer.get('cancel'):
            dialog.request_cancel()
        return QMessageBox.StandardButton.Yes if answer['import'] else QMessageBox.StandardButton.No

    QMessageBox.information = staticmethod(information)
    QMessageBox.exec = exec_

    window = Window()
    steps = []
    script = [
        dict(step='decline', answer=dict(**{'import': False, 'convert': False})),
        dict(step='cancel in review', answer=dict(**{'import': True, 'convert': True, 'cancel': True})),
        dict(step='import', answer=dict(**{'import': True, 'convert': True})),
        dict(step='again', answer=dict(**{'import': True, 'convert': False})),
    ]
    for item in script:
        shown.clear()
        reviews.clear()
        answers[:] = [item['answer']]
        lightroom_import_dialog.begin_import(window, str(source))
        dialog = window.extras.lightroom_dialog
        steps.append(dict(step=item['step'], answer=item['answer'], reviews=rel(reviews), shown=list(shown),
                          running=window.maintenance_running, finished=not dialog.running,
                          timer=window.photo_model.check_timer.isActive(),
                          last=rel(getattr(window.extras, 'last_lightroom_import', None)),
                          tables=tables(dest.db)))
    steps = changes_only(steps)

    # 최근 Lightroom 가져오기 결과, and the refusal while a job runs.
    tools = extras.ExtraTools.__new__(extras.ExtraTools)
    tools.w = window
    shown.clear()
    tools.lightroom_report()
    report = list(shown)
    window.jobs = {'busy': None}
    try:
        tools.import_lightroom()
        refusal = None
    except ValueError as error:
        refusal = str(error)
    empty = Catalog(folder / 'empty')
    window.catalog = empty
    shown.clear()
    tools.lightroom_report()
    nothing = list(shown)
    empty.close()
    window.close()
    dest.close()
    app.processEvents()
    return dict(recipe=recipe.ops, steps=steps, report=report, refusal=refusal, nothing=nothing)


with tempfile.TemporaryDirectory(prefix='grainy-app-lightroom-', ignore_cleanup_errors=True) as temporary:
    ROOT = Path(temporary).resolve()
    results = [scenario(name, change, steps) for name, change, steps in SCENARIOS]
    with Case('app', 'lightroom_import') as case:
        case.json(stamp=STAMP, photos=PHOTOS, schema=list(SCHEMA['tables'].values()), scenarios=results)

    recipe = Recipe()
    (ROOT / 'inspect').mkdir()
    source = ROOT / 'inspect' / 'fixture.lrcat'
    build_source(recipe, source, smart)
    info = inspect_catalog(source)
    wrong = ROOT / 'inspect' / 'wrong.lrcat'
    with sqlite3.connect(wrong) as db:
        db.execute('CREATE TABLE Adobe_images(id_local INTEGER)')
    db.close()
    try:
        inspect_catalog(wrong)
        refused = None
    except ValueError as error:
        refused = str(error)
    with Case('app', 'lightroom_inspect') as case:
        case.json(recipe=recipe.ops, info=rel(info), refused=refused)

    with Case('app', 'lightroom_flow') as case:
        case.json(stamp=STAMP, **flow())
