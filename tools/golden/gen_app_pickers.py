"""Goldens of work package 4 of grainy-app: the white-balance eyedropper and the film-base picker.

Everything is made with the oracle's own code:

* `golden/app/pickers_gesture`: luma/widgets.py `PhotoView` itself (offscreen Qt, a real view of a real picture):
  its mouse and key handlers in the eyedropper mode (`sample_mode`, `sample_press`, `sample_rect`, the `sampled`,
  `sampledArea` and `sampleCancelled` signals), driven by scripted presses, moves, releases and Esc.
* `golden/app/pickers_display`, `pickers_raw_*`, `pickers_negative_*`: luma/app.py `MainWindow.toggle_wb`,
  `toggle_negative_base`, `sample_wb`, `sample_wb_area`, `sample_negative_base` (and `set_setting`) run unbound on a
  stand-in window, with luma/studio.py's own `source_point`, `source_points` and `area_pixels` as its studio:
  clicks and rectangles on a display picture (ordinary photos), on a camera source of a DNG (the RAW white balance:
  `eyedropper.mean_colour`, `rawcolor.neutral_white`) and on film scans (`negative.base_from`). Each step records
  the settings it changed, the status texts, the commits (undo labels) and the state of the picker buttons.

The DNG files are the ones `gen_rawload.py` stores in `golden/rawload/_files`. Nothing is written but `golden/app`.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_pickers.py
"""
import sys
from copy import deepcopy

from common import GOLDEN, Case, inputs, np

from PySide6.QtCore import QEvent, QPoint, QPointF, Qt  # noqa: E402
from PySide6.QtGui import QKeyEvent, QMouseEvent  # noqa: E402
from PySide6.QtWidgets import QApplication  # noqa: E402

from luma import engine, eyedropper, negative, rawcolor  # noqa: E402
from luma.app import MainWindow  # noqa: E402
from luma.engine import defaults, normalized, to_srgb  # noqa: E402
from luma.panel_keys import switch_on  # noqa: E402
from luma.studio import StudioTools  # noqa: E402
from luma.widgets import PhotoView  # noqa: E402

_qt = QApplication.instance() or QApplication([])
FILES = GOLDEN / 'rawload' / '_files'
LIVE = 720                      # the long side of the oracle's live source (`resize_float(source, 720)`)


class NoOp:
    """Anything the stand-in does not need: every attribute is a callable that returns itself."""

    def __getattr__(self, name):
        return self

    def __call__(self, *args, **kwargs):
        return self


# ---- the pointer: PhotoView in the eyedropper mode -------------------------------------------------------------

def picture():
    """The display picture of the cases: the garden (every second pixel, 80 x 120), 8-bit like the Rust frame, with a
    clipped patch, a black patch and a neutral grey patch."""
    a = np.clip(to_srgb(inputs()['garden'][::2, ::2]), 0, 1)
    u = np.round(a * 255).astype(np.uint8)
    u[8:20, 8:24] = 255                       # clipped
    u[44:56, 8:24] = 0                        # black
    u[40:70, 80:110] = (128, 128, 128)        # neutral grey
    u[10:16, 60:64] = (255, 255, 255)         # a thin clipped strip beside colour
    return np.ascontiguousarray(u)


GESTURES = [   # (name, steps): ['press'|'move'|'release'|'key', x, y]
    ('click', [['press', 300, 200], ['release', 300, 200]]),
    ('click_with_a_small_move', [['press', 300, 200], ['move', 302, 202], ['release', 303, 202]]),
    ('drag_just_far_enough', [['press', 300, 200], ['move', 303, 200], ['move', 304, 201], ['release', 304, 201]]),
    ('drag_a_rectangle', [['press', 250, 150], ['move', 300, 200], ['move', 420, 260], ['release', 420, 260]]),
    ('drag_up_and_left', [['press', 420, 260], ['move', 380, 230], ['release', 260, 160]]),
    ('drag_off_the_photo', [['press', 500, 300], ['move', 900, 520], ['release', 900, 520]]),
    ('drag_off_the_top_left', [['press', 200, 120], ['move', 5, 5], ['release', 5, 5]]),
    ('press_off_the_photo', [['press', 4, 4], ['move', 40, 40], ['release', 40, 40],
                             ['press', 300, 200], ['release', 300, 200]]),
    ('escape', [['press', 300, 200], ['key', 0, 0], ['release', 300, 200], ['press', 300, 200], ['release', 300, 200]]),
    ('escape_mid_drag', [['press', 250, 150], ['move', 330, 220], ['key', 0, 0], ['move', 400, 250],
                         ['release', 400, 250]]),
]


