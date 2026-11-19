"""Goldens of grainy-app's export completion (APP_PLAN package 13), made with the oracle's own code.

What is stored (`golden/app/export/case.json`), all from the Python Grainy's own methods:

* `dialog_formats`: the real `ExportDialog` (a QDialog driven offscreen): the format list (value and text) and, for
  each format, which fields `format_changed` enables and the colour space it leaves.
* `dialog_validate`: `ExportDialog.validate` for folders and naming rules (the message box texts it shows).
* `dialog_previous`: the values the dialog opens with for saved "previous export" preferences (valid, legacy,
  invalid, JPEG XL with a colour space, HDR format).
* `quick`: `MainWindow.quick_export` unbound on a stand-in window: what it does for every kind with and without
  saved settings, a missing folder, a cancelled folder dialog, no selection and a running export.
* `runs`: `MainWindow.start_export` unbound on a stand-in window over a real `Catalog` with real (small, generated)
  photos: the files and records it writes (names, sizes, the XMP sidecar text), the preferences `previous_photo_export`
  and `export_history`, the status messages and the failure boxes; then `export_history`, `show_exports` and
  `reset_exports` for the library the runs left.

Paths in the golden are relative to the photo folder (`<src>`) or the export folder (`<out>`); no clock, process id
or random value is stored (the history's `created` is the fixed `STAMP`).

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_export.py
"""
import json
import tempfile
from datetime import datetime
from pathlib import Path

from common import Case, np  # noqa: F401
from PySide6.QtWidgets import QApplication, QDialog, QTreeWidget, QWidget
from luma import app as app_module, workflow_data, workflow_ui
from luma.app import ExportDialog, MainWindow
from luma.catalog import Catalog
from luma.engine import DEFAULTS, normalized
from luma.quick_export import FORMATS
from luma.workflow_ui import WorkflowTools

STAMP = '2026-11-13T01:02:03.456789+00:00'
app = QApplication.instance() or QApplication([])


class Clock(datetime):
    @classmethod
    def now(cls, tz=None):
        value = datetime.fromisoformat(STAMP)
        return value if tz else value.replace(tzinfo=None)


workflow_data.datetime = Clock
import luma.catalog as catalog_module  # noqa: E402
catalog_module.datetime = Clock


class Box:
    """`QMessageBox` replaced: the calls are kept."""
    calls = []

    @staticmethod
    def information(parent, title, text):
        Box.calls.append(['information', title, text])

    @staticmethod
    def warning(parent, title, text):
        Box.calls.append(['warning', title, text])


class Files:
    """`QFileDialog` replaced: `getExistingDirectory` answers what is scripted and keeps its arguments."""
    answer = ''
    calls = []

    @staticmethod
    def getExistingDirectory(parent, title='', directory=''):
        Files.calls.append([title, directory])
        return Files.answer


app_module.QMessageBox = Box
app_module.QFileDialog = Files


class Parent(QWidget):
    alive = []

    def __init__(self, catalog):
        super().__init__()
        self.catalog = catalog
        Parent.alive.append(self)   # the dialog's widgets live as long as its parent


class Bar:
    def __init__(self):
        self.messages = []

    def showMessage(self, text, *args):
        self.messages.append(text)


class Nop:
    def __getattr__(self, name):
        return Nop()

    def __call__(self, *args, **kwargs):
        return Nop()

    def __bool__(self):
        return False


class Window:
    """The state `export_dialog`, `quick_export` and `start_export` read, with the window's own methods."""
    export_dialog_calls = 0
    quick_export = MainWindow.quick_export
    start_export = MainWindow.start_export

    def __init__(self, catalog):
        self.catalog = catalog
        self.export_running = False
        self.export_button = Nop()
        self.selection = []
        self.bar = Bar()
        self.started = []
        self.dialogs = 0
        self.errors = []
        self.last_export = None
        self.commits = 0

    def statusBar(self):
        return self.bar

    def commit(self):
        self.commits += 1

    def selected_ids(self):
        return list(self.selection)

    def export_dialog(self):
        self.dialogs += 1

    def show_error(self, message):
        self.errors.append(message.strip().splitlines()[-1])

    def spawn(self, work, finished, failed=None):
        """The worker pool, run at once on this thread."""
        try:
            result = work()
        except Exception as error:   # the oracle passes the traceback text; the last line is what is shown
            failed(f'Traceback\n{type(error).__name__}: {error}')
            return
        finished(result)


