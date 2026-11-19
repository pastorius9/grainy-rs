"""Goldens of work package 17 (compare and reference views), from the oracle's own code.

* `compare_before_settings`: luma/compare_view.py `before_settings` of several edited photos (what the "before"
  side of the compare view renders), stored as the keys that differ from `defaults()`.
* `compare_session` and `compare_fallback`: the real `CompareLayout` (its `start`, `stop`, `toggle`, `set_mode`,
  `set_vertical`, `swap`, `pick`, `activate_other`, `sync`, `arrange`, `show`, `sharpen`) driven on a stand-in of
  `MainWindow` (a widget with a real `PhotoView`, a catalogue of five photos, a selection, `activate` with the
  `previous_id` of luma/app.py). After every step the timer's `sync` runs once and the state is recorded: mode,
  order, direction, captions, the white frame, the photo beside the current one, and the record and size the
  second picture is rendered from (the `spawn` call, recorded instead of run).

Run with the oracle's interpreter from the repository root:
    "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_compare.py
"""
from copy import deepcopy

from common import Case

from PySide6.QtCore import QObject, Signal  # noqa: E402
from PySide6.QtWidgets import QApplication, QPushButton, QWidget  # noqa: E402

from luma.compare_view import CompareLayout, before_settings  # noqa: E402
from luma.engine import defaults, normalized  # noqa: E402

_qt = QApplication.instance() or QApplication([])
DEFAULTS = defaults()


def changed(settings):
    """The keys whose value differs from the defaults."""
    return {k: v for k, v in settings.items() if k not in DEFAULTS or v != DEFAULTS[k]}


# ---- before_settings ---------------------------------------------------------------------------------------------

BEFORE_CASES = [
    {},
    {'exposure': 1.2, 'contrast': 30, 'temperature': 12, 'film_look': 'Portra 400', 'grain': 20},
    {'crop': [0.1, 0.05, 0.8, 0.9], 'rotation': 1, 'straighten': -2.5, 'flip': True, 'exposure': -0.5},
    {'distortion': 12, 'lens_vignette': 20, 'ca_red': 1.5, 'defringe': 30, 'perspective_v': -18, 'perspective_h': 7,
     'aspect_scale': 4, 'shift_x': 3, 'transform_scale': 95, 'upright': 'auto', 'upright_crop': True,
     'saturation': 40, 'masks': [{'type': 'linear', 'name': 'm', 'start': [0.2, 0.2], 'end': [0.8, 0.8],
                                  'enabled': True, 'invert': False, 'adjustments': {'exposure': 1}}]},
    {'working_space': 'ProPhoto', 'lensfun_enabled': True, 'lensfun_distortion': False, 'guides': [[0.1, 0.2, 0.3, 0.4]],
     'retouch': [{'x': 0.5, 'y': 0.5, 'radius': 0.02}], 'curve': [[0, 0], [0.5, 0.6], [1, 1]], 'monochrome': True},
]

with Case('app', 'compare_before_settings') as case:
    cases = []
    for changes in BEFORE_CASES:
        settings = normalized({**DEFAULTS, **deepcopy(changes)})
        result = before_settings(settings)
        assert set(result) == set(DEFAULTS)
        cases.append({'changes': changed(settings), 'before': changed(result)})
    case.json(cases=cases)


# ---- CompareLayout on a stand-in window --------------------------------------------------------------------------

class Model:
    def __init__(self, ids):
        self.positions = {ident: row for row, ident in enumerate(ids)}

    def index(self, row):
        return row


class Selection:
    def __init__(self, w):
        self.w = w

    def setCurrentIndex(self, row, flags):
        self.w.selection = [self.w.ids[row]]


class Catalog:
    def __init__(self, photos):
        self.photos = photos

    def photo(self, ident):
        return deepcopy(self.photos[ident])


class Colour(QObject):
    changed = Signal()


class Display:
    def __init__(self):
        self.main = Colour()

    def refresh_view(self, view, target):
        pass


class Status:
    def __init__(self):
        self.message = ''

    def showMessage(self, text, timeout=0):
        self.message = text


class Modes:
    def apply(self):
        pass


class Flow:
    previous_id = None


