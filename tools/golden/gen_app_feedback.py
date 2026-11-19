"""Goldens of the feedback, updates, safe start and cloud backup package of grainy-app (APP_PLAN package 31):
`golden/app/feedback*/`.

The oracle's own code is driven headless (nothing is reimplemented), nothing leaves the machine:

* `feedback_dialog`: `FeedbackDialog` with a fake sender: the texts of the window, the empty text, a failed send, the
  retry with the same text (same reference), a new text (new reference), the success line and the records in the
  settings file; both languages. References are chosen here (`AAAAA`, ...) so the goldens are fixed.
* `feedback_update`: `UpdateController.startup` (first-run question and the daily check), `after_crash` and `install`
  (refusal while a job runs, failure, cancel, hand-over) with a fake fetcher, fake `download` / `stage` / `launch`
  and the message boxes recorded instead of shown.
* `feedback_safe_start`: `safe_start.begin` / `end` on real files in a temporary folder, and what
  `MainWindow.restore_folder_selection` asks for in safe mode and not.
* `feedback_cloud_functions`: `upload`, `surplus`, `trim` (fake recycle bin), `digest`.
* `feedback_cloud_backup`: `CloudBackup.settings` / `save` / `finish` / `start` and the 구글 드라이브 백업 dialog
  (`cloud_backup.dialog`) with the dialog's `exec` replaced by scripted user actions.
* `feedback_file_access`: the box of `file_access.guide` for both systems.

Every file the oracle writes is in a temporary folder made here and left out of the output (no paths, clocks or
process ids are stored: paths are replaced by `<folder>`, time stamps by `<stamp>`).

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_feedback.py
"""
import hashlib
import json
import platform
import re
import shutil
import sqlite3
import sys
import tempfile
import time
import types
from contextlib import closing
from datetime import datetime
from pathlib import Path

from common import Case

from PySide6.QtWidgets import (QApplication, QCheckBox, QDialog, QDialogButtonBox, QLabel, QLineEdit, QMainWindow,  # noqa: E402
                               QMenu, QMessageBox, QPlainTextEdit, QPushButton, QWidget)

import luma  # noqa: E402
from luma import cloud_backup, feedback, file_access, i18n, native_gpu, safe_start, updater  # noqa: E402
from luma.preferences import Preferences  # noqa: E402

_qt = QApplication.instance() or QApplication([])
WORK = Path(tempfile.mkdtemp(prefix='grainy-feedback-golden-'))


def fresh(name):
    folder = WORK / name
    shutil.rmtree(folder, ignore_errors=True)
    folder.mkdir(parents=True)
    return folder


def clean(text, *folders):
    """A text without this machine's paths and clocks."""
    text = str(text)
    for folder in folders:
        text = text.replace(str(folder), '<folder>').replace(str(folder).replace('\\', '/'), '<folder>')
    text = re.sub(r'\d{4}-\d\d-\d\d[ T]\d\d:\d\d:\d\d', '<time>', text)
    return re.sub(r'\d{8}-\d{6}', '<stamp>', text)


# ---- feedback --------------------------------------------------------------------------------------------------------

FACTS = {'system': 'windows', 'mac_version': '', 'windows_release': '11', 'windows_version': '10.0.26200',
         'machine': 'AMD64', 'adapter': 'Test GPU', 'gpu_enabled': True}
REFS = ['AAAAA', 'BBBBB', 'CCCCC', 'DDDDD', 'EEEEE']


