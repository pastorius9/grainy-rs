"""Goldens of the display colour package of grainy-app (APP_PLAN package 23): `golden/app/display*/`.

Made with the oracle's own code (nothing is reimplemented):

* `display_state`: luma/display_color.py `DisplayColor`, `DisplayTarget` and `DisplayDialog` on a stand-in main window
  (a real `QMainWindow` with a real `Catalog`; `spawn` runs the job at once): the button text and tooltip, the resolved
  state (mode, screen, path, description, message, error, digest), `revision`, the thumbnails invalidated and the
  renders asked for, and the preferences `display_color` / `monitor_icc` after each step: first resolve, a profile
  appearing, no change, a manual profile, errors (no file, not a profile), off, auto again, an unsupported mode; the
  dialog's items, enabled fields and status text; the legacy `monitor_icc` of an older library.
* `display_texts_ko`, `display_texts_en`: every text of the box and the button in both languages.
* `display_background`: luma/app.py `MainWindow.set_view_background` and `_build_bg_context_menu` unbound on a stand-in
  window: the brushes of the photo view and the compare view, the preference writes and the checked items.
* `display_inputs`: the small photo (`photo.png`) and the monitor profiles (built-in ones with the creation date
  cleared, `Display_P3.icc`, `Adobe_RGB.icc`, `sRGB.icc`) the next cases use.
* `display_frame_*`: the frame the oracle shows with a monitor profile (`develop(output_space=None)` then
  `output_rgb(working, space, profile)`, quantised as `qimage` does) for that photo and the settings in `case.json`.
* `display_thumbnail`: the conversion of an 8-bit thumbnail in `PhotoModel.thumbnail` (`convert(pixels / 255, sRGB,
  monitor)`, quantised).

Every temporary folder is made here and left out of the output (paths are replaced by `<dir>`; no clocks or process
ids are stored).

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_display.py
"""
import json
import shutil
import struct
import tempfile
from pathlib import Path

from common import Case, inputs, np

from PySide6.QtGui import QAction, QColor  # noqa: E402
from PySide6.QtWidgets import (QApplication, QComboBox, QDialogButtonBox, QLabel, QLineEdit, QMainWindow,  # noqa: E402
                               QPushButton, QWidget)

from luma import colorio, display_profiles, i18n  # noqa: E402
from luma import display_color as display_module  # noqa: E402
from luma.app import BACKGROUNDS, MainWindow  # noqa: E402
from luma.catalog import Catalog  # noqa: E402
from luma.engine import defaults, develop, load_image, normalized, output_rgb, to_srgb  # noqa: E402

import gc  # noqa: E402

gc.disable()   # the stand-in windows stay alive: collecting a half-dead DisplayTarget would run its event filter
_qt = QApplication.instance() or QApplication([])
KEEP = []
WORK = Path(tempfile.mkdtemp(prefix='grainy-display-golden-'))


_made = []


def fresh(name):
    """A new empty folder (never reused: an earlier catalog may still hold its files open)."""
    folder = WORK / f'{len(_made):02d}-{name}'
    _made.append(folder)
    folder.mkdir(parents=True)
    return folder


def clean(text, folder):
    """Paths of the temporary folder are written as <dir> (and with forward slashes)."""
    if not isinstance(text, str):
        return text
    return text.replace(str(folder), '<dir>').replace(str(folder).replace('\\', '/'), '<dir>')


def fixed_profile(builtin):
    """A built-in profile with its creation date and profile ID cleared: LittleCMS stamps the time it made it, which
    would change the stored bytes (and their digest) in every run."""
    data = bytearray(colorio.profile(builtin))
    data[24:36] = struct.pack('>6H', 2026, 1, 1, 0, 0, 0)
    data[84:100] = bytes(16)
    return bytes(data)


def profile_file(folder, name, builtin):
    path = folder / name
    path.write_bytes(fixed_profile(builtin))
    return path


# ---- the stand-in main window -----------------------------------------------------------------------------------

class Counter:
    def __init__(self):
        self.invalidated = 0

    def invalidate_thumbnails(self, ids=None, retry=True):
        self.invalidated += 1


class Window(QMainWindow):
    """What DisplayColor needs of the main window: a catalog, `spawn` (run at once), the pool, the thumbnail model and the
    render entry."""

    def __init__(self, catalog):
        super().__init__()
        self.catalog = catalog
        self.display_pool = None
        self.photo_model = Counter()
        self.render_version = 0
        self.renders = 0

    def render(self):
        self.renders += 1

    def spawn(self, function, callback, error=None, pool=None):
        try:
            result = function()
        except Exception as failure:
            if error:
                error(str(failure))
            return
        callback(result)


