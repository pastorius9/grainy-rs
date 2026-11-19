"""Goldens of the settings and small dialogs package of grainy-app (APP_PLAN package 26): `golden/app/settings*/`.

The oracle's own code is driven headless (nothing is reimplemented):

* `settings_menu`: the real `MainWindow` with the real `DesktopSession.open_window` additions (update controller,
  separator, 이번 버전의 달라진 점…, Grainy 정보…): the 설정 menu's items in order.
* `settings_language_dialog`: `LanguageDialog` for the first run and for a change, with the language in use.
* `settings_language_change`: `DesktopSession.change_language` on the real window: the refusal while a job runs, the
  same language, a change (the preference written, the saved photo, mode and panel state handed to the new window).
* `settings_whats_new`: `whats_new.show` composed for the notes of this version (both languages), no notes, and
  the shipped-feedback line: title, heading, line, markdown given to the text view.
* `settings_about`: `MainWindow.show_about` and `show_help` / the learn link (the URL opened).
* `settings_updates`: `UpdateController.checked` after a manual or automatic check that fails, finds nothing or
  finds a newer release: the boxes shown (title, text, button names) and the status message.
* `settings_screen_modes`: `ScreenModes` (`toggle_sides`, `toggle_bars`, `toggle_lights`, `leave`) on a stand-in
  window: which parts of the window are visible after each step.
* `settings_preferences`: the settings file after the preference writes of this package (language, update_check,
  version_seen, use_gpu) and the library preference `drt_advanced` of `toggle_drt_advanced`.

Every file the oracle writes is in a temporary folder made here and left out of the output (no paths, clocks or
process ids are stored).

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_settings.py
"""
import json
import shutil
import tempfile
from pathlib import Path

from common import Case

from PySide6.QtCore import QUrl  # noqa: E402
from PySide6.QtGui import QDesktopServices  # noqa: E402
from PySide6.QtWidgets import (QApplication, QDialog, QDialogButtonBox, QLabel, QMainWindow, QMenu, QMessageBox,  # noqa: E402
                               QPushButton, QSplitter, QTextBrowser, QWidget)

import luma  # noqa: E402
from luma import i18n, updater, whats_new  # noqa: E402
from luma.language_dialog import LanguageDialog  # noqa: E402
from luma.preferences import Preferences  # noqa: E402
from luma.screen_modes import ScreenModes  # noqa: E402

_qt = QApplication.instance() or QApplication([])
WORK = Path(tempfile.mkdtemp(prefix='grainy-settings-golden-'))


def fresh(name):
    folder = WORK / name
    shutil.rmtree(folder, ignore_errors=True)
    folder.mkdir(parents=True)
    return folder


def label_texts(widget):
    return [label.text() for label in widget.findChildren(QLabel)]


# ---- the Settings menu --------------------------------------------------------------------------------------------

def menu_rows(menu, skip_checked=()):
    rows = []
    for action in menu.actions():
        if action.isSeparator():
            rows.append({'separator': True})
            continue
        text, _, shortcut = action.text().partition('\t')
        row = {'text': text, 'shortcut': shortcut, 'checkable': action.isCheckable()}
        if action.isCheckable() and text not in skip_checked:
            row['checked'] = action.isChecked()
        rows.append(row)
    return rows


def build_window(name, language='ko'):
    """The real window, opened as DesktopSession.open_window does (data and preferences in a temporary folder)."""
    from luma.app import MainWindow
    from luma.desktop_session import DesktopSession
    folder = fresh(name)
    preferences = Preferences(folder / 'settings.json')
    i18n.set_language(language)
    window = MainWindow(folder / 'data')
    session = DesktopSession(_qt, folder / 'data', preferences, window_factory=lambda data: window)
    session.open_window()
    return window, session, preferences


window, session, preferences = build_window('menu')
with Case('app', 'settings_menu') as case:
    # The GPU row depends on the machine (adapter present, tooltip), so only its text is stored.
    case.json(rows=menu_rows(window.findChild(QMenu, 'settingsMenu'), skip_checked=('GPU 가속 사용',)),
              learn_url=luma.app.LEARN_URL)

# ---- the language dialog ------------------------------------------------------------------------------------------