def feedback_run(language):
    i18n.set_language(language)
    saved = (platform.release, platform.version, platform.machine, feedback.reference, native_gpu.adapter, native_gpu.enabled)
    platform.release, platform.version, platform.machine = lambda: '11', lambda: '10.0.26200', lambda: 'AMD64'
    refs = iter(REFS)
    feedback.reference = lambda: next(refs)
    native_gpu.adapter, native_gpu.enabled = 'Test GPU', (lambda: True)
    try:
        folder = fresh('feedback-' + language)
        prefs = Preferences(folder / 'settings.json')
        calls = []
        outcomes = iter(['boom', '', '', 'boom', ''])

        def sender(text, system):
            calls.append([text, system])
            return next(outcomes)

        dialog = feedback.FeedbackDialog(QWidget(), sender=sender, preferences=prefs)
        steps = []

        def snap(name):
            sent = prefs.values.get('feedback_sent', [])
            steps.append({'name': name, 'status': dialog.status.text(), 'send_enabled': dialog.send_button.isEnabled(),
                          'message': dialog.message.toPlainText(), 'calls': [list(c) for c in calls],
                          'sent': [{'ref': e['ref'], 'version': e['version'],
                                    'date': bool(re.fullmatch(r'\d{4}-\d\d-\d\d', e['sent']))} for e in sent]})

        def send():
            dialog.submit()
            if dialog.thread is not None:
                snap('sending')
                steps[-1]['calls'] = None      # the thread may or may not have run yet
                dialog.thread.join()
                dialog.check()

        texts = {'title': dialog.dialog.windowTitle(), 'labels': [label.text() for label in dialog.dialog.findChildren(QLabel)],
                 'placeholder': dialog.message.placeholderText(), 'send': dialog.send_button.text(),
                 'close': dialog.close_button.text(), 'system': dialog.system, 'size': [dialog.dialog.width(), dialog.dialog.height()]}
        snap('opened')
        dialog.message.setPlainText('   ')
        dialog.submit()
        snap('empty')
        dialog.message.setPlainText('  첫 번째 의견  ')
        send()
        snap('failed')
        send()      # the same text again: the same reference
        snap('sent')
        dialog.message.setPlainText('둘째 의견')
        send()
        snap('second')
        dialog.message.setPlainText('x')
        send()
        snap('failed x')
        dialog.message.setPlainText('y')      # another text: another reference
        send()
        snap('sent y')
        return {'language': language, 'texts': texts, 'steps': steps}
    finally:
        platform.release, platform.version, platform.machine, feedback.reference, native_gpu.adapter, native_gpu.enabled = saved
        i18n.set_language('ko')


def system_lines():
    """`system_info` for several machines."""
    lines = []
    saved = (platform.release, platform.version, platform.machine, platform.mac_ver, native_gpu.adapter, native_gpu.enabled, sys.platform)
    try:
        platform.release, platform.version, platform.machine = lambda: '11', lambda: '10.0.26200', lambda: 'AMD64'
        platform.mac_ver = lambda: ('14.5', ('', '', ''), 'arm64')
        for system, adapter, enabled in (('windows', 'Test GPU', True), ('windows', 'Test GPU', False), ('windows', '', False),
                                         ('mac', 'Apple M2', True)):
            sys.platform = 'darwin' if system == 'mac' else 'win32'
            native_gpu.adapter, native_gpu.enabled = adapter, (lambda e=enabled: e)
            lines.append({'system': system, 'adapter': adapter, 'enabled': enabled, 'line': feedback.system_info()})
    finally:
        platform.release, platform.version, platform.machine, platform.mac_ver, native_gpu.adapter, native_gpu.enabled, sys.platform = saved
    return lines


with Case('app', 'feedback_dialog') as case:
    case.json(runs=[feedback_run('ko'), feedback_run('en')], ref_order=REFS, facts=FACTS, system_lines=system_lines(),
              limit=feedback.LIMIT)

# ---- the update controller ------------------------------------------------------------------------------------------

boxes, infos, warnings = [], [], []
choice = [None]      # the index of the button pressed


def record_exec(self):
    boxes.append({'title': self.windowTitle(), 'text': self.text(), 'informative': self.informativeText(),
                  'detailed': self.detailedText(), 'buttons': [b.text() for b in self.buttons()]})
    return 0


def pick_button(self):
    buttons = self.buttons()
    return buttons[choice[0]] if isinstance(choice[0], int) and choice[0] < len(buttons) else None