def build(name, language='ko', preferences=None):
    i18n.set_language(language)
    folder = fresh(name)
    catalog = Catalog(folder / 'Library')
    for key, value in (preferences or {}).items():
        catalog.save_preference(key, value)
    window = Window(catalog)
    window.show()
    manager = display_module.DisplayColor(window)
    KEEP.append((window, manager, catalog))
    return folder, catalog, window, manager


def snapshot(label, folder, catalog, window, manager):
    s = manager.main.state
    stored = {key: catalog.preference(key, '__unset__') for key in ('display_color', 'monitor_icc')}
    return {
        'step': label,
        'button': manager.button.text(),
        'tooltip': clean(manager.button.toolTip(), folder),
        'mode': manager.mode,
        'path': clean(manager.path, folder),
        'state': {'mode': s.mode, 'screen': s.screen, 'path': clean(s.path, folder), 'description': s.description,
                  'message': clean(s.message, folder), 'error': s.error, 'has_profile': s.data is not None, 'digest': s.digest},
        'revision': manager.main.revision,
        'thumbnails_invalidated': window.photo_model.invalidated,
        'renders': window.renders,
        'render_version': window.render_version,
        'stored': {key: (clean(value, folder) if isinstance(value, str) else
                         {k: clean(v, folder) for k, v in value.items()} if isinstance(value, dict) else value)
                   for key, value in stored.items()},
    }


def dialog_snapshot(dialog, folder):
    return {
        'mode': dialog.mode.currentData(),
        'path': clean(dialog.path.text(), folder),
        'path_enabled': dialog.path.isEnabled(),
        'choose_enabled': dialog.choose.isEnabled(),
        'status': clean(dialog.status.text(), folder),
    }


# ---- display_state --------------------------------------------------------------------------------------------------

def state_case():
    folder, catalog, window, manager = build('state')
    p3 = profile_file(folder, 'p3.icc', 'Display P3')
    adobe = profile_file(folder, 'adobe.icc', 'Adobe RGB')
    text = folder / 'notes.icc'
    text.write_text('this is not an ICC profile', encoding='ascii')
    steps = [snapshot('initial', folder, catalog, window, manager)]
    dialog_steps = []

    def step(label):
        steps.append(snapshot(label, folder, catalog, window, manager))

    manager.main.refresh()
    step('first resolve: Windows default')
    real = display_profiles.windows_profile_path
    display_profiles.windows_profile_path = lambda screen: str(p3)
    try:
        manager.main.schedule()
        manager.main.refresh()
        step('auto with a profile from the system')
        manager.main.schedule()
        manager.main.refresh()
        step('the same again')
    finally:
        display_profiles.windows_profile_path = real
    manager.configure('manual', str(adobe))
    manager.main.refresh()
    step('manual profile')
    manager.configure('manual', '')
    manager.main.refresh()
    step('manual without a file')
    manager.configure('manual', str(text))
    manager.main.refresh()
    step('manual with a file that is not a profile')
    manager.configure('off')
    manager.main.refresh()
    step('off')
    manager.configure('auto')
    manager.main.refresh()
    step('auto, Windows default again')
    try:
        manager.configure('sepia')
        refusal = None
    except ValueError as error:
        refusal = str(error)
    step('after an unsupported mode')

    # The dialog (a fresh state: manual with the Adobe profile).
    manager.configure('manual', str(adobe))
    manager.main.refresh()
    dialog = display_module.DisplayDialog(manager)
    dialog_steps.append({'step': 'opened on a manual choice', **dialog_snapshot(dialog, folder)})
    dialog.mode.setCurrentIndex(dialog.mode.findData('auto'))
    dialog_steps.append({'step': 'mode set to auto', **dialog_snapshot(dialog, folder)})
    dialog.mode.setCurrentIndex(dialog.mode.findData('manual'))
    dialog.path.setText('')
    dialog.apply()
    dialog_steps.append({'step': 'apply manual without a file', **dialog_snapshot(dialog, folder)})
    dialog.path.setText(f'  {p3}  ')
    dialog.apply()
    dialog_steps.append({'step': 'apply manual with a file (spaces stripped)', **dialog_snapshot(dialog, folder)})
    manager.main.refresh()
    dialog_steps.append({'step': 'resolved', **dialog_snapshot(dialog, folder)})
    manager.configure('off')
    dialog_steps.append({'step': 'a menu item turned it off', **dialog_snapshot(dialog, folder)})
    manager.main.refresh()
    dialog_steps.append({'step': 'resolved off', **dialog_snapshot(dialog, folder)})
    final = snapshot('after the dialog', folder, catalog, window, manager)

    # A library of an older Grainy: only `monitor_icc`.
    legacy = []
    for label, preferences in [('unset', {}), ('a path', {'monitor_icc': str(adobe)}), ('empty', {'monitor_icc': ''}),
                               ('null', {'monitor_icc': None}), ('a broken mode', {'display_color': {'mode': 'x', 'path': 'p'}}),
                               ('the new choice wins', {'display_color': {'mode': 'off', 'path': ''}, 'monitor_icc': str(adobe)})]:
        sub_folder, _, _, sub = build(f'legacy{len(legacy)}', preferences=preferences)
        legacy.append({'stored': label, 'mode': sub.mode, 'path': Path(sub.path).name})
        sub.main.shutdown()
    with Case('app', 'display_state') as case:
        case.json(steps=steps, dialog=dialog_steps, final=final, legacy=legacy, refusal=refusal)
    manager.main.shutdown()


