"""Goldens of grainy-app's Transform panel (work package 19): `golden/app/transform_*`.

* `transform_guides_*`: guided upright. luma/studio.py `StudioTools.start_guides`, `mode`, `stroke` (its guide
  branch: the corner on the source, the convexity and area check, `guides`, the commit), `source_point`,
  `source_points` and luma/widgets.py `PhotoView.set_tool`, `cancel_stroke`, `keyPressEvent` run on stand-ins for the
  window, the view and the panel. The view's tool stroke (press inside the photo, move, release) is copied from
  `PhotoView.mousePressEvent` / `mouseMoveEvent` / `mouseReleaseEvent` with the points already in frame fractions
  (`normalized_point` clamps them to the frame). After every step: the view's tool, the corners placed, `guides`,
  the commits, the status message and the message boxes shown.
* `transform_upright_*`: the automatic alignment buttons. `StudioTools.auto_upright` and `clear_upright` run on a
  stand-in window whose `spawn` keeps the job until the script finishes it (the work is the oracle's own: lens
  shading, geometry and `upright.estimate` on a generated scene). The labels are the lines of
  `StudioTools.load_settings` that show the alignment (copied below as `load_labels`). After every step: `upright`,
  `straighten`, `upright_crop`, the commits, both labels, whether the buttons are enabled, the status message and
  the outcome of a finished job (its result, or the last line of its traceback).

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_transform.py
"""
import traceback
from copy import deepcopy

import cv2

from common import Case, np
from luma import studio as studio_module
from luma.engine import defaults, normalized
from luma.i18n import tr as oracle_tr
from luma.panel_keys import switch_on
from luma.studio import StudioTools
from luma.widgets import PhotoView
from PySide6.QtCore import Qt


# ---- stand-ins (the few lines copied from gen_app.py's healing section) ----------------------------------------

class Emitter:
    def __init__(self, target=None):
        self.target = target

    def emit(self, *values):
        if self.target is not None:
            self.target(*values)


class Quiet:
    def __getattr__(self, name):
        return lambda *a, **k: None


class Text:
    def __init__(self, text=''):
        self.value = text
        self.enabled = True

    def setText(self, text):
        self.value = text

    def setEnabled(self, enabled):
        self.enabled = bool(enabled)


class Check:
    def __init__(self):
        self.checked = False

    def isChecked(self):
        return self.checked

    def setChecked(self, value):
        self.checked = bool(value)


class Key:
    def __init__(self, key):
        self.code = key

    def key(self):
        return self.code

    def accept(self):
        pass


class Adjustment:
    class Slider:
        def __init__(self, position):
            self.position = position

        def value(self):
            return self.position

    def __init__(self, position):
        self.slider = Adjustment.Slider(position)


BOXES = []


def information(parent, title, text):
    BOXES.append([title, text])


studio_module.QMessageBox.information = staticmethod(information)


class GuideView:
    set_tool = PhotoView.set_tool
    cancel_stroke = PhotoView.cancel_stroke
    keyPressEvent = PhotoView.keyPressEvent

    def __init__(self):
        self.tool_mode = ''
        self.tool_points = []
        self.tool_alt = False
        self.tool_shift = False
        self.brush_radius = .025
        self.brush_feather = 70
        self.healing_overlay = None
        self.tool_overlay = None
        self.on_screen = None
        self.sample_mode = False
        self.crop_mode = False
        self.handle_drag = None
        self.tool_handles = []
        self.brush_position = None
        self.adjust_origin = None
        self.strokeCancelled = Emitter()
        self.toolModeChanged = Emitter()

    def viewport(self):
        return Quiet()

    def sync_cursor(self):
        pass

    # PhotoView.mousePressEvent / mouseMoveEvent / mouseReleaseEvent, the tool stroke part, in frame fractions.
    def press(self, point):
        if self.tool_mode and 0 <= point[0] <= 1 and 0 <= point[1] <= 1:
            self.tool_points = [list(point)]

    def move(self, point):
        point = [min(1., max(0., point[0])), min(1., max(0., point[1]))]
        if self.tool_mode and self.tool_points and self.tool_points[-1] != point:
            self.tool_points.append(point)

    def release(self, point):
        point = [min(1., max(0., point[0])), min(1., max(0., point[1]))]
        if self.tool_mode and self.tool_points:
            if self.tool_points[-1] != point:
                self.tool_points.append(point)
            points = self.tool_points
            self.tool_points = []
            self.studio.stroke(points, self.tool_alt)