QMessageBox.exec = record_exec
QMessageBox.clickedButton = pick_button
QMessageBox.information = staticmethod(lambda parent, title, text, *a, **k: infos.append({'title': title, 'text': text}) or QMessageBox.StandardButton.Ok)
QMessageBox.warning = staticmethod(lambda parent, title, text, *a, **k: warnings.append({'title': title, 'text': text}) or QMessageBox.StandardButton.Ok)
updater.cleanup = lambda: None
RELEASE = {'version': '9.9.9', 'name': 'Grainy-9.9.9-windows-x64.zip', 'url': 'https://example.invalid/x.zip', 'size': 1234,
           'sha256': 'ab' * 32, 'notes': ''}


def controller(name, language, values, packaged=True):
    i18n.set_language(language)
    main = QMainWindow()
    main.setObjectName(name)
    menu = main.menuBar().addMenu('설정')
    menu.setObjectName('settingsMenu')
    main.update_button = QPushButton()
    main.export_running = main.import_busy = main.import_scans = False
    prefs = Preferences(fresh('update-' + name + language) / 'settings.json')
    for key, value in values.items():
        prefs.save(key, value)
    fetched = []
    ctl = updater.UpdateController(main, prefs, fetcher=lambda: fetched.append(1) or None, packaged=packaged)
    return main, ctl, prefs, fetched


def settle_check(ctl):
    if ctl.thread is not None:
        ctl.thread.join()
        ctl.checked()


def scenario(name, language, values, action, picked=None, packaged=True):
    boxes.clear(); infos.clear(); warnings.clear()
    choice[0] = picked
    main, ctl, prefs, fetched = controller(name, language, values, packaged)
    action(ctl, main)
    settle_check(ctl)
    stored = prefs.values
    return {'name': name, 'language': language, 'picked': picked, 'boxes': list(boxes), 'infos': list(infos),
            'fetch_calls': len(fetched), 'update_check': stored.get('update_check'),
            'update_checked_saved': 'update_checked' in stored, 'status': main.statusBar().currentMessage()}


def install_scenario(name, language, mode):
    boxes.clear(); infos.clear(); warnings.clear()
    main, ctl, prefs, fetched = controller('install-' + name, language, {})
    app = fresh('app-' + name + language) / 'app'
    app.mkdir()
    closed, launched = [], []
    main.close = lambda: closed.append(True)
    saved = (updater.app_folder, updater.download, updater.stage, updater.launch, updater.restart_arguments)
    updater.app_folder = lambda: app

    def download(release, folder, progress, cancel):
        if mode == 'fail':
            raise OSError('boom')
        if mode == 'cancel':
            return None
        progress(10, 10)
        return Path(folder) / 'x.zip'

    updater.download = download
    updater.stage = lambda archive, work: Path(work) / 'new'
    updater.restart_arguments = lambda: ['--probe']
    updater.launch = lambda staged, a, args: launched.append({'staged': Path(staged).name, 'app': Path(a).name, 'args': list(args)})
    if mode == 'busy':
        main.export_running = True
    try:
        result = ctl.install(dict(RELEASE))
    finally:
        updater.app_folder, updater.download, updater.stage, updater.launch, updater.restart_arguments = saved
    return {'name': name, 'language': language, 'result': result, 'infos': list(infos), 'warnings': list(warnings),
            'launched': launched, 'closed': bool(closed)}