# ---- the texts in both languages ----------------------------------------------------------------------------------

def texts_case(language):
    folder, catalog, window, manager = build('texts', language)
    manager.show_dialog()
    dialog = manager.dialog
    labels = [label.text() for label in dialog.findChildren(QLabel)]
    buttons = [b.text() for b in dialog.findChildren(QPushButton)]
    box = dialog.findChild(QDialogButtonBox)
    result = {
        'button_initial': manager.button.text(),
        'title': dialog.windowTitle(),
        'labels': labels,
        'buttons': buttons,
        'box_buttons': [b.text() for b in box.buttons()],
        'items': [dialog.mode.itemText(i) for i in range(dialog.mode.count())],
        'placeholder': dialog.path.placeholderText(),
        'size': [dialog.width(), dialog.height()],
        'background_menu': [],
    }
    for mode, path, name in [('auto', '', 'auto'), ('off', '', 'off')]:
        manager.configure(mode, path)
        manager.main.refresh()
        result[f'button_{name}'] = manager.button.text()
    manager.configure('manual', '')
    manager.main.refresh()
    result['button_error'] = manager.button.text()
    result['status_error'] = dialog.status.text()
    dialog.mode.setCurrentIndex(dialog.mode.findData('manual'))
    dialog.path.setText('')
    dialog.apply()
    result['status_no_file'] = dialog.status.text()
    dialog.path.setText('x.icc')
    dialog.apply()
    result['status_applying'] = dialog.status.text()
    manager.main.shutdown()
    with Case('app', f'display_texts_{language}') as case:
        case.json(**result)


# ---- the background -------------------------------------------------------------------------------------------------

class Brush:
    def __init__(self):
        self.colours = []

    def setBackgroundBrush(self, brush):
        self.colours.append(brush.color().name())


class FakeCatalog:
    def __init__(self, stored=None):
        self.values = dict(stored or {})
        self.saved = []

    def preference(self, key, default=None):
        return self.values.get(key, default)

    def save_preference(self, key, value):
        self.values[key] = value
        self.saved.append([key, value])


class Holder(QWidget):
    """A widget carrying what the unbound MainWindow methods use."""


def background_case():
    i18n.set_language('ko')
    steps = []
    window = Holder()
    window.view = Brush()
    window.compare = Holder()
    window.compare.other = Brush()
    window.catalog = FakeCatalog()
    window.manager = Holder()
    window.manager.bg_group = []
    for key in BACKGROUNDS:
        action = QAction(key, window)
        action.setCheckable(True)
        window.manager.bg_group.append((key, action))
    menu = MainWindow._build_bg_context_menu(window)

    def record(label):
        steps.append({'step': label,
                      'view': list(window.view.colours), 'compare': list(window.compare.other.colours),
                      'saved': [list(item) for item in window.catalog.saved],
                      'menu_checked': [a.isChecked() for k, a in window.manager.bg_group],
                      'context_checked': [a.isChecked() for k, a in window._bg_context_actions]})

    record('built, nothing chosen')
    for label, key in [('white', 'white'), ('white again', 'white'), ('mid grey', 'mid_grey'), ('unknown key', 'bogus'),
                       ('black', 'black'), ('dark grey', 'dark_grey')]:
        MainWindow.set_view_background(window, key)
        record(label)
    texts = {}
    for language in ('ko', 'en'):
        i18n.set_language(language)
        window.catalog = FakeCatalog({'view_background': 'dark_grey'})
        built = MainWindow._build_bg_context_menu(window)
        texts[language] = [{'text': a.text(), 'checked': a.isChecked(), 'checkable': a.isCheckable()} for a in built.actions()]
    i18n.set_language('ko')
    with Case('app', 'display_background') as case:
        case.json(steps=steps, colours={key: QColor(value).name() for key, value in BACKGROUNDS.items()},
                  default=QColor('#101215').name(), menus=texts, keys=list(BACKGROUNDS))