dialogs = []
for first_run in (True, False):
    for current in ('ko', 'en'):
        dialog = LanguageDialog(current, None, first_run)
        buttons = dialog.findChild(QDialogButtonBox)
        dialogs.append({
            'first_run': first_run, 'current': current,
            'title': dialog.windowTitle(), 'minimum_width': dialog.minimumWidth(),
            'labels': label_texts(dialog),
            'items': [[dialog.languages.itemText(i), dialog.languages.itemData(i)] for i in range(dialog.languages.count())],
            'index': dialog.languages.currentIndex(), 'selected': dialog.selected_language,
            'ok': buttons.button(QDialogButtonBox.StandardButton.Ok).text(),
            'cancel': buttons.button(QDialogButtonBox.StandardButton.Cancel).text(),
        })
with Case('app', 'settings_language_dialog') as case:
    case.json(dialogs=dialogs)

# ---- DesktopSession.change_language -------------------------------------------------------------------------------

steps = []
window.catalog.save_preference('probe', 1)


def language_step(name, action, expect_calls=False):
    calls = []
    session.open_window = lambda geometry=None, photo=None, mode=None, safe=False: calls.append(
        {'geometry': geometry is not None, 'photo': photo, 'mode': mode})
    result = action()
    stored = json.loads((preferences.path).read_text(encoding='utf-8')) if preferences.path.exists() else {}
    steps.append({'name': name, 'result': result, 'language': i18n.language(), 'stored_language': stored.get('language'),
                  'status': window.statusBar().currentMessage(), 'calls': calls})


# The same language: nothing happens.
language_step('same', lambda: session.change_language('ko'))
# A job is running: the window refuses and says so; the preference stays.
window.export_running = True
language_step('busy export', lambda: session.change_language('en'))
window.export_running = False
window.import_busy = True
language_step('busy import', lambda: session.change_language('en'))
window.import_busy = False
window.command_running = True
language_step('busy command', lambda: session.change_language('en'))
window.command_running = False
# A change: the preference is written and the new window gets the geometry, the photo and the screen.
window.statusBar().clearMessage()
language_step('change', lambda: session.change_language('en'))
with Case('app', 'settings_language_change') as case:
    case.json(steps=steps)
i18n.set_language('ko')

# ---- What's new ---------------------------------------------------------------------------------------------------

recorded = {}


def record_dialog(self):
    view = self.findChild(QTextBrowser)
    recorded['title'] = self.windowTitle()
    recorded['labels'] = [[label.objectName(), label.text()] for label in self.findChildren(QLabel)]
    recorded['size'] = [self.width(), self.height()]
    recorded['has_view'] = view is not None
    return 0


original_exec, original_markdown = QDialog.exec, QTextBrowser.setMarkdown
QDialog.exec = record_dialog


def record_markdown(self, text):
    recorded['markdown'] = text
    return original_markdown(self, text)


QTextBrowser.setMarkdown = record_markdown
cases = []
for language in ('ko', 'en'):
    i18n.set_language(language)
    for name, entries, refs in (('this version', None, ()), ('none', [], ()), ('shipped', [], ('AB12', 'CD34'))):
        recorded.clear()
        whats_new.show(None, entries, refs)
        cases.append({'language': language, 'name': name, **{k: v for k, v in recorded.items()},
                      'markdown': recorded.get('markdown'), 'thanks': whats_new.thanks(refs) if refs else None})
QDialog.exec, QTextBrowser.setMarkdown = original_exec, original_markdown
i18n.set_language('ko')
with Case('app', 'settings_whats_new') as case:
    case.json(version=luma.__version__, cases=cases)

# ---- About, help and the learn link --------------------------------------------------------------------------------

boxes = []


def record_box(self):
    boxes.append({'title': self.windowTitle(), 'text': self.text(), 'informative': self.informativeText(),
                  'detailed': self.detailedText(), 'icon': int(self.icon().value) if hasattr(self.icon(), 'value') else int(self.icon()),
                  'buttons': [b.text() for b in self.buttons()]})
    return 0


original_box_exec = QMessageBox.exec
QMessageBox.exec = record_box
about = []
from luma.app import MainWindow  # noqa: E402
parent = QWidget()
for language in ('ko', 'en'):
    i18n.set_language(language)
    boxes.clear()
    MainWindow.show_about(parent)
    about.append({'language': language, 'box': boxes[0]})
opened = []
original_open = QDesktopServices.openUrl
QDesktopServices.openUrl = staticmethod(lambda url: opened.append(url.toString()) or True)
MainWindow.show_help(parent)
help_url = opened[-1]
QDesktopServices.openUrl = original_open
QMessageBox.exec = original_box_exec
i18n.set_language('ko')
with Case('app', 'settings_about') as case:
    case.json(about=about, help_file=help_url.rsplit('/', 1)[-1], help_scheme=help_url.split(':', 1)[0],
              version=luma.__version__)

# ---- UpdateController: what the boxes say -------------------------------------------------------------------------