updates = []
for language in ('ko', 'en'):
    updates.append(scenario('first run yes', language, {}, lambda c, m: c.startup(), 0))
    updates.append(scenario('first run no', language, {}, lambda c, m: c.startup(), 1))
    updates.append(scenario('due never checked', language, {'update_check': True}, lambda c, m: c.startup()))
    updates.append(scenario('due long ago', language, {'update_check': True, 'update_checked': time.time() - updater.INTERVAL - 5},
                            lambda c, m: c.startup()))
    updates.append(scenario('not due', language, {'update_check': True, 'update_checked': time.time() - 60}, lambda c, m: c.startup()))
    updates.append(scenario('off', language, {'update_check': False}, lambda c, m: c.startup()))
    updates.append(scenario('not packaged', language, {}, lambda c, m: c.startup(), packaged=False))
    yes, no = 0, 1
    updates.append(scenario('crash asks, yes', language, {}, lambda c, m: c.after_crash(), yes))
    updates.append(scenario('crash asks, continue', language, {}, lambda c, m: c.after_crash(), no))
    updates.append(scenario('crash checks at once', language, {'update_check': True}, lambda c, m: c.after_crash()))
    updates.append(scenario('crash not packaged', language, {}, lambda c, m: c.after_crash(), packaged=False))
installs = [install_scenario(mode, language, mode) for language in ('ko', 'en') for mode in ('busy', 'fail', 'cancel', 'ok')]
i18n.set_language('ko')
with Case('app', 'feedback_update') as case:
    case.json(updates=updates, installs=installs, interval=updater.INTERVAL)

# ---- safe start ---------------------------------------------------------------------------------------------------------

steps = []


def marker(folder):
    path = Path(folder) / safe_start.NAME
    if not path.exists():
        return None
    data = json.loads(path.read_text(encoding='utf-8'))
    return {'version': data.get('version'), 'pid_is_own': data.get('pid') == __import__('os').getpid(),
            'started_is_number': isinstance(data.get('started'), (int, float))}


def safe_step(name, action, folder):
    result = action()
    steps.append({'name': name, 'result': result, 'marker': marker(folder)})


folder = fresh('safe-start')
safe_step('begin on an empty folder', lambda: safe_start.begin(folder) is not None, folder)
safe_step('begin again: the run before did not close', lambda: safe_start.begin(folder) is not None, folder)
safe_step('end removes our marker', lambda: safe_start.end(folder), folder)
safe_step('begin after a clean end', lambda: safe_start.begin(folder) is not None, folder)
(Path(folder) / safe_start.NAME).write_text(json.dumps({'pid': 99999999, 'version': '0.0.1', 'started': 1.0}), encoding='utf-8')
safe_step('begin over a dead run (the previous version)', lambda: (safe_start.begin(folder) or {}).get('version'), folder)
(Path(folder) / safe_start.NAME).write_text(json.dumps({'pid': 99999999, 'version': '0.0.2', 'started': 1.0}), encoding='utf-8')
safe_step('end leaves a marker of another run', lambda: safe_start.end(folder), folder)
(Path(folder) / safe_start.NAME).write_text('not json', encoding='utf-8')
safe_step('begin over a damaged marker', lambda: safe_start.begin(folder) is not None, folder)

from luma.app import MainWindow  # noqa: E402

restores = []
for safe in (False, True):
    for saved_photo in (7, None):
        calls = []
        stand = types.SimpleNamespace(folder_preferences={'current_photo': saved_photo}, folder_filter='', safe_start=safe)
        stand.refresh_lists = lambda ensure_current=False, calls=calls: calls.append(ensure_current)
        stand.set_mode = lambda mode: None
        if not safe:
            del stand.safe_start
        MainWindow.restore_folder_selection(stand)
        restores.append({'safe': safe, 'saved': saved_photo, 'restore_photo_id': stand.restore_photo_id, 'ensure_current': calls[0]})
with Case('app', 'feedback_safe_start') as case:
    case.json(steps=steps, restores=restores, name=safe_start.NAME)

# ---- cloud backup: the functions --------------------------------------------------------------------------------------


def make_sqlite(path, rows):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.unlink(missing_ok=True)
    with closing(sqlite3.connect(path)) as db:
        db.execute('CREATE TABLE t (a INTEGER, b TEXT)')
        db.executemany('INSERT INTO t VALUES (?,?)', [(i, 'x' * 10) for i in range(rows)])
        db.commit()
    return path


