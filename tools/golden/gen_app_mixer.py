"""Goldens of grainy-app work package 7: the colour mixer, its targeted adjustment and the point colours.

* `golden/app/mixer_targeted`: luma/targeted.py `hsl_weights` and `drag_hsl`, and a session through the oracle's
  own `TargetedAdjust` (`toggle('hsl')`, `started`, `dragged`, `finished`) on a stand-in window whose `hsl_color`
  and `hsl_target` are the real combo boxes; after every step the `hsl` value, the selected colour and the commits.
* `golden/app/mixer_panel`: luma/app.py `hsl_changed` / `update_hsl` run unbound on a stand-in (the colour combo
  and the three sliders), after every step the `hsl` value and what the sliders show.
* `golden/app/mixer_points`: luma/studio.py `pick_point_color` (its job run at once on a generated source through
  luma/point_color.py `sample`), `load_points`, `load_point`, `point_changed`, `remove_point` run unbound on a
  stand-in with the panel's real widgets (combo, `Adjustment` sliders, check box, note); after every step the
  `point_colors`, the selected point, the sliders, the note, the status and the commits.
* `golden/app/mixer_coverage`: luma/point_color.py `sample` at several places and `preview` (the coverage the
  선택 색상 범위 표시 overlay draws) of every point of a list with versions 1 and 2.

Nothing is written outside `golden/app/mixer*`. Run:
    "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_mixer.py
"""
from copy import deepcopy

from common import Case, inputs, np

from PySide6.QtWidgets import QApplication, QCheckBox, QComboBox, QLabel  # noqa: E402
from luma import point_color, targeted  # noqa: E402
from luma.app import MainWindow  # noqa: E402
from luma.engine import defaults, normalized, to_srgb  # noqa: E402
from luma.panel_keys import switch_on  # noqa: E402
from luma.studio import StudioTools  # noqa: E402
from luma.widgets import Adjustment  # noqa: E402

_qt = QApplication.instance() or QApplication([])

COLOURS = ['빨강', '주황', '노랑', '초록', '청록', '파랑', '보라', '자홍']


class NoOp:
    def __getattr__(self, name):
        return self

    def __call__(self, *args, **kwargs):
        return self


def display_picture():
    """A display-referred picture as the view holds it, quantized to 8 bits (the Rust frame is 8-bit)."""
    a = np.clip(to_srgb(inputs()['colours'][:24, :36]), 0, 1)
    return (np.round(a * 255) / 255).astype(np.float32)


# ---- the targeted HSL adjustment ---------------------------------------------------------------------------------

class TargetView(NoOp):
    def __init__(self, picture):
        self.on_screen = picture
        self.crop_mode = False
        self.sample_mode = False


class TargetWindow(NoOp):
    """The attributes of `MainWindow` that `TargetedAdjust` reads for the mixer; commits are recorded."""

    def __init__(self, picture, settings):
        self.view = TargetView(picture)
        self.source = picture
        self.settings = settings
        self.last_saved = deepcopy(settings)
        self.commits = []
        self.wb_button = NoOp()
        self.hsl_color = QComboBox()
        self.hsl_color.addItems(COLOURS)
        self.hsl_target = QComboBox()
        self.hsl_target.addItems(['색상', '채도', '밝기 (HSV)'])
        self.hsl_target.setCurrentIndex(1)

    def commit(self, label='보정 변경'):
        if self.settings != self.last_saved:
            self.commits.append(dict(label=label, hsl=deepcopy(self.settings['hsl'])))
            self.last_saved = deepcopy(self.settings)

    def set_setting(self, key, value):
        self.settings[key] = deepcopy(value)
        switch_on(self.settings, (key,))

    def update_hsl(self, *args):
        pass


WEIGHT_COLOURS = [
    [0.5, 0.5, 0.5], [1.0, 0.0, 0.0], [1.0, 0.5, 0.0], [0.9, 0.85, 0.1], [0.2, 0.8, 0.3], [0.1, 0.8, 0.8],
    [0.1, 0.2, 0.9], [0.55, 0.2, 0.9], [0.9, 0.1, 0.7], [1.0, 1.0, 1.0], [0.0, 0.0, 0.0], [0.6, 0.4, 0.4],
    [0.3, 0.31, 0.29], [1.0, 0.25, 0.0], [0.0, 1.0, 0.5], [0.95, 0.05, 0.3], [1.2, 0.4, -0.1],
]
TARGET_POINTS = [[0.5, 0.5], [0.0, 0.0], [1.0, 1.0], [0.1, 0.9], [0.73, 0.21], [0.999, 0.5], [0.4, 0.05]]
START_HSL = [[0, 0, 0], [10.0, -20.0, 5.0], [0.0, 95.0, 0.0], [0, 0, 0], [-100.0, 0.0, 30.0], [0, 0, 0],
             [0.0, 0.0, -99.5], [3.0, 3.0, 3.0]]