# ---- helpers ----------------------------------------------------------------------------------------------------

def rel(text, roots):
    """`text` with the temporary folders replaced: roots is a list of (folder, marker)."""
    if not isinstance(text, str):
        return text
    for folder, marker in roots:
        text = text.replace(str(folder), marker).replace(str(folder).replace('\\', '/'), marker)
    return text.replace('\\', '/')


def make_photo(path, gain, size=(48, 32)):
    from PIL import Image
    w, h = size
    x = np.linspace(0, 1, w, dtype=np.float32)[None, :, None]
    y = np.linspace(0, 1, h, dtype=np.float32)[:, None, None]
    image = np.concatenate([x * np.ones((h, 1, 1), np.float32), y * np.ones((1, w, 1), np.float32),
                            (1 - x) * (1 - y)], axis=2)
    image = np.clip(image * np.array(gain, np.float32), 0, 1)
    Image.fromarray((image * 255 + 0.5).astype(np.uint8)).save(path)


# ---- the dialog -------------------------------------------------------------------------------------------------

def dialog_state(dialog):
    return {'folder': dialog.folder.text(), 'format': dialog.format.currentData(),
            'format_text': dialog.format.currentText(), 'quality': dialog.quality.value(),
            'longest': dialog.longest.value(), 'color_space': dialog.color_space.currentText(),
            'keep_metadata': dialog.keep_metadata.isChecked(), 'grayscale': dialog.grayscale.isChecked(),
            'dither': dialog.dither.isChecked(), 'naming': dialog.naming.text()}


def dialog_enabled(dialog):
    return {'quality': dialog.quality.isEnabled(), 'color_space': dialog.color_space.isEnabled(),
            'longest': dialog.longest.isEnabled(), 'keep_metadata': dialog.keep_metadata.isEnabled(),
            'grayscale': dialog.grayscale.isEnabled(), 'dither': dialog.dither.isEnabled()}


def dialog_formats(catalog):
    dialog = ExportDialog(3, Parent(catalog))
    listed = [[dialog.format.itemData(i), dialog.format.itemText(i)] for i in range(dialog.format.count())]
    rows = []
    for i in range(dialog.format.count()):
        dialog.color_space.setCurrentText('Adobe RGB')
        dialog.format.setCurrentIndex(i)
        rows.append({'format': dialog.format.currentData(), 'enabled': dialog_enabled(dialog),
                     'color_space': dialog.color_space.currentText()})
    return {'listed': listed, 'rows': rows, 'initial': dialog_state(ExportDialog(3, Parent(catalog))),
            'minimum_quality': dialog.quality.minimum(), 'maximum_longest': dialog.longest.maximum()}


NAMES = ['{stem}_grainy', '{stem}', '{stem}_{n:03d}', '{n:03d}', '{n}', '', ' ', '{stem}/x', '{stem}\\x', 'a:b',
         'x*', '{film}', '{stem', '{}', '{stem}_{n:03d', '{stem!r}', 'x.', '{stem:>8}', '{n:5d}', '{n:x}', 'a?b',
         '{stem}{{x}}', 'photo name', 'あ{stem}', '{n:03}', '{0}']


def dialog_validate(catalog, folders):
    cases = []
    for folder in ('empty', 'blank', 'missing', 'dir'):
        text = {'empty': '', 'blank': '   ', 'missing': str(folders['missing']), 'dir': str(folders['out'])}[folder]
        for naming in (['{stem}_grainy'] if folder != 'dir' else NAMES):
            dialog = ExportDialog(1, Parent(catalog))
            dialog.folder.setText(text)
            dialog.naming.setText(naming)
            Box.calls.clear()
            dialog.validate()
            cases.append({'folder': folder, 'naming': naming, 'accepted': dialog.result() == QDialog.DialogCode.Accepted,
                          'box': list(Box.calls[0]) if Box.calls else None})
    return cases