recycled = []
fake = types.ModuleType('send2trash')


def fake_trash(path):
    recycled.append(Path(path).name)
    Path(path).unlink()


fake.send2trash = fake_trash
sys.modules['send2trash'] = fake

functions = {}
work = fresh('cloud-functions')
staged = make_sqlite(work / 'a.sqlite', 3)
content = staged.read_bytes()
functions['digest'] = {'hex': content.hex(), 'sha256': cloud_backup.digest(staged)}
target_folder = work / 'target'
now = datetime(2026, 3, 4, 5, 6, 7)
uploads = []


def do_upload(name, data, last_hash=None, keep=cloud_backup.KEEP, when=now):
    s = work / (name + '.staged.sqlite')
    s.write_bytes(data)
    try:
        result = cloud_backup.upload(s, target_folder, last_hash, keep, when)
        out = {'name': name, 'target': None if result[0] is None else result[0].name, 'hash': result[1], 'error': None}
        if result[0] is not None:
            out['copied'] = result[0].read_bytes() == data
    except Exception as error:
        out = {'name': name, 'target': None, 'hash': None, 'error': type(error).__name__}
    out['staged_removed'] = not s.exists()
    out['part_files'] = len(list(target_folder.glob('*.part'))) if target_folder.exists() else 0
    uploads.append(out)
    return out


first = do_upload('first', content)
do_upload('unchanged', content, last_hash=first['hash'])
other = make_sqlite(work / 'b.sqlite', 9).read_bytes()
uploads_other = do_upload('changed', other, last_hash=first['hash'], when=datetime(2026, 3, 4, 5, 6, 8))
do_upload('same second', other, last_hash=None, when=datetime(2026, 3, 4, 5, 6, 8))
do_upload('same second again', other, last_hash=None, when=datetime(2026, 3, 4, 5, 6, 8))
do_upload('not a database', b'this is not a database file' * 50)
functions['uploads'] = uploads
functions['uploads_other_hex'] = other.hex()
functions['folder_listing'] = sorted(p.name for p in target_folder.iterdir())

names = ([f'{cloud_backup.PREFIX}2026{m:02d}{d:02d}-120000.sqlite' for m in (1, 2, 3) for d in (1, 5, 9)]
         + [f'{cloud_backup.PREFIX}20251231-235959.sqlite', f'{cloud_backup.PREFIX}20251201-000001.sqlite'])
functions['surplus'] = [{'keep': keep, 'names': names, 'out': cloud_backup.surplus(names, keep)} for keep in (20, 6, 3, 1, 0)]
trim_folder = fresh('cloud-trim')
for n in names:
    (trim_folder / n).write_bytes(b'x')
(trim_folder / 'other.txt').write_bytes(b'x')
(trim_folder / (cloud_backup.PREFIX + 'sub.sqlite')).mkdir()
recycled.clear()
cloud_backup.trim(trim_folder, 3)
functions['trim'] = {'names': names, 'keep': 3, 'recycled': sorted(recycled), 'left': sorted(p.name for p in trim_folder.iterdir())}
functions['constants'] = {'pref': cloud_backup.PREF, 'prefix': cloud_backup.PREFIX, 'folder_name': cloud_backup.FOLDER_NAME,
                          'keep': cloud_backup.KEEP, 'interval_ms': cloud_backup.INTERVAL_MS, 'close_timeout': cloud_backup.CLOSE_TIMEOUT}
with Case('app', 'feedback_cloud_functions') as case:
    case.json(**functions)

# ---- cloud backup: the controller and the dialog -----------------------------------------------------------------------

from luma.catalog import Catalog  # noqa: E402


class Window(QMainWindow):
    def __init__(self, folder):
        super().__init__()
        self.catalog = Catalog(folder)
        self.closing = False
        self.cloud_backup = cloud_backup.CloudBackup(self)

    def commit(self):
        pass

    def save_keywords(self):
        pass


def join(backup):
    if backup.thread is not None:
        backup.thread.join()
        backup.check()