class Window(QWidget):
    """The parts of `MainWindow` that `CompareLayout` uses."""

    def __init__(self, photos, current):
        super().__init__()
        from luma.widgets import PhotoView
        self.view = PhotoView()
        self.ids = list(photos)
        self.photo_model = Model(self.ids)
        self.visible_ids = list(self.ids)
        self.photo_selection = Selection(self)
        self.catalog = Catalog(photos)
        self.display_color = Display()
        self.status = Status()
        self.screen_modes = Modes()
        self.workflow = Flow()
        self.compare_button = QPushButton()
        self.compare_button.setCheckable(True)
        self.spawned = []
        self.screen = 0
        self.current_id = current
        self.settings = deepcopy(photos[current]['settings'])
        self.selection = [current]

    def selected_ids(self):
        return list(self.selection)

    def statusBar(self):
        return self.status

    def set_mode(self, mode):
        self.screen = mode

    def commit(self):
        self.catalog.photos[self.current_id]['settings'] = deepcopy(self.settings)

    def activate(self, ident):
        """`MainWindow.activate` up to its background load: commit, `previous_id`, the stored settings."""
        if ident == self.current_id:
            return
        self.commit()
        if self.current_id is not None and self.current_id != ident:
            self.workflow.previous_id = self.current_id
        self.current_id = ident
        self.settings = deepcopy(self.catalog.photos[ident]['settings'])

    def spawn(self, work, loaded, failed):
        # The work's closure holds the record and the size `CompareLayout.show` renders.
        cells = dict(zip(work.__code__.co_freevars, (c.cell_contents for c in work.__closure__)))
        record, size = cells['record'], cells['size']
        self.spawned.append({'id': record['id'], 'path': record['path'], 'name': record['name'],
                             'settings': changed(normalized(record['settings'])), 'size': size})


def photo(ident, **changes):
    return {'id': ident, 'path': f'Z:/compare/photo-{ident}.png', 'name': f'photo-{ident}.png',
            'settings': normalized({**DEFAULTS, **changes})}


def state(w, layout):
    first = layout.order.itemAt(0).widget()
    main, other = layout.main_pane, layout.other_pane
    return {
        'on': layout.on, 'mode': layout.mode, 'swapped': layout.swapped, 'vertical': layout.vertical,
        'other_id': layout.other_id, 'current_id': w.current_id, 'previous_id': w.workflow.previous_id,
        'selection': list(w.selection), 'main_first': first is main, 'checked': w.compare_button.isChecked(),
        'main_caption': main.caption.text() if layout.on else '', 'other_caption': other.caption.text() if layout.on else '',
        'main_marked': main.marked if layout.on else False, 'full': layout.full,
        'renders': len(w.spawned), 'shown': w.spawned[-1] if w.spawned else None, 'status': w.status.message,
        'screen': w.screen,
    }


def run(w, layout, steps):
    records = []
    for step in steps:
        name, args = step[0], step[1:]
        if name == 'toggle':
            layout.toggle()
        elif name == 'start':
            layout.start(args[0])
        elif name == 'stop':
            layout.stop(show_other=args[0])
        elif name == 'mode':
            layout.set_mode(args[0])
        elif name == 'vertical':
            layout.set_vertical(args[0])
        elif name == 'swap':
            layout.swap()
        elif name == 'activate_other':
            layout.activate_other()
        elif name == 'set':
            w.settings[args[0]] = args[1]
        elif name == 'select':
            w.selection = list(args[0])
        elif name == 'activate':
            w.selection = [args[0]]
            w.activate(args[0])
        elif name == 'zoom':
            w.view.actual_size()
        else:
            raise ValueError(name)
        layout.sync()          # the 400 ms timer
        records.append({'step': list(step), 'state': state(w, layout)})
    return records


PHOTOS = [
    photo(1, exposure=1.0, crop=[0.1, 0.1, 0.7, 0.8], rotation=1, distortion=10),
    photo(2, contrast=20, flip=True),
    photo(3),
    photo(4, saturation=-30, perspective_v=12),
    photo(5, straighten=4.0, temperature=-10),
]

SESSION = [
    ('toggle',),
    ('set', 'exposure', 2.0),
    ('set', 'straighten', 3.0),
    ('swap',),
    ('vertical', True),
    ('mode', 'photos'),
    ('activate_other',),
    ('swap',),
    ('vertical', False),
    ('toggle',),
    ('select', [2, 4]),
    ('toggle',),
    ('mode', 'before'),
    ('mode', 'photos'),
    ('zoom',),
    ('stop', True),
    ('activate', 5),
    ('start', 'photos'),
    ('mode', 'photos'),
    ('start', None),
    ('stop', False),
    ('start', 'before'),
    ('activate', 3),
    ('activate_other',),
    ('start', 'photos'),
    ('stop', False),
]

with Case('app', 'compare_session') as case:
    photos = {p['id']: p for p in deepcopy(PHOTOS)}
    w = Window(photos, 1)
    layout = CompareLayout(w)
    case.json(photos=[{**p, 'settings': changed(p['settings'])} for p in PHOTOS], current=1,
              steps=run(w, layout, SESSION))

with Case('app', 'compare_fallback') as case:
    # One photo only: the photos mode finds no other photo and falls back to before/after.
    photos = {p['id']: p for p in deepcopy(PHOTOS[:1])}
    w = Window(photos, 1)
    layout = CompareLayout(w)
    case.json(photos=[{**p, 'settings': changed(p['settings'])} for p in PHOTOS[:1]], current=1,
              steps=run(w, layout, [('start', 'photos'), ('mode', 'photos'), ('toggle',), ('toggle',)]))