HSL_DRAGS = [   # (weights colour index, column, delta)
    (1, 1, 12.5), (2, 0, -7.0), (3, 2, 40.0), (5, 1, -250.0), (6, 2, 0.5), (8, 0, 300.0), (11, 1, 1.0),
    (13, 2, -33.3), (0, 1, 9.0),
]
TARGET_SESSION = [   # ('toggle', checked) | ('started', x, y) | ('dragged', pixels) | ('finished',) | ('target', i)
    ('toggle', True), ('started', 0.3, 0.6), ('dragged', 12.0), ('dragged', 45.0), ('finished',),
    ('target', 0), ('started', 0.82, 0.15), ('dragged', -30.0), ('dragged', -90.0), ('finished',),
    ('target', 2), ('started', 0.05, 0.95), ('dragged', 500.0), ('finished',),
    ('started', 0.6, 0.4), ('dragged', 0.0), ('finished',),
    ('target', 1), ('started', 0.45, 0.5), ('dragged', -7.0), ('finished',),
    ('toggle', False), ('started', 0.5, 0.5), ('dragged', 50.0), ('finished',),
]

with Case('app', 'mixer_targeted') as case:
    picture = case.array('picture', display_picture())
    samples = [targeted.sample(picture, x, y).tolist() for x, y in TARGET_POINTS]
    weights = [targeted.hsl_weights(np.asarray(rgb, np.float32)).tolist() for rgb in WEIGHT_COLOURS + samples]
    drags = [targeted.drag_hsl(START_HSL, np.asarray(weights[i], np.float32), column, delta)
             for i, column, delta in HSL_DRAGS]
    window = TargetWindow(picture, normalized({**defaults(), 'hsl': deepcopy(START_HSL), 'panels_off': ['mixer']}))
    adjust = targeted.TargetedAdjust(window)
    adjust.buttons = {'curve': NoOp(), 'hsl': NoOp()}
    trace = []
    for step in TARGET_SESSION:
        if step[0] == 'toggle':
            adjust.toggle('hsl', step[1])
        elif step[0] == 'started':
            adjust.started(step[1], step[2])
        elif step[0] == 'dragged':
            adjust.dragged(step[1])
        elif step[0] == 'target':
            window.hsl_target.setCurrentIndex(step[1])
        else:
            adjust.finished()
        trace.append(dict(hsl=deepcopy(window.settings['hsl']), color=window.hsl_color.currentIndex(),
                          panels_off=list(window.settings['panels_off']), commits=len(window.commits)))
    case.json(points=TARGET_POINTS, samples=samples, colours=WEIGHT_COLOURS + samples, weights=weights,
              start=START_HSL, drag_cases=[list(c) for c in HSL_DRAGS], drags=drags,
              session=[list(s) for s in TARGET_SESSION], trace=trace, commits=window.commits,
              per_pixel=targeted.HSL_PER_PIXEL, centers=targeted.HSL_CENTERS.tolist())


# ---- the mixer panel ---------------------------------------------------------------------------------------------

class Shown:
    def __init__(self):
        self.value = None

    def set_value(self, value):
        self.value = value


class MixerWindow:
    hsl_changed = MainWindow.hsl_changed
    update_hsl = MainWindow.update_hsl

    def __init__(self):
        self.settings = normalized(defaults())
        self.hsl_color = QComboBox()
        self.hsl_color.addItems(COLOURS)
        self.hsl_color.currentIndexChanged.connect(self.update_hsl)
        self.hsl_widgets = [Shown(), Shown(), Shown()]
        self.update_hsl()

    def set_setting(self, key, value):
        self.settings[key] = deepcopy(value)
        switch_on(self.settings, (key,))


MIXER_STEPS = [
    ('color', 3), ('change', '1', 25.0), ('change', '0', -40.0), ('color', 0), ('change', '2', 100.0),
    ('color', 3), ('color', 7), ('change', '1', -100.0), ('change', '1', 0.0), ('color', 5), ('change', '0', 7.0),
]