results = []


def backup_window(name):
    home = fresh('cloud-' + name)
    window = Window(home / 'library')
    return window, home


# finish: the messages
for language in ('ko', 'en'):
    i18n.set_language(language)
    window, home = backup_window('finish' + language)
    backup = window.cloud_backup
    notes = []
    out = {'language': language, 'defaults': backup.settings()}
    backup.finish(None, 'h1', None, False, notes.append)
    out['skipped'] = {'notes': list(notes), 'settings': {k: v for k, v in backup.settings().items() if k != 'last'}, 'last_empty': backup.settings()['last'] == ''}
    notes.clear()
    backup.finish(Path('X') / (cloud_backup.PREFIX + '20260304-050607.sqlite'), 'h2', None, True, notes.append)
    s = backup.settings()
    out['target'] = {'notes': [clean(n) for n in notes], 'hash': s['hash'], 'error': s['error'],
                     'last_is_iso': bool(re.fullmatch(r'\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d', s['last']))}
    window.statusBar().clearMessage()
    backup.finish(None, None, OSError('boom'), True, None)
    out['error'] = {'status': window.statusBar().currentMessage(), 'settings_error': backup.settings()['error']}
    window.statusBar().clearMessage()
    backup.finish(None, 'h3', None, True, None)
    out['manual_skipped_status'] = window.statusBar().currentMessage()
    window.statusBar().clearMessage()
    backup.finish(None, 'h3', None, False, None)
    out['auto_skipped_status'] = window.statusBar().currentMessage()
    results.append(out)
i18n.set_language('ko')

# start: the conditions and a real backup into a folder
window, home = backup_window('start')
backup = window.cloud_backup
target = home / 'drive'
start_cases = []
start_cases.append({'name': 'no folder', 'manual': True, 'started': backup.start(True)})
backup.save(folder=str(target))
start_cases.append({'name': 'automatic but off', 'manual': False, 'started': backup.start(False)})
notes = []
started = backup.start(True, notes.append)
join(backup)
files = sorted(p.name for p in target.glob('*.sqlite'))
s = backup.settings()
start_cases.append({'name': 'manual', 'manual': True, 'started': started, 'notes': [clean(n, target) for n in notes],
                    'files': [clean(f) for f in files], 'error': s['error'], 'hash_is_hex': bool(re.fullmatch(r'[0-9a-f]{64}', s['hash'])),
                    'last_is_iso': bool(s['last']), 'staging_left': len(list((home / 'library' / 'backups' / 'cloud-staging').glob('*')))})
backup.save(enabled=True)
notes.clear()
second = backup.start(False, notes.append)
join(backup)
start_cases.append({'name': 'automatic', 'manual': False, 'started': second})
window.closing = True
start_cases.append({'name': 'closing', 'manual': True, 'started': backup.start(True)})
window.closing = False

# the dialog
captured = []
actions = {}


def fake_exec(self):
    captured.append({
        'title': self.windowTitle(),
        'labels': [label.text() for label in self.findChildren(QLabel)],
        'checkbox': [c.text() for c in self.findChildren(QCheckBox)], 'checked': [c.isChecked() for c in self.findChildren(QCheckBox)],
        'line': [e.text() for e in self.findChildren(QLineEdit)],
        'buttons': [b.text() for b in self.findChildren(QPushButton)],
    })
    if actions.get('script'):
        actions['script'](self)
    return actions.get('result', QDialog.DialogCode.Rejected)


QDialog.exec = fake_exec


def widget(d, kind, text=None):
    for w in d.findChildren(kind):
        if text is None or w.text() == text:
            return w