shown = []
original_info, original_exec_box = QMessageBox.information, QMessageBox.exec


def record_info(parent, title, text, *args, **kwargs):
    shown.append({'kind': 'information', 'title': title, 'text': text})
    return QMessageBox.StandardButton.Ok


def record_offer(self):
    shown.append({'kind': 'offer', 'title': self.windowTitle(), 'text': self.text(), 'informative': self.informativeText(),
                  'detailed': self.detailedText(), 'buttons': [b.text() for b in self.buttons()]})
    return 0


QMessageBox.information = staticmethod(record_info)
QMessageBox.exec = record_offer
RELEASES = {
    'newer': {'version': '9.9.9', 'name': 'Grainy-9.9.9.zip', 'url': 'https://example.invalid/x.zip', 'size': 1234,
              'sha256': 'ab' * 32, 'notes': '새 기능\n- 하나\n- 둘'},
    'same': {'version': luma.__version__, 'name': 'x', 'url': 'https://example.invalid/x.zip', 'size': 1, 'sha256': '00' * 32, 'notes': ''},
    'none': None,
}


def make_controller(name, fetcher, language='ko'):
    i18n.set_language(language)
    main = QMainWindow()
    menu = main.menuBar().addMenu('설정')
    menu.setObjectName('settingsMenu')
    main.update_button = QPushButton()
    folder = fresh('updates-' + name)
    prefs = Preferences(folder / 'settings.json')
    return main, updater.UpdateController(main, prefs, fetcher=fetcher, packaged=False), prefs


def run_check(name, fetcher, manual, language='ko'):
    main, controller, prefs = make_controller(name, fetcher, language)
    shown.clear()
    rows = menu_rows(main.findChild(QMenu, 'settingsMenu'))
    controller.check(manual)
    controller.thread.join()
    controller.checked()
    return {'name': name, 'language': language, 'manual': manual, 'menu': rows, 'shown': list(shown),
            'status': main.statusBar().currentMessage(), 'button_text': main.update_button.text(),
            'button_hidden': main.update_button.isHidden(), 'update_checked_saved': 'update_checked' in prefs.values}


def failing():
    raise OSError('boom')


results = []
for language in ('ko', 'en'):
    results.append(run_check('error manual', failing, True, language))
    results.append(run_check('error auto', failing, False, language))
    results.append(run_check('same manual', lambda: RELEASES['same'], True, language))
    results.append(run_check('none manual', lambda: RELEASES['none'], True, language))
    results.append(run_check('newer manual', lambda: RELEASES['newer'], True, language))
    results.append(run_check('newer auto', lambda: RELEASES['newer'], False, language))
QMessageBox.information, QMessageBox.exec = original_info, original_exec_box
i18n.set_language('ko')
# The automatic-check toggle writes the preference.
main, controller, prefs = make_controller('toggle', lambda: None)
toggles = []
for value in (True, False, True):
    controller.auto_action.setChecked(value)
    toggles.append(json.loads(prefs.path.read_text(encoding='utf-8')).get('update_check'))
with Case('app', 'settings_updates') as case:
    case.json(results=results, toggles=toggles, version=luma.__version__, release=RELEASES['newer'])

# ---- ScreenModes ---------------------------------------------------------------------------------------------------


class Stack:
    def __init__(self):
        self.index = 0

    def currentIndex(self):
        return self.index


class Compare:
    def __init__(self):
        self.on = False
        self.bar = QWidget()
        self.other = type('V', (), {'setBackgroundBrush': lambda s, b: None})()


class Stand(QWidget):
    """The parts ScreenModes.apply shows and hides, each a plain widget."""

    def __init__(self):
        super().__init__()
        self.parts = {}
        for name in ('library_column', 'edit_column', 'header_bar', 'view_bar', 'filmstrip', 'library_toolbar', 'menu', 'status'):
            self.parts[name] = QWidget(self)
        self.library_column, self.edit_column = self.parts['library_column'], self.parts['edit_column']
        self.header_bar, self.view_bar = self.parts['header_bar'], self.parts['view_bar']
        self.filmstrip, self.library_toolbar = self.parts['filmstrip'], self.parts['library_toolbar']
        self.split = QSplitter()
        for part in (self.library_column, QWidget(), self.edit_column):
            self.split.addWidget(part)
        self.stack, self.compare = Stack(), Compare()
        self.view = type('V', (), {'setBackgroundBrush': lambda s, b: None, 'setFocus': lambda s: None})()
        self.catalog = type('C', (), {'preference': lambda s, k, d=None: d})()
        self.full = False
        self.maximized_flag = False

    def menuBar(self):
        return self.parts['menu']

    def statusBar(self):
        return self.parts['status']

    def set_view_background(self, name):
        pass

    def isFullScreen(self):
        return self.full

    def isMaximized(self):
        return self.maximized_flag

    def showFullScreen(self):
        self.full = True

    def showMaximized(self):
        self.full = False

    def showNormal(self):
        self.full = False