with Case('app', 'mixer_panel') as case:
    window = MixerWindow()
    window.settings['panels_off'] = ['mixer']
    trace = []
    for step in MIXER_STEPS:
        if step[0] == 'color':
            window.hsl_color.setCurrentIndex(step[1])
        else:
            window.hsl_changed(step[1], step[2])
        trace.append(dict(hsl=deepcopy(window.settings['hsl']), shown=[w.value for w in window.hsl_widgets],
                          color=window.hsl_color.currentIndex(), panels_off=list(window.settings['panels_off'])))
    case.json(steps=[list(s) for s in MIXER_STEPS], trace=trace)


# ---- point colours -----------------------------------------------------------------------------------------------

def point_source():
    """The garden, halved (80 x 120), scene-linear float32: the develop source of the picks."""
    return np.ascontiguousarray(inputs()['garden'][::2, ::2], dtype=np.float32)


POINT_SETTINGS = dict(exposure=0.4, contrast=15.0, saturation=10.0, mixer_mode='hsl', crop=[0.1, 0.05, 0.9, 0.95],
                      hsl=[[0, 0, 0], [0, 20.0, 0], [0, 0, 0], [15.0, 0, -10.0], [0, 0, 0], [0, 0, 0], [0, 0, 0],
                           [0, 0, 0]])


class PointView:
    fit_mode = True


class PointWindow:
    """MainWindow's attributes and methods the point colours use (app.py set_setting, commit, spawn)."""

    def __init__(self, source):
        self.settings = normalized({**defaults(), **deepcopy(POINT_SETTINGS), 'panels_off': ['mixer']})
        self.last_saved = deepcopy(self.settings)
        self.source = source
        self.full_source = None
        self.original_size = None
        self.view = PointView()
        self.current_id = 1
        self.load_version = 1
        self.closing = False
        self.commits = []
        self.messages = []

    def statusBar(self):
        return self

    def showMessage(self, text, *args):
        self.messages.append(text)

    def commit(self, label='보정 변경'):
        if self.current_id and self.settings != self.last_saved:
            self.commits.append(dict(label=label, point_colors=deepcopy(self.settings['point_colors'])))
            self.last_saved = deepcopy(self.settings)

    def set_setting(self, key, value):
        if self.source is None:
            return
        self.settings[key] = deepcopy(value)
        switch_on(self.settings, (key,))

    def finish_interaction(self):
        self.commit()

    def render(self):
        pass

    def spawn(self, work, ready, failed):
        try:
            value = work()
        except Exception as error:   # noqa: BLE001 - the oracle's job reports any failure
            failed(error)
        else:
            ready(value)


class PointStudio:
    """The point-colour controls of build_color (studio.py), the real widgets."""

    def __init__(self, window):
        self.w = window
        self.loading = False
        self.color_pick_generation = 0
        self.color_pick_running = False
        self.color_pick_token = 0
        self.layer_index = -1
        self.point_index = -1
        self.point_note = QLabel('사진에서 색을 고른 뒤 범위를 조절하세요.')
        self.points = QComboBox()
        self.points.currentIndexChanged.connect(self.load_point)
        self.point_controls = {}
        for key, title, low, high, scale in [('range', '색상 범위', .01, .5, 100),
                                             ('saturation_range', '채도 범위', .01, 1, 100),
                                             ('lightness_range', '명도 범위', .01, 1, 100),
                                             ('hue', '색상', -100, 100, 1), ('saturation', '채도', -100, 100, 1),
                                             ('lightness', '명도', -100, 100, 1)]:
            c = Adjustment(key, title, low, high, scale)
            c.changed.connect(self.point_changed)
            c.committed.connect(self.w.finish_interaction)
            self.point_controls[key] = c
            c.default_value = {'range': .12, 'saturation_range': .6, 'lightness_range': .5}.get(key, 0)
        self.point_preview = QCheckBox('선택 색상 범위 표시')

    def mode(self, kind):
        # StudioTools.mode's part that matters here: a new pick generation (the rest drives the view).
        self.color_pick_generation += 1

    def preview_ready(self, *args):
        pass


for _name in ('load_points', 'load_point', 'point_changed', 'remove_point', 'pick_point_color'):
    setattr(PointStudio, _name, getattr(StudioTools, _name))


def control_state(studio):
    return {key: dict(value=c.slider.value() / c.scale, enabled=c.isEnabled())
            for key, c in studio.point_controls.items()}