class Window:
    """MainWindow's attributes and methods the Transform panel uses (app.py set_setting, commit, spawn)."""

    def __init__(self, changes, width, height, source=None):
        self.settings = normalized({**defaults(), **changes})
        self.last_saved = deepcopy(self.settings)
        self.current_id = 1
        self.load_version = 1
        self.live_source = np.zeros((height, width, 3), np.float32)
        self.source = self.live_source if source is None else source
        self.history = []
        self.messages = []
        self.jobs = []
        self.crop_button = Check()
        self.wb_button = Check()
        self.before_button = Check()
        self.auto_level_button = Text()
        self.auto_level_note = Text(oracle_tr('직선이 보이는 사진에서 자동 보정한 뒤 아래 슬라이더로 미세 조절하세요.'))
        self.view = GuideView()

    def statusBar(self):
        return self

    def showMessage(self, text):
        self.messages.append(text)

    def commit(self, label='보정 변경'):
        if self.current_id and self.settings != self.last_saved:
            self.history.append(label)
            self.last_saved = deepcopy(self.settings)

    def set_setting(self, key, value):
        if self.source is None:
            return
        self.studio.cancel_stroke()
        self.settings[key] = deepcopy(value)
        switch_on(self.settings, (key,))
        self.before_button.setChecked(False)

    def finish_interaction(self):
        self.commit()

    def load_controls(self):
        self.studio.load_labels()

    def render(self):
        pass

    def set_mode(self, index):
        pass

    def spawn(self, work, finished, failed):
        self.jobs.append((work, finished, failed))

    def finish_job(self):
        work, finished, failed = self.jobs.pop(0)
        try:
            result = work()
        except Exception:
            error = traceback.format_exc()
            failed(error)
            return {'error': error.strip().splitlines()[-1]}
        finished(result)
        return {'result': result}


class Studio:
    def __init__(self, window):
        self.w = window
        window.studio = window.view.studio = self
        window.view.strokeCancelled.target = self.cancel_stroke
        self.loading = False
        self.photo_id = 1
        self.guide_points = []
        self.coordinate_key = None
        self.coordinates = None
        self.color_pick_generation = 0
        self.pending_stroke = None
        self.stroke_generation = 0
        self.overlay_paused = False
        self.upright_generation = 0
        self.radius = Adjustment(25)
        self.feather = Adjustment(70)
        self.retouch_radius = Adjustment(15)
        self.retouch_feather = Adjustment(60)
        self.upright_buttons = [Text() for _ in range(4)]
        self.upright_label = Text(oracle_tr('자동 정렬 미적용'))

    def preview_ready(self, *_):
        pass

    def load_labels(self):
        # StudioTools.load_settings: the lines that show the alignment.
        s = self.w.settings
        upright = s.get('upright')
        modes = {'level': '수평', 'vertical': '수직', 'full': '전체 원근'}
        self.upright_label.setText(oracle_tr('{0} 적용 · 회전 {1}° · 가장자리 확대 {2}배', f"{modes.get(upright['mode'], '자동')}", f"{upright['rotation']:+.2f}", f"{upright['crop_scale']:.2f}") if upright else oracle_tr('자동 정렬 미적용'))
        self.w.auto_level_note.setText(oracle_tr('자동 보정 {0}° · 아래 슬라이더로 미세 조절할 수 있습니다.', f"{upright['rotation']:+.2f}") if upright else oracle_tr('직선이 보이는 사진에서 자동 보정한 뒤 아래 슬라이더로 미세 조절하세요.'))


for _name in ('mode', 'start_guides', 'source_point', 'source_points', 'stroke', 'stroke_valid', 'cancel_stroke',
              'auto_upright', 'clear_upright'):
    setattr(Studio, _name, getattr(StudioTools, _name))


# ---- guided upright ---------------------------------------------------------------------------------------------

def guide_step(studio, step):
    w = studio.w
    kind, *args = step
    if kind == 'start':
        studio.start_guides()
    elif kind == 'press':
        w.view.press(args[0])
    elif kind == 'move':
        w.view.move(args[0])
    elif kind == 'release':
        w.view.release(args[0])
    elif kind == 'click':
        w.view.press(args[0])
        w.view.release(args[0])
    elif kind == 'escape':
        w.view.keyPressEvent(Key(Qt.Key.Key_Escape))
    elif kind == 'clear':
        # 가이드 원근 초기화: set_setting, then the commit timer.
        w.set_setting('guides', None)
        w.commit()
    else:
        raise ValueError(kind)


def guide_state(studio):
    w = studio.w
    return dict(
        mode=w.view.tool_mode, stroke=deepcopy(w.view.tool_points), corners=deepcopy(studio.guide_points),
        guides=deepcopy(w.settings['guides']), panels_off=list(w.settings.get('panels_off') or []),
        history=list(w.history), status=w.messages[-1] if w.messages else None, boxes=deepcopy(BOXES))


QUAD = [['click', [0.1, 0.15]], ['click', [0.85, 0.1]], ['click', [0.9, 0.9]], ['click', [0.12, 0.8]]]