PREVIOUS = {
    'full': dict(folder='D:/Exports', format='PNG 16-bit', quality=70, longest=1200, color_space='Display P3',
                 keep_metadata=True, naming='{stem}_{n:03d}', grayscale=False, dither=False),
    'legacy': dict(folder='D:/Exports', format='AVIF', quality=60, longest=0, color_space='sRGB', keep_metadata=False,
                   naming='{stem}'),
    'jxl_space': dict(folder='D:/Exports', format='JPEG XL', quality=88, longest=0, color_space='Adobe RGB',
                      keep_metadata=False, naming='{stem}', grayscale=True, dither=True),
    'original': dict(folder='D:/Exports', format='Original', quality=95, longest=500, color_space='ProPhoto RGB',
                     keep_metadata=True, naming='{stem}', grayscale=True, dither=True),
    'hdr': dict(folder='D:/Exports', format='TIFF HDR 32-bit', quality=95, longest=0, color_space='sRGB',
                keep_metadata=False, naming='{stem}', grayscale=True, dither=True),
    'bad_quality': dict(folder='D:/Exports', format='JPEG', quality=10, longest=0, color_space='sRGB',
                        keep_metadata=False, naming='{stem}', grayscale=True, dither=True),
    'bad_naming': dict(folder='D:/Exports', format='JPEG', quality=90, longest=0, color_space='sRGB',
                       keep_metadata=False, naming='a/b', grayscale=True, dither=True),
    'no_folder': dict(folder='', format='JPEG', quality=90, longest=0, color_space='sRGB', keep_metadata=False,
                      naming='{stem}', grayscale=True, dither=True),
    'bad_flag': dict(folder='D:/Exports', format='JPEG', quality=90, longest=0, color_space='sRGB',
                     keep_metadata=False, naming='{stem}', grayscale=1, dither=True),
}


def dialog_previous(catalog):
    cases = {}
    for name, value in [('none', None), *PREVIOUS.items(), ('not_a_dict', [1, 2])]:
        if value is None:
            catalog.db.execute("DELETE FROM preferences WHERE key='previous_photo_export'")
            catalog.db.commit()
        else:
            catalog.save_preference('previous_photo_export', value)
        dialog = ExportDialog(1, Parent(catalog))
        cases[name] = {'saved': value, 'state': dialog_state(dialog), 'enabled': dialog_enabled(dialog)}
    catalog.db.execute("DELETE FROM preferences WHERE key='previous_photo_export'")
    catalog.db.commit()
    return cases


# ---- the quick exports ------------------------------------------------------------------------------------------

def quick(catalog, folders):
    roots = [(folders['out'], '<out>'), (folders['missing'], '<missing>')]
    cases = []

    def run(name, kind, saved=None, selection=(1, 2), answer='', running=False):
        catalog.db.execute("DELETE FROM preferences WHERE key='previous_photo_export'")
        if saved is not None:
            catalog.save_preference('previous_photo_export', saved)
        catalog.db.commit()
        window = Window(catalog)
        window.selection = list(selection)
        window.export_running = running
        calls = []

        def fake_start(ids, folder, **options):
            calls.append({'ids': ids, 'folder': rel(str(folder), roots), 'options': options})
            return True
        window.start_export = fake_start
        Files.calls.clear()
        Files.answer = answer
        window.quick_export(kind)
        cases.append({'name': name, 'kind': kind, 'saved': rel_saved(saved, roots), 'selection': list(selection),
                      'answer': rel(answer, roots), 'running': running, 'started': calls,
                      'dialog_opened': window.dialogs, 'folder_dialogs': [[t, rel(d, roots)] for t, d in Files.calls],
                      'committed': window.commits})

    def rel_saved(saved, roots):
        return None if saved is None else {k: rel(v, roots) for k, v in saved.items()}

    out, missing = str(folders['out']), str(folders['missing'])
    saved_ok = dict(folder=out, format='PNG', quality=85, longest=900, color_space='Adobe RGB', keep_metadata=True,
                    naming='{stem}_x', grayscale=False, dither=True)
    saved_missing = dict(saved_ok, folder=missing)
    for kind in ('small', 'large', 'original'):
        run(f'{kind}_no_saved_answer', kind, None, answer=out)
        run(f'{kind}_no_saved_cancel', kind, None, answer='')
        run(f'{kind}_saved_answer', kind, saved_ok, answer=out)
        run(f'{kind}_saved_cancel', kind, saved_ok, answer='')
    run('previous_none', 'previous', None, answer=out)
    run('previous_ok', 'previous', saved_ok)
    run('previous_missing_answer', 'previous', saved_missing, answer=out)
    run('previous_missing_cancel', 'previous', saved_missing, answer='')
    run('previous_legacy', 'previous', {k: v for k, v in saved_ok.items() if k not in ('grayscale', 'dither')})
    run('previous_invalid', 'previous', dict(saved_ok, quality=5), answer=out)
    run('no_selection', 'small', saved_ok, selection=(), answer=out)
    run('running', 'previous', saved_ok, running=True)
    run('one_photo', 'large', None, selection=(3,), answer=out)
    return cases