def run_dialog(name, language, settings, drive, script=None, accept=True):
    i18n.set_language(language)
    window, home = backup_window('dialog-' + name + language)
    cloud_backup.google_drive = lambda: (home / 'My Drive') if drive else None
    if drive:
        (home / 'My Drive').mkdir()
    window.cloud_backup.save(**{k: (str(home / v) if k == 'folder' and v else v) for k, v in settings.items()})
    captured.clear()
    actions['script'] = (lambda d: script(d, home, window)) if script else None
    actions['result'] = QDialog.DialogCode.Accepted if accept else QDialog.DialogCode.Rejected
    cloud_backup.dialog(window)
    join(window.cloud_backup)
    s = window.cloud_backup.settings()
    files = sorted(p.name for p in (home / 'out').glob('*.sqlite')) if (home / 'out').exists() else []
    first = captured[0]
    return {'name': name, 'language': language, 'drive': bool(drive),
            'opened': {k: ([clean(x, home) for x in v] if isinstance(v, list) and v and isinstance(v[0], str) else v) for k, v in first.items()},
            'settings': {'enabled': s['enabled'], 'folder': clean(s['folder'], home), 'error': clean(s['error'], home),
                         'last_is_iso': bool(s['last']), 'hash_is_hex': bool(s['hash'])},
            'files': [clean(f) for f in files], 'status_bar': window.statusBar().currentMessage(),
            'final': {k: ([clean(x, home) for x in v] if isinstance(v, list) and v and isinstance(v[0], str) else v)
                      for k, v in (captured[-1].items())}}


dialogs = []
for language in ('ko', 'en'):
    dialogs.append(run_dialog('plain', language, {}, False, accept=False))
    dialogs.append(run_dialog('drive', language, {}, True, accept=False))
    dialogs.append(run_dialog('saved', language, {'enabled': True, 'folder': 'out', 'last': '2026-01-02T03:04:05', 'error': 'boom'}, True, accept=False))

    def enable_and_accept(d, home, window):
        widget(d, QCheckBox).setChecked(True)
        widget(d, QLineEdit).setText(str(home / 'out'))

    dialogs.append(run_dialog('enable and accept', language, {}, True, script=enable_and_accept))

    def enable_without_folder(d, home, window):
        widget(d, QCheckBox).setChecked(True)
        widget(d, QLineEdit).setText('  ')

    dialogs.append(run_dialog('enable without a folder', language, {}, False, script=enable_without_folder))

    def now_without_folder(d, home, window):
        widget(d, QLineEdit).setText('')
        widget(d, QPushButton, i18n.tr('지금 백업')).click()
        captured.append({'status': [label.text() for label in d.findChildren(QLabel)]})

    dialogs.append(run_dialog('now without a folder', language, {}, False, script=now_without_folder, accept=False))

    def now_with_folder(d, home, window):
        widget(d, QLineEdit).setText(str(home / 'out'))
        widget(d, QPushButton, i18n.tr('지금 백업')).click()
        join(window.cloud_backup)
        captured.append({'status': [clean(label.text(), home) for label in d.findChildren(QLabel)],
                         'now_enabled': widget(d, QPushButton, i18n.tr('지금 백업')).isEnabled()})

    dialogs.append(run_dialog('now with a folder', language, {}, False, script=now_with_folder, accept=False))
i18n.set_language('ko')
with Case('app', 'feedback_cloud_backup') as case:
    case.json(finish=results, start=start_cases, dialogs=dialogs)

# ---- file access ----------------------------------------------------------------------------------------------------------

guides = []
original_mac = file_access.MAC
for mac in (False, True):
    for language in ('ko', 'en'):
        for place in ('', 'D:\\Photos'):
            i18n.set_language(language)
            file_access.MAC = mac
            file_access.shown = False
            boxes.clear()
            shown = file_access.guide(QWidget(), place)
            guides.append({'mac': mac, 'language': language, 'place': place, 'shown': shown, 'box': boxes[0] if boxes else None})
file_access.MAC = original_mac
file_access.shown = False
i18n.set_language('ko')
with Case('app', 'feedback_file_access') as case:
    case.json(guides=guides, url=file_access.SETTINGS_URL)

shutil.rmtree(WORK, ignore_errors=True)