def send(view, step):
    kind, x, y = step
    if kind == 'key':
        view.keyPressEvent(QKeyEvent(QEvent.Type.KeyPress, Qt.Key.Key_Escape, Qt.KeyboardModifier.NoModifier))
        return
    event_type = {'press': QEvent.Type.MouseButtonPress, 'move': QEvent.Type.MouseMove,
                  'release': QEvent.Type.MouseButtonRelease}[kind]
    button = Qt.MouseButton.NoButton if kind == 'move' else Qt.MouseButton.LeftButton
    buttons = Qt.MouseButton.NoButton if kind == 'release' else Qt.MouseButton.LeftButton
    point = QPointF(x, y)
    event = QMouseEvent(event_type, point, point, button, buttons, Qt.KeyboardModifier.NoModifier)
    {'press': view.mousePressEvent, 'move': view.mouseMoveEvent, 'release': view.mouseReleaseEvent}[kind](event)


def rect_fractions(rect):
    return None if rect is None else [rect.left() / view_width(), rect.top() / view_height(),
                                      rect.right() / view_width(), rect.bottom() / view_height()]


VIEW = None


def view_width():
    return VIEW.image_rect.width()


def view_height():
    return VIEW.image_rect.height()


def gesture_case():
    global VIEW
    shown = picture().astype(np.float32) / 255
    scripts = []
    for name, steps in GESTURES:
        view = VIEW = PhotoView()
        view.resize(800, 500)
        view.set_image(shown)
        view.fit_photo()
        view.show()
        _qt.processEvents()
        emitted = []
        view.sampled.connect(lambda x, y, emitted=emitted: emitted.append(['click', x, y]))
        view.sampledArea.connect(lambda a, b, c, d, emitted=emitted: emitted.append(['area', a, b, c, d]))
        view.sampleCancelled.connect(lambda emitted=emitted: emitted.append(['cancelled']))
        view.sample_mode = True
        # The scene is the shown frame in logical pixels: screen = (scene - origin) / step.
        origin = view.mapToScene(QPoint(0, 0))
        step = view.mapToScene(QPoint(1, 0)).x() - origin.x()
        width, height = view.image_rect.width(), view.image_rect.height()
        results = []
        for item in steps:
            before = len(emitted)
            send(view, item)
            results.append(dict(emitted=emitted[before:], area=rect_fractions(view.sample_area),
                                pressed=view.sample_press is not None, sample_mode=view.sample_mode))
        scripts.append(dict(name=name, steps=steps, results=results))
        left, top = -origin.x() / step, -origin.y() / step
        rect = [left, top, left + width / step, top + height / step]
        view.close()
        view.deleteLater()
    return rect, scripts


with Case('app', 'pickers_gesture') as case:
    rect, scripts = gesture_case()
    case.json(photo_rect=rect, drag=PhotoView.SAMPLE_DRAG, scripts=scripts)


# ---- the window's picker methods -----------------------------------------------------------------------------

SUBSET = ['temperature', 'tint', 'raw_neutral', 'raw_mode', 'raw_kelvin', 'raw_tint', 'kelvin_enabled',
          'negative_base', 'negative', 'negative_peak']


class Button:
    def __init__(self):
        self.checked = False

    def isChecked(self):
        return self.checked

    def setChecked(self, value):
        self.checked = bool(value)

    def __getattr__(self, name):
        return NoOp()


class Bar:
    def __init__(self):
        self.messages = []

    def showMessage(self, text, timeout=0):
        self.messages.append(text)


class View(NoOp):
    def __init__(self, on_screen):
        self.on_screen = on_screen
        self.crop_mode = False
        self.sample_mode = False


class Studio(NoOp):
    """luma/studio.py's own `source_point`, `source_points`, `area_pixels` on the window's live source."""
    source_point = StudioTools.source_point
    source_points = StudioTools.source_points
    area_pixels = StudioTools.area_pixels

    def __init__(self, window):
        self.w = window
        self.coordinate_key = None
        self.coordinates = None


class PickWindow(NoOp):
    toggle_wb = MainWindow.toggle_wb
    toggle_negative_base = MainWindow.toggle_negative_base
    sample_wb = MainWindow.sample_wb
    sample_wb_area = MainWindow.sample_wb_area
    sample_negative_base = MainWindow.sample_negative_base
    set_setting = MainWindow.set_setting

    def __init__(self, source, settings, on_screen=None):
        self.source = source
        self.live_source = engine.resize_float(source, LIVE)
        self.load_version = 1
        self.settings = settings
        self.last_saved = deepcopy(settings)
        self.view = View(on_screen)
        self.studio = Studio(self)
        self.bar = Bar()
        self.wb_button, self.negative_base_button = Button(), Button()
        self.commits = []
        self.render_version = 0

    def statusBar(self):
        return self.bar

    def commit(self, label='보정 변경'):
        if self.settings != self.last_saved:
            self.commits.append(dict(label=label, values={k: deepcopy(self.settings[k]) for k in SUBSET}))
            self.last_saved = deepcopy(self.settings)

    def snapshot(self):
        return {k: deepcopy(self.settings[k]) for k in SUBSET}