# ---- frames and thumbnails ------------------------------------------------------------------------------------------

def photo(folder):
    """A small photo: the garden, 8-bit sRGB (80 x 120), with a clipped and a saturated patch."""
    from PIL import Image
    a = np.clip(to_srgb(inputs()['garden'][::2, ::2]), 0, 1)
    u = np.round(a * 255).astype(np.uint8)
    u[8:20, 8:24] = (255, 255, 255)
    u[44:56, 8:24] = (0, 0, 0)
    u[30:50, 90:110] = (230, 20, 40)
    path = folder / 'photo.png'
    Image.fromarray(u, 'RGB').save(path)
    return path


def quantise(a):
    """`widgets.qimage`: `np.uint8(np.clip(a, 0, 1) * 255 + .5)` in float32."""
    part = np.clip(a, 0, 1)
    part *= 255
    part += .5
    return part.astype(np.uint8)


FRAMES = [
    ('p3_exposure', 'Display P3', dict(exposure=.4, contrast=12, saturation=15)),
    ('adobe_prophoto', 'Adobe RGB', dict(working_space='ProPhoto', exposure=-.2, vibrance=30, temperature=8)),
    ('srgb_profile', 'sRGB', dict(shadows=25, highlights=-20)),
]


def frame_cases():
    folder = fresh('inputs')
    path = photo(folder)
    with Case('app', 'display_inputs') as case:
        shutil.copyfile(path, case.folder / 'photo.png')
        for name in ('Display P3', 'Adobe RGB', 'sRGB'):
            (case.folder / f'{name.replace(" ", "_")}.icc').write_bytes(fixed_profile(name))
        case.json(note='the photo and the monitor profiles of the display_frame_* and display_thumbnail_* cases')
    for name, builtin, changes in FRAMES:
        settings = normalized({**defaults(), **changes})
        monitor = fixed_profile(builtin)
        source, meta = load_image(path, 2048, settings['working_space'], settings)
        size = (meta.get('width', source.shape[1]), meta.get('height', source.shape[0]))
        working = develop(source, settings, True, output_space=None, original_size=size)
        display = output_rgb(working, settings['working_space'], monitor)
        with Case('app', f'display_frame_{name}') as case:
            case.json(settings=changes, profile=builtin)
            case.array('display', quantise(display))


def thumbnail_case():
    rng = np.random.default_rng(2611)
    h, w = 24, 36
    yy, xx = np.mgrid[:h, :w]
    pixels = np.stack([xx * 255 // (w - 1), yy * 255 // (h - 1), 255 - xx * yy * 255 // ((w - 1) * (h - 1))], -1).astype(np.uint8)
    pixels = np.clip(pixels.astype(np.int16) + rng.integers(-12, 13, pixels.shape), 0, 255).astype(np.uint8)
    pixels[:4, :4] = 255
    pixels[-4:, -4:] = 0
    for name, builtin in [('p3', 'Display P3'), ('adobe', 'Adobe RGB')]:
        converted = colorio.convert(pixels.astype(np.float32) / 255, colorio.profile('sRGB'), fixed_profile(builtin))
        with Case('app', f'display_thumbnail_{name}') as case:
            case.json(profile=builtin)
            case.array('input', pixels)
            case.array('output', quantise(converted))


if __name__ == '__main__':
    state_case()
    texts_case('ko')
    texts_case('en')
    background_case()
    frame_cases()
    thumbnail_case()
    print('display goldens written', flush=True)
    # The stand-in windows are still open: leave without tearing Qt down (its event filters would run on half-dead objects).
    import os
    os._exit(0)