# ---- the export runs --------------------------------------------------------------------------------------------

def options_of(**changes):
    values = dict(format='JPEG', quality=95, longest=0, color_space='sRGB', keep_metadata=False,
                  naming='{stem}_grainy', grayscale=True, dither=True)
    values.update(changes)
    return values


def listing(folder):
    return sorted(p.name for p in Path(folder).iterdir()) if Path(folder).is_dir() else []


def project(catalog, roots):
    """The preferences the exports write, paths relative; `created` is the fixed stamp."""
    previous = catalog.preference('previous_photo_export')
    history = catalog.preference('export_history', [])
    return {'previous': None if previous is None else {k: rel(v, roots) for k, v in previous.items()},
            'history': [{k: rel(v, roots) for k, v in item.items()} for item in history]}


def run_export(window, name, ids, folder, roots, **options):
    window.selection = list(ids)
    before = len(window.bar.messages)
    window.errors.clear()
    window.last_export = None
    Box.calls.clear()
    files_before = set(listing(folder))
    started = window.start_export(ids, folder, **options)
    result = window.last_export
    new_files = sorted(set(listing(folder)) - files_before)
    outputs = []
    if result:
        outputs = [{'path': rel(o['path'], roots), 'size': list(o['size']), 'photo_id': o['photo_id'],
                    'edit_signature': o['edit_signature']} for o in result['outputs']]
    return {'name': name, 'ids': list(ids), 'folder': rel(str(folder), roots), 'options': options, 'started': started,
            'messages': [rel(m, roots) for m in window.bar.messages[before:]], 'outputs': outputs,
            'failures': [rel(f, roots).split(':')[0] for f in (result['failures'] if result else [])],
            'failure_count': len(result['failures']) if result else None,
            'box': [list(c[:2]) + [rel(c[2], roots).count('\n') + 1] for c in Box.calls],
            'errors': list(window.errors), 'new_files': new_files, **project(window.catalog, roots)}


def runs(work, catalog, folders):
    out = folders['out']
    roots = [(out, '<out>'), (folders['src'], '<src>')]
    window = Window(catalog)
    results = []
    results.append(run_export(window, 'jpeg_two', (1, 2), out, roots, **options_of(longest=24)))
    results.append(run_export(window, 'jpeg_again_numbers', (1, 2), out, roots, **options_of(longest=24)))
    results.append(run_export(window, 'png16_numbered', (3, 4), out, roots,
                              **options_of(format='PNG 16-bit', naming='{stem}_{n:03d}', color_space='Display P3')))
    results.append(run_export(window, 'tiff8_dither_off', (1,), out, roots,
                              **options_of(format='TIFF 8-bit', dither=False, grayscale=False, naming='{stem}')))
    results.append(run_export(window, 'avif', (2,), out, roots, **options_of(format='AVIF', quality=60)))
    results.append(run_export(window, 'original_pair', (1, 3), out, roots,
                              **options_of(format='Original', naming='{stem}')))
    results.append(run_export(window, 'original_again', (1,), out, roots,
                              **options_of(format='Original', naming='{stem}')))
    # One photo whose file is gone: the others are written, the failure is listed, the history has the others.
    results.append(run_export(window, 'one_fails', (5, 1), out, roots, **options_of(naming='{stem}_mix')))
    # Nothing is written: the saved settings and the history stay as they were.
    results.append(run_export(window, 'all_fail', (5,), out, roots, **options_of(format='PNG', naming='{stem}_none')))
    # A folder that is not there: the oracle raises before anything starts.
    try:
        bad = run_export(window, 'no_folder', (1,), out / 'nowhere', roots, **options_of())
        results.append(bad)
    except ValueError as error:
        results.append({'name': 'no_folder', 'raised': str(error)})
    # Selected nothing: nothing starts.
    results.append(run_export(window, 'nothing_selected', (), out, roots, **options_of()))
    # The sidecar and the copy of the original.
    sidecars = {}
    for name in listing(out):
        if name.endswith('.xmp'):
            sidecars[name] = (out / name).read_bytes().decode('utf-8')
    copies = {name: (out / name).read_bytes() == (folders['src'] / name).read_bytes()
              for name in listing(out) if name.endswith('.png') and (folders['src'] / name).exists()
              and not name.endswith('_grainy.png')}
    results_files = listing(out)
    return {'runs': results, 'sidecars': sidecars, 'copies_equal_original': copies, 'files': results_files}