STORED = {}


def keep(case, name, pixels):
    """Stores `pixels` in `case` unless an earlier case holds the same array; returns the case that has them."""
    for owner, other in STORED.items():
        if other.shape == pixels.shape and np.array_equal(other, pixels):
            return owner
    STORED[name] = pixels
    case.array('pixels', pixels)
    return name


def armed(steps, toggle):
    """The steps with the picker button pressed before each sample, as the user has to (a sample switches it off)."""
    result = []
    for step in steps:
        if step[0] in ('click', 'area'):
            result.append([toggle, True])
        result.append(step)
    return result


def run_steps(window, steps):
    results = []
    for step in steps:
        kind = step[0]
        messages, commits = len(window.bar.messages), len(window.commits)
        if kind == 'toggle_wb':
            window.wb_button.setChecked(step[1])
            window.toggle_wb(step[1])
        elif kind == 'toggle_base':
            window.negative_base_button.setChecked(step[1])
            window.toggle_negative_base(step[1])
        elif kind == 'click':
            window.sample_wb(step[1], step[2])
        elif kind == 'area':
            window.sample_wb_area(*step[1:])
        elif kind == 'set':
            window.settings[step[1]] = step[2]
            window.last_saved = deepcopy(window.settings)
        else:
            raise ValueError(kind)
        results.append(dict(settings=window.snapshot(), status=window.bar.messages[messages:],
                            commits=window.commits[commits:], sample_mode=window.view.sample_mode,
                            wb_checked=window.wb_button.checked, base_checked=window.negative_base_button.checked))
    return results


def settings_with(**changes):
    return normalized({**defaults(), **changes})


# ---- ordinary photos: the display picture --------------------------------------------------------------------

DISPLAY_STEPS = [
    ['click', 0.5, 0.5], ['click', 0.0, 0.0], ['click', 1.0, 1.0], ['click', 0.9, 0.1],
    ['click', 0.78, 0.55],                       # the grey patch
    ['click', 0.2, 0.15],                        # the clipped patch: it is averaged as it is (a click does not skip)
    ['click', 0.2, 0.55],                        # the black patch
    ['click', 0.333, 0.667],
    ['area', 0.1, 0.1, 0.4, 0.4], ['area', 0.0, 0.0, 1.0, 1.0], ['area', 0.6, 0.45, 0.95, 0.9],
    ['area', 0.05, 0.1, 0.3, 0.25],              # mostly the clipped patch with some colour around it
    ['area', 0.1, 0.17, 0.28, 0.22],             # inside the clipped patch only: nothing left
    ['area', 0.1, 0.62, 0.28, 0.68],             # inside the black patch only
    ['area', 0.5, 0.5, 0.5, 0.5],                # a single point: one pixel
    ['area', 0.995, 0.995, 1.0, 1.0],
    ['area', 0.3, 0.3, 0.30001, 0.9],            # a thin column
    ['toggle_wb', False], ['click', 0.4, 0.4],
]


def display_case():
    u = picture()
    shown = u.astype(np.float32) / 255
    for tag, start in [('', {}), ('_warm', dict(temperature=35., tint=-20.)),
                       ('_saturated', dict(temperature=-80., tint=90.))]:
        window = PickWindow(np.zeros((8, 8, 3), np.float32), settings_with(**start), on_screen=shown)
        # `source` is a plain array: the picture's own ordinary branch runs
        name = 'pickers_display' + tag
        with Case('app', name) as case:
            if tag == '':
                case.array('picture', u)
            initial = window.snapshot()
            steps = armed(DISPLAY_STEPS, 'toggle_wb')
            results = run_steps(window, steps)
            case.json(start=start, initial=initial, steps=steps, results=results)


display_case()


# ---- RAW photos: the camera source of a DNG ------------------------------------------------------------------