def visible(stand):
    names = ('library_column', 'edit_column', 'header_bar', 'view_bar', 'filmstrip', 'library_toolbar', 'menu', 'status')
    result = {name: not stand.parts[name].isHidden() for name in names}
    result['compare_bar'] = not stand.compare.bar.isHidden()
    return result


SEQUENCES = {
    'develop': ['sides', 'sides', 'bars', 'bars', 'lights', 'sides', 'lights', 'lights', 'leave', 'lights', 'bars', 'leave',
                'bars', 'sides', 'bars', 'bars', 'lights', 'leave', 'leave'],
    'library': ['lights', 'sides', 'leave', 'bars', 'lights', 'bars', 'lights', 'leave'],
    'develop-compare': ['bars', 'lights', 'leave', 'bars', 'bars'],
    'full-screen': ['full', 'leave', 'lights', 'full', 'leave', 'leave', 'leave'],
}
runs = []
for name, steps_list in SEQUENCES.items():
    stand = Stand()
    stand.stack.index = 1 if name == 'library' else 0
    stand.compare.on = name == 'develop-compare'
    modes = ScreenModes(stand)
    modes.apply()
    rows = [{'step': 'start', 'visible': visible(stand), 'sides': modes.sides, 'bars': modes.bars, 'lights': modes.lights,
             'full': stand.full, 'result': None}]
    for step in steps_list:
        result = None
        if step == 'sides':
            modes.toggle_sides()
        elif step == 'bars':
            modes.toggle_bars()
        elif step == 'lights':
            modes.toggle_lights()
        elif step == 'full':
            modes.toggle_full_screen()
        elif step == 'leave':
            result = modes.leave()
        rows.append({'step': step, 'visible': visible(stand), 'sides': modes.sides, 'bars': modes.bars,
                     'lights': modes.lights, 'full': stand.full, 'result': result})
    runs.append({'name': name, 'develop': stand.stack.index == 0, 'compare': stand.compare.on, 'rows': rows})
with Case('app', 'settings_screen_modes') as case:
    case.json(runs=runs)

# ---- the preferences this package writes ---------------------------------------------------------------------------

folder = fresh('prefs')
prefs = Preferences(folder / 'settings.json')
trace = []


def snapshot(step):
    trace.append({'step': step, 'file': json.loads(prefs.path.read_text(encoding='utf-8')), 'order': list(prefs.values)})


prefs.save_language('en')
snapshot('language en')
prefs.save('update_check', True)
snapshot('update_check on')
prefs.save('update_check', False)
snapshot('update_check off')
whats_new.remember(prefs, '0.6.8')
snapshot('version_seen')
prefs.save_use_gpu(False)
snapshot('gpu off')
prefs.save_language('ko')
snapshot('language ko')
rejected = False
try:
    prefs.save_language('fr')
except ValueError:
    rejected = True
pending = {
    'first run': [e[0] for e in whats_new.pending(Preferences(folder / 'none.json'), first_run=True)],
    'same version': [e[0] for e in whats_new.pending(prefs)],
    'older seen': [e[0] for e in whats_new.pending(Preferences(folder / 'none.json'))],
}
prefs.values['version_seen'] = '0.6.5'
pending['from 0.6.5'] = [e[0] for e in whats_new.pending(prefs)]
prefs.values['version_seen'] = '9.0.0'
pending['downgrade'] = [e[0] for e in whats_new.pending(prefs)]

# The library preference of the DRT's 고급 설정 button: the real MainWindow.toggle_drt_advanced on the real window.
drt_window, _, _ = build_window('drt')
catalog = drt_window.catalog
drt = {'default': catalog.preference('drt_advanced', False)}
for value in (True, False, True):
    drt_window.toggle_drt_advanced(value)
    row = catalog.db.execute("SELECT value FROM preferences WHERE key='drt_advanced'").fetchone()
    drt[f'after {value}'] = {'stored': row['value'], 'read': catalog.preference('drt_advanced', False)}
with Case('app', 'settings_preferences') as case:
    case.json(trace=trace, rejected=rejected, pending=pending, drt=drt)

shutil.rmtree(WORK, ignore_errors=True)
print('settings goldens written')