POINT_SESSION = [   # ('pick', x, y) | ('slide', key, value) | ('release', key) | ('select', i) | ('remove',)
    #                 | ('legacy', point) | ('reset', key)
    ('pick', 0.30, 0.40), ('slide', 'hue', 30.0), ('slide', 'hue', 45.0), ('release', 'hue'),
    ('slide', 'range', 0.2), ('release', 'range'),
    ('pick', 0.72, 0.65), ('select', 0), ('slide', 'saturation', -40.0), ('slide', 'lightness', 25.0),
    ('release', 'lightness'), ('slide', 'saturation_range', 0.35), ('release', 'saturation_range'),
    ('select', 1), ('reset', 'range'), ('remove',),
    ('legacy', {'rgb': [0.6, 0.3, 0.2], 'range': 0.1, 'hue': 20, 'saturation': 10}),
    ('select', 1), ('slide', 'hue', -15.0), ('release', 'hue'), ('slide', 'lightness_range', 0.4),
    ('release', 'lightness_range'),
    ('pick', 0.0, 1.0), ('select', 2), ('remove',), ('remove',), ('remove',), ('remove',),
]

with Case('app', 'mixer_points') as case:
    source = case.array('source', point_source())
    window = PointWindow(source)
    studio = PointStudio(window)
    studio.load_points()
    trace = []
    for step in POINT_SESSION:
        kind = step[0]
        if kind == 'pick':
            studio.pick_point_color([step[1], step[2]])
        elif kind == 'slide':
            control = studio.point_controls[step[1]]
            control.slider.setValue(round(step[2] * control.scale))
        elif kind == 'release':
            studio.point_controls[step[1]].committed.emit()
        elif kind == 'reset':
            studio.point_controls[step[1]].reset()
        elif kind == 'select':
            studio.points.setCurrentIndex(step[1])
        elif kind == 'remove':
            studio.remove_point()
            window.commit()     # the commit timer of set_setting
        elif kind == 'legacy':
            window.settings['point_colors'] = deepcopy(window.settings['point_colors']) + [deepcopy(step[1])]
            window.commit()
            studio.load_points()
        trace.append(dict(point_colors=deepcopy(window.settings['point_colors']), index=studio.point_index,
                          items=studio.points.count(), current=studio.points.currentIndex(),
                          controls=control_state(studio), preview_enabled=studio.point_preview.isEnabled(),
                          note=studio.point_note.text(), message=window.messages[-1] if window.messages else '',
                          panels_off=list(window.settings['panels_off']), commits=len(window.commits)))
    case.json(settings=POINT_SETTINGS, session=[list(s) for s in POINT_SESSION], trace=trace,
              commits=window.commits)


COVERAGE_POINTS = [
    dict(version=2, rgb=[0.62, 0.38, 0.21], range=0.12, saturation_range=0.6, lightness_range=0.5, hue=30,
         saturation=0, lightness=0),
    dict(version=2, rgb=[0.5, 0.5, 0.505], range=0.2, saturation_range=0.3, lightness_range=0.25, hue=0,
         saturation=-50, lightness=0),
    dict(version=2, rgb=[0.2, 0.5, 0.1], range=0.5, saturation_range=1, lightness_range=1, hue=0, saturation=0,
         lightness=40),
    dict(rgb=[0.25, 0.4, 0.15], range=0.15, hue=-20, saturation=30, lightness=5),
    dict(version=2, rgb=[0.3, 0.45, 0.2], range=0.08, saturation_range=0.45, lightness_range=0.3, hue=0,
         saturation=0, lightness=0),
]
COVERAGE_SAMPLES = [[0.5, 0.5], [0.0, 0.0], [1.0, 1.0], [0.25, 0.8], [0.9, 0.1]]

with Case('app', 'mixer_coverage') as case:
    source = case.array('source', point_source())
    settings = normalized({**defaults(), **deepcopy(POINT_SETTINGS), 'point_colors': deepcopy(COVERAGE_POINTS)})
    samples = [point_color.sample(source, settings, p) for p in COVERAGE_SAMPLES]
    for index in range(len(COVERAGE_POINTS)):
        case.array(f'coverage_{index}', point_color.preview(source, settings, (None, index)).astype(np.float32))
    errors = []
    for bad in ([1.2, 0.5], [-0.1, 0.5], [float('nan'), 0.5]):
        try:
            point_color.sample(source, settings, bad)
            errors.append('')
        except ValueError as error:
            errors.append(str(error))
    case.json(settings=POINT_SETTINGS, points=COVERAGE_POINTS, sample_points=COVERAGE_SAMPLES, samples=samples,
              bad_points=['[1.2, 0.5]', '[-0.1, 0.5]', '[nan, 0.5]'], errors=errors)