RAW_STEPS = [
    ['click', 0.5, 0.5], ['click', 0.1, 0.1], ['click', 0.9, 0.85], ['click', 0.0, 0.0], ['click', 1.0, 1.0],
    ['click', 0.31, 0.77], ['click', 0.15, 0.58],                           # the black patch: too dark
    ['area', 0.12, 0.12, 0.22, 0.22], ['area', 0.12, 0.52, 0.22, 0.64],    # the clipped patch, the black patch
    ['area', 0.3, 0.3, 0.7, 0.7], ['area', 0.0, 0.0, 1.0, 1.0], ['area', 0.05, 0.6, 0.4, 0.95],
    ['area', 0.5, 0.5, 0.5, 0.5], ['area', 0.2, 0.2, 0.2001, 0.9],
    ['toggle_wb', False], ['click', 0.5, 0.5],
]
RAW_CASES = [   # (name, file, settings)
    ('triple', 'triple.dng', dict(raw_mode='as_shot')),
    ('triple_crop_rotation', 'triple.dng', dict(raw_mode='as_shot', crop=[0.1, 0.15, 0.8, 0.9], rotation=1)),
    ('triple_straighten_flip', 'triple.dng', dict(raw_mode='as_shot', straighten=7.5, flip=True, temperature=12.)),
    ('bayer1', 'bayer1.dng', dict(raw_mode='as_shot')),
    ('bayer2_crop', 'bayer2.dng', dict(raw_mode='daylight', crop=[0.05, 0.1, 0.9, 0.95], rotation=3)),
    ('bayer3', 'bayer3.dng', dict(raw_mode='as_shot', straighten=-4.0)),
    ('lineardual', 'lineardual.dng', dict(raw_mode='as_shot')),
]