def history(catalog, folders):
    roots = [(folders['out'], '<out>'), (folders['src'], '<src>')]
    first = Window(catalog)
    ids_all = [1, 2, 3, 4, 5]
    shown = {}

    def records(ids):
        return [{k: rel(v, roots) for k, v in r.items()} for r in workflow_data.export_history(catalog, ids)]

    shown['all'] = records(ids_all)
    shown['one'] = records([1])
    shown['none'] = records([])
    # Photo 1 is edited after its export: `edited_since` turns true for it only.
    photo = catalog.photo(1)
    photo['settings']['exposure'] = 1.25
    catalog.set_settings(1, photo['settings'])
    shown['after_edit'] = records(ids_all)
    # A file is removed: the dialog says so.
    gone = sorted(p for p in Path(folders['out']).iterdir() if p.name.endswith('.avif'))
    for p in gone:
        p.unlink()

    class Flow:
        show_exports = WorkflowTools.show_exports
        reset_exports = WorkflowTools.reset_exports

        def __init__(self, window):
            self.w = window

    class Capture(QDialog):
        rows = None

        def __init__(self, parent=None):
            super().__init__(None)

        def exec(self):
            tree = self.findChildren(QTreeWidget)[0]
            headers = [tree.headerItem().text(i) for i in range(tree.columnCount())]
            Capture.rows = {'title': self.windowTitle(), 'headers': headers,
                            'rows': [[rel(tree.topLevelItem(r).text(c), roots) for c in range(tree.columnCount())]
                                     for r in range(tree.topLevelItemCount())]}
            return 0

    workflow_ui.QDialog = Capture
    window = Window(catalog)
    flow = Flow(window)
    for name, ids in (('all', ids_all), ('one', [2]), ('none', [])):
        window.selection = list(ids)
        flow.show_exports()
        shown['dialog_' + name] = Capture.rows
    window.selection = [1, 3]
    flow.reset_exports()
    shown['after_reset_1_3'] = [{k: rel(v, roots) for k, v in item.items()}
                                for item in catalog.preference('export_history', [])]
    window.selection = []
    flow.reset_exports()
    shown['after_reset_none'] = [{k: rel(v, roots) for k, v in item.items()}
                                 for item in catalog.preference('export_history', [])]
    return shown


def main():
    with tempfile.TemporaryDirectory(prefix='grainy-app-export-golden-', ignore_cleanup_errors=True) as temporary:
        work = Path(temporary).resolve()
        folders = {'src': work / 'src', 'out': work / 'out', 'lib': work / 'lib', 'missing': work / 'missing'}
        for key in ('src', 'out', 'lib'):
            folders[key].mkdir()
        photos = [('a.png', (1.0, 1.0, 1.0), {'exposure': 0.4}), ('b.png', (1.0, 0.8, 0.6), {'saturation': 12.0, 'rotation': 1}),
                  ('c.png', (0.9, 0.9, 0.9), {'monochrome': True}), ('d.png', (0.6, 0.8, 1.0), {})]
        catalog = Catalog(folders['lib'])
        for name, gain, settings in photos:
            make_photo(folders['src'] / name, gain)
            ident = catalog.add(folders['src'] / name, settings=normalized(settings),
                                metadata={'format': 'png', 'width': 48, 'height': 32})
        # Photo 5: its file is gone.
        make_photo(folders['src'] / 'gone.png', (1, 1, 1))
        catalog.add(folders['src'] / 'gone.png', settings=normalized({'contrast': 10.0}),
                    metadata={'format': 'png', 'width': 48, 'height': 32})
        (folders['src'] / 'gone.png').unlink()
        data = {'stamp': STAMP, 'formats': list(FORMATS), 'defaults_order': list(DEFAULTS)}
        data['dialog_formats'] = dialog_formats(catalog)
        data['dialog_validate'] = dialog_validate(catalog, folders)
        data['dialog_previous'] = dialog_previous(catalog)
        data['quick'] = quick(catalog, folders)
        data['runs'] = runs(work, catalog, folders)
        data['history'] = history(catalog, folders)
        catalog.close()
    with Case('app', 'export') as case:
        case.json(**data)


main()