GUIDE_CASES = {
    # Four clicks in order: the guides are set and committed; the tool ends.
    'plain': ({}, 97, 65, [['start'], *QUAD]),
    # A turned, straightened, cropped frame with a manual perspective: a click on the empty canvas is refused, a
    # stroke that starts there takes its first point on the photo, Esc during a stroke and Esc to end the tool,
    # a crossed quadrilateral and a tiny one are refused with a message box, then 가이드 원근 초기화.
    'transformed': ({'rotation': 1, 'straighten': 6.0, 'perspective_v': 20.0, 'crop': [0.05, 0.05, 0.95, 0.95]},
                    97, 65, [
        ['start'],
        ['click', [0.0, 0.0]],
        ['press', [0.0, 0.02]], ['move', [0.2, 0.25]], ['move', [0.3, 0.3]], ['release', [0.3, 0.3]],
        ['press', [0.6, 0.2]], ['move', [0.65, 0.25]], ['escape'], ['release', [0.7, 0.3]],
        ['press', [1.4, 0.5]],
        ['click', [0.8, 0.2]],
        ['escape'],
        ['click', [0.5, 0.5]],
        ['start'],
        ['click', [0.2, 0.2]], ['click', [0.8, 0.8]], ['click', [0.8, 0.2]], ['click', [0.2, 0.8]],
        ['start'],
        ['click', [0.5, 0.5]], ['click', [0.53, 0.5]], ['click', [0.53, 0.53]], ['click', [0.5, 0.53]],
        ['start'], *QUAD,
        ['clear'],
    ]),
    # Guides set already: the new corners are found through them (guides are part of the geometry).
    'again': ({'guides': [[0.05, 0.1], [0.9, 0.02], [0.95, 0.97], [0.02, 0.9]]}, 97, 65, [
        ['start'],
        ['click', [0.25, 0.2]], ['click', [0.75, 0.22]], ['click', [0.8, 0.85]], ['click', [0.2, 0.8]],
    ]),
}

for name, (changes, width, height, script) in GUIDE_CASES.items():
    BOXES.clear()
    with Case('app', f'transform_guides_{name}') as case:
        window = Window(changes, width, height)
        studio = Studio(window)
        states = []
        for step in script:
            guide_step(studio, step)
            states.append(guide_state(studio))
        case.json(changes=changes, width=width, height=height, script=script, states=states)


# ---- automatic alignment ------------------------------------------------------------------------------------------

def scene(kind, h=240, w=320):
    """Scenes of gen_auto.py: a facade turned by 4 degrees, or an empty grey frame."""
    if kind == 'building':
        canvas = np.full((h * 2, w * 2), 70, np.uint8)
        for x in range(40, 2 * w, 50):
            cv2.rectangle(canvas, (x, 30), (x + 22, 2 * h - 30), 200, -1)
        for y in range(60, 2 * h, 70):
            cv2.line(canvas, (0, y), (2 * w, y), 140, 3)
        m2 = cv2.getRotationMatrix2D((w, h), 4, 1)
        img = cv2.warpAffine(canvas, m2, (2 * w, 2 * h))[h // 2:h // 2 + h, w // 2:w // 2 + w].copy()
    else:
        img = np.full((h, w), 60, np.uint8)
    return np.repeat((img.astype(np.float32) / np.float32(255))[..., None], 3, axis=2)


def upright_step(studio, step):
    w = studio.w
    kind, *args = step
    if kind == 'upright':
        studio.auto_upright(args[0])
    elif kind == 'finish':
        return w.finish_job()
    elif kind == 'clear':
        studio.clear_upright()
    elif kind == 'set':
        w.set_setting(args[0], args[1])
    elif kind == 'commit':
        w.commit()
    else:
        raise ValueError(kind)
    return None


def upright_state(studio, outcome):
    w = studio.w
    return dict(
        upright=deepcopy(w.settings['upright']), straighten=w.settings['straighten'],
        upright_crop=w.settings['upright_crop'], history=list(w.history), label=studio.upright_label.value,
        level_note=w.auto_level_note.value, enabled=all(b.enabled for b in studio.upright_buttons),
        level_enabled=w.auto_level_button.enabled, status=w.messages[-1] if w.messages else None, outcome=outcome)


UPRIGHT_CASES = {
    'building': ('building', {}, [
        ['upright', 'full'], ['finish'],
        ['upright', 'level'], ['finish'],
        ['upright', 'vertical'], ['finish'],
        ['upright', 'auto'], ['finish'],
        ['clear'],
        ['set', 'upright_crop', False], ['commit'],
        ['set', 'straighten', 3.0], ['commit'],
        ['upright', 'level'], ['set', 'exposure', 0.5], ['finish'],
        ['upright', 'auto'], ['upright', 'level'], ['finish'], ['finish'],
        ['clear'],
    ]),
    'empty': ('empty', {'straighten': -2.0}, [
        ['upright', 'full'], ['finish'],
        ['upright', 'level'], ['finish'],
        ['upright', 'auto'], ['set', 'exposure', 0.5], ['finish'],
    ]),
}

for name, (source, changes, script) in UPRIGHT_CASES.items():
    with Case('app', f'transform_upright_{name}') as case:
        window = Window(changes, 320, 240, scene(source))
        studio = Studio(window)
        studio.load_labels()
        states = []
        for step in script:
            outcome = upright_step(studio, step)
            states.append(upright_state(studio, outcome))
        case.json(changes=changes, script=script, states=states)