def patterned(file, settings, film=False):
    """The DNG's camera source (its calibration record) with a picture in its pixels: the DNGs are flat, so the
    garden is laid into the camera's colours (a clipped and a black patch in it); `film` makes an orange negative.
    Returns the source and its pixels (stored by the case, the Rust test puts them into the same record)."""
    loaded, _ = engine.load_image(FILES / file, 1800, 'sRGB', settings)
    assert isinstance(loaded, rawcolor.CameraSource), file
    h, w = loaded.shape[:2]
    g = np.clip(inputs()['garden'], 0, 1)
    ys = (np.arange(h) * g.shape[0] // h)[:, None]
    xs = (np.arange(w) * g.shape[1] // w)[None, :]
    a = g[ys, xs]
    if film:
        base = np.array([.8, .55, .35], np.float32)
        a = base * (.12 + .88 * (1 - np.power(a.mean(axis=2, keepdims=True), .6)))
        a[:h // 8] = base * .97
        a[:, :w // 8] = base * 1.02
    else:
        a = a * np.array([.55, .9, .6], np.float32) * .8 + np.float32(.01)
        a[h // 10:h // 4, w // 10:w // 4] = 1.0
        a[h // 2:h // 2 + h // 6, w // 10:w // 4] = 0.0
        a[h // 3:h // 3 + h // 5, w // 2:w // 2 + w // 5] = np.array([.3, .45, .27], np.float32)
    pixels = np.ascontiguousarray(a.astype(np.float32))
    return rawcolor.CameraSource(pixels, loaded.raw_info), pixels


def raw_cases():
    for name, file, changes in RAW_CASES:
        settings = settings_with(**changes)
        source, pixels = patterned(file, settings)
        window = PickWindow(source, settings)
        with Case('app', 'pickers_raw_' + name) as case:
            owner = keep(case, 'pickers_raw_' + name, pixels)
            steps = armed(RAW_STEPS, 'toggle_wb')
            initial = window.snapshot()
            results = run_steps(window, steps)
            case.json(file=file, pixels_case=owner, settings=changes, initial=initial, steps=steps, results=results,
                      live=list(window.live_source.shape[:2]))
    # A negative: the camera source stays, but a negative has no camera white balance: the picture's branch runs.
    u = picture()
    settings = settings_with(raw_mode='legacy', negative=True, negative_base=[.8, .7, .6],
                             negative_peak=[.02, .03, .01])
    source, pixels = patterned('bayer0.dng', settings, film=True)
    window = PickWindow(source, settings, on_screen=u.astype(np.float32) / 255)
    steps = armed([['click', 0.5, 0.5], ['area', 0.1, 0.1, 0.5, 0.5], ['click', 0.9, 0.9]], 'toggle_wb')
    with Case('app', 'pickers_raw_negative_display') as case:
        owner = keep(case, 'pickers_raw_negative_display', pixels)
        initial = window.snapshot()
        results = run_steps(window, steps)
        case.json(file='bayer0.dng', pixels_case=owner, initial=initial, steps=steps, results=results,
                  settings=dict(raw_mode='legacy', negative=True, negative_base=[.8, .7, .6],
                                negative_peak=[.02, .03, .01]), live=list(window.live_source.shape[:2]))


raw_cases()


# ---- film negatives: the film base -------------------------------------------------------------------------

def scan():
    """A film scan: orange base around a darker picture, float32 like a plain source (80 x 120)."""
    g = np.clip(inputs()['garden'][::2, ::2].mean(axis=2, keepdims=True), 0, 1)
    base = np.array([.8, .55, .35], np.float32)
    a = base * (.12 + .88 * (1 - np.power(g, .6)))
    a[:10] = base * .97                       # clear film along the top and the left
    a[:, :10] = base * 1.02
    a[60:70, 100:118] = base * np.array([.9, 1.0, 1.1], np.float32)       # a bright patch with a different cast
    return np.ascontiguousarray(a.astype(np.float32))


BASE_STEPS = [
    ['click', 0.03, 0.5], ['click', 0.5, 0.03], ['click', 0.5, 0.5],             # clear film, clear film, picture
    ['click', 0.9, 0.9], ['click', 0.0, 0.0],
    ['area', 0.0, 0.0, 0.08, 1.0], ['area', 0.2, 0.0, 0.9, 0.08],
    ['area', 0.3, 0.3, 0.7, 0.7],                          # the picture: refused
    ['area', 0.85, 0.75, 0.97, 0.9],
    ['area', 0.4, 0.4, 0.4, 0.4],
    ['set', 'negative_peak', [0.9, 0.7, 0.5]], ['click', 0.03, 0.5],       # too dark for clear film: refused
    ['set', 'negative_peak', None], ['click', 0.03, 0.5],   # no peak yet: refused
    ['set', 'negative', False], ['click', 0.03, 0.5],       # not a negative: off
]


def negative_cases():
    plain = scan()
    changes = dict(negative=True, negative_base=[.8, .55, .35], negative_peak=[.03, .02, .01])
    for tag, source_changes in [('plain', {}), ('plain_crop_rotation', dict(crop=[0.1, 0.1, 0.9, 0.95], rotation=1)),
                                ('plain_straighten', dict(straighten=6.0))]:
        settings = settings_with(**changes, **source_changes)
        window = PickWindow(plain, settings)
        name = 'pickers_negative_' + tag
        with Case('app', name) as case:
            if tag == 'plain':
                case.array('source', plain)
            initial = window.snapshot()
            steps = armed(BASE_STEPS, 'toggle_base')
            results = run_steps(window, steps)
            case.json(initial=initial, steps=steps, results=results, settings=dict(changes, **source_changes),
                      live=list(window.live_source.shape[:2]))
    raw_changes = dict(raw_mode='legacy', negative=True, negative_base=[.8, .7, .6], negative_peak=[.03, .02, .01])
    for tag, file, source_changes in [('raw_bayer0', 'bayer0.dng', {}),
                                      ('raw_triple_crop', 'triple.dng', dict(crop=[0.0, 0.1, 0.9, 0.95], rotation=2))]:
        settings = settings_with(**raw_changes, **source_changes)
        source, pixels = patterned(file, settings, film=True)
        window = PickWindow(source, settings)
        name_of = lambda tag: 'pickers_negative_' + tag  # noqa: E731
        with Case('app', 'pickers_negative_' + tag) as case:
            owner = keep(case, name_of(tag), pixels)
            steps = armed(BASE_STEPS, 'toggle_base')
            initial = window.snapshot()
            results = run_steps(window, steps)
            case.json(file=file, pixels_case=owner, initial=initial, steps=steps, results=results,
                      settings=dict(raw_changes, **source_changes), live=list(window.live_source.shape[:2]))


negative_cases()



# ---- the two buttons: which one is pressed, the status texts -----------------------------------------------------

TOGGLE_STEPS = [
    ['toggle_base', True],            # not a negative yet: nothing
    ['toggle_wb', True], ['toggle_wb', False], ['toggle_wb', True],
    ['set', 'negative', True],
    ['toggle_base', True],            # the film base replaces the eyedropper
    ['toggle_wb', True],              # and back
    ['toggle_base', True], ['toggle_base', False],
    ['toggle_wb', True], ['toggle_base', True], ['toggle_base', False],
    ['set', 'negative', False], ['toggle_base', True],
]

with Case('app', 'pickers_toggles') as case:
    _settings = settings_with()
    _window = PickWindow(np.zeros((8, 8, 3), np.float32), _settings, on_screen=np.zeros((8, 8, 3), np.float32))
    _initial = _window.snapshot()
    _results = run_steps(_window, TOGGLE_STEPS)
    case.json(initial=_initial, steps=TOGGLE_STEPS, results=_results)
print('pickers goldens written')
