"""Goldens of grainy-app.

* the histogram and clipping of luma/widgets.py (`histogram_bins`, `clipping_channels`);
* the coordinate maps of the canvas tools (luma/studio.py `source_point`, `source_points`, `frame_affine`,
  `screen_point`, `gradient_point`), run as the oracle's own methods on a stand-in for the window;
* a library built with the oracle's own code (`golden/app/library`): photos made from the standard inputs and
  saved as PNG/JPEG, imported as `import_next` does (`engine.thumbnail(path, with_info=True)`, `Catalog.add`),
  edited through `MainWindow.commit` / `undo` / `redo`, rated, labelled, stacked, a collection, a smart
  collection and a virtual copy; then the listings `library_query.query` gives for a spread of filters and sorts,
  each photo's settings and history, and the tables after a scripted session run through the oracle's own
  `MainWindow` methods (commits, undo, redo, ratings, labels, paste, history restore, virtual copy).

The window's methods are called unbound on `Window`, a stand-in with the attributes they read; widgets they
touch are no-ops. Nothing is written outside a temporary folder and `golden/app`; the paths stored in the
library are moved to the fixed folder `Z:\\GrainyGolden` so the goldens do not depend on this machine.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app.py
"""
import re
import json
import shutil
import sqlite3
import tempfile
import threading
from copy import deepcopy
from datetime import datetime
from pathlib import Path, PureWindowsPath

from common import GOLDEN, Case, inputs, np, write_json
from luma import widgets


def frame():
    """A display frame with clipping on both ends and more than 600 rows (histogram step 2): the garden, brightened,
    with a strip pushed below 0 and above 1."""
    garden = inputs()['garden'][:, :40]                      # 160 x 40, scene-linear float32
    a = np.tile(np.sqrt(np.clip(garden, 0, None)) * np.float32(1.25), (4, 1, 1)).astype(np.float32)
    a[:20, :, 2] -= np.float32(2.0)
    a[-20:, :, 0] += np.float32(0.5)
    return np.ascontiguousarray(a)


with Case('app', 'histogram_garden') as case:
    a = case.array('input', frame())
    bins = widgets.histogram_bins(a)
    case.array('bins', np.stack([np.asarray(b, np.int64) for b in bins]))
    shadows, highlights = widgets.clipping_channels(a)
    case.json(shadows=list(shadows), highlights=list(highlights), step=max(1, a.shape[0] // 300))

with Case('app', 'histogram_small') as case:
    a = case.array('input', np.ascontiguousarray(inputs()['patches'].astype(np.float32)))
    bins = widgets.histogram_bins(a)
    case.array('bins', np.stack([np.asarray(b, np.int64) for b in bins]))
    shadows, highlights = widgets.clipping_channels(a)
    case.json(shadows=list(shadows), highlights=list(highlights), step=max(1, a.shape[0] // 300))


# ---- coordinate maps of the canvas tools ---------------------------------------------------------------------

from luma.engine import defaults, normalized  # noqa: E402
from luma.studio import StudioTools  # noqa: E402


class Live:
    """The window's attributes `source_point` reads."""

    def __init__(self, settings, width, height):
        self.settings = settings
        self.load_version = 1
        self.live_source = np.zeros((height, width, 3), np.float32)


class Tools:
    source_point = StudioTools.source_point
    source_points = StudioTools.source_points
    frame_affine = StudioTools.frame_affine
    screen_point = StudioTools.screen_point
    gradient_point = StudioTools.gradient_point

    def __init__(self, settings, width, height):
        self.w = Live(settings, width, height)
        self.coordinate_key = None
        self.coordinates = None
        self.affine_key = None
        self.affine = None


COORDINATE_CASES = {
    'plain': {},
    'rotation_1': {'rotation': 1},
    'rotation_3_flip': {'rotation': 3, 'flip': True},
    'crop': {'crop': [0.1, 0.2, 0.85, 0.9]},
    'straighten': {'straighten': 7.5},
    'straighten_crop_rotation': {'straighten': -4.0, 'crop': [0.05, 0.1, 0.9, 0.8], 'rotation': 2},
    'transform': {'transform_vertical': 25.0, 'transform_rotate': 3.0},
}
FRAME_POINTS = [[0.5, 0.5], [0.0, 0.0], [1.0, 1.0], [0.25, 0.75], [0.9, 0.1], [0.333, 0.667], [0.02, 0.98]]
OFF_POINTS = [[-0.2, 0.5], [1.3, -0.1], [0.5, 1.5]]
SOURCE_POINTS = [[0.5, 0.5], [0.3, 0.4], [0.7, 0.2], [0.11, 0.93], [0.99, 0.01]]

for name, changes in COORDINATE_CASES.items():
    with Case('app', f'coordinates_{name}') as case:
        settings = normalized({**defaults(), **changes})
        tools = Tools(settings, 97, 65)
        source = [tools.source_point(p) for p in FRAME_POINTS]
        forward, backward, frame_size, source_size = tools.frame_affine()
        case.json(
            changes=changes, width=97, height=65, frame_points=FRAME_POINTS, off_points=OFF_POINTS,
            source_points_in=SOURCE_POINTS,
            source=source,
            stroke=tools.source_points(FRAME_POINTS),
            forward=np.asarray(forward).tolist(), backward=np.asarray(backward).tolist(),
            frame_size=list(frame_size), source_size=list(source_size),
            gradient=[[float(v) for v in tools.gradient_point(p)] for p in FRAME_POINTS + OFF_POINTS],
            screen=[[float(v) for v in tools.screen_point(p)] for p in SOURCE_POINTS + OFF_POINTS],
        )


# ---- the healing tool (work package 11) ----------------------------------------------------------------------
# The oracle's own StudioTools methods (build_retouch's controls, mode, preview_stroke, stroke, cancel_stroke,
# apply_healing, remove_healing, clear_healing, update_healing_overlay, remove_retouch, automatic_healing_source,
# set_brush_size, heal_shortcut) and PhotoView's set_tool, cancel_stroke, resize_brush and keyPressEvent run on
# stand-ins for the window, the view and the panel's widgets. The view's mouse handling (press, move, release of a
# tool stroke) is copied from PhotoView.mousePressEvent / mouseMoveEvent / mouseReleaseEvent: the points are
# already in frame fractions. After every step: the tool's state, the settings' `retouch`, the commits and the
# status message; the red coverage overlay's alpha when it is shown.

import types  # noqa: E402

from PySide6.QtCore import Qt  # noqa: E402

from luma.i18n import tr as oracle_tr  # noqa: E402
from luma.panel_keys import switch_on  # noqa: E402
from luma.widgets import PhotoView  # noqa: E402

RETOUCH = ('clone', 'heal', 'inpaint', 'red_eye')


class Emitter:
    def __init__(self, target=None):
        self.target = target

    def emit(self, *values):
        if self.target is not None:
            self.target(*values)


class Quiet:
    def __getattr__(self, name):
        return lambda *a, **k: None


class HealSlider:
    """QSlider: an integer position clamped to its range; `changed` runs when the position changes."""

    def __init__(self, low, high, scale, value):
        self.low, self.high = int(low * scale), int(high * scale)
        self.position = round(value * scale)
        self.changed = None

    def value(self):
        return self.position

    def setValue(self, value):
        value = int(min(self.high, max(self.low, value)))
        if value != self.position:
            self.position = value
            if self.changed is not None:
                self.changed()


class HealAdjustment:
    def __init__(self, low, high, scale, value):
        self.slider = HealSlider(low, high, scale, value)


class HealText:
    def __init__(self, text=''):
        self.value = text
        self.enabled = True

    def setText(self, text):
        self.value = text

    def setEnabled(self, enabled):
        self.enabled = bool(enabled)


class HealCheck:
    def __init__(self):
        self.checked = False
        self.toggled = None

    def isChecked(self):
        return self.checked

    def setChecked(self, value):
        if bool(value) != self.checked:
            self.checked = bool(value)
            if self.toggled is not None:
                self.toggled()


class HealKey:
    def __init__(self, key):
        self.code = key

    def key(self):
        return self.code

    def accept(self):
        pass


class HealView:
    set_tool = PhotoView.set_tool
    cancel_stroke = PhotoView.cancel_stroke
    resize_brush = PhotoView.resize_brush
    keyPressEvent = PhotoView.keyPressEvent

    def __init__(self):
        self.tool_mode = ''
        self.tool_points = []
        self.tool_alt = False
        self.brush_radius = .02
        self.brush_feather = 70
        self.healing_overlay = None
        self.tool_overlay = None
        self.on_screen = None
        self.sample_mode = False
        self.crop_mode = False
        self.handle_drag = None
        self.strokeCancelled = Emitter()
        self.brushSizeChanged = Emitter()
        self.toolModeChanged = Emitter()

    def viewport(self):
        return Quiet()

    def sync_cursor(self):
        pass

    # PhotoView.mousePressEvent / mouseMoveEvent / mouseReleaseEvent, the tool stroke part.
    def press(self, point, alt):
        if self.tool_mode:
            self.tool_points = [list(point)]
            self.tool_alt = bool(alt)
            if self.tool_mode in RETOUCH:
                self.studio.preview_stroke(self.tool_points, self.tool_alt)

    def move(self, point):
        if self.tool_mode and self.tool_points:
            if self.tool_points[-1] != list(point):
                self.tool_points.append(list(point))
            if self.tool_mode in RETOUCH:
                self.studio.preview_stroke(self.tool_points, self.tool_alt)

    def release(self, point):
        if self.tool_mode and self.tool_points:
            if self.tool_points[-1] != list(point):
                self.tool_points.append(list(point))
            points = self.tool_points
            self.tool_points = []
            self.studio.stroke(points, self.tool_alt)


class HealTabs:
    """DevelopPanels.select_tool / open_tool for the Healing tool (index 6)."""

    def __init__(self, window):
        self.w = window
        self.index = 0

    def select_tool(self, index):
        if index == self.index and index in (1, 5, 6):
            index = 0
        self.index = index
        self.w.studio.tab_changed(index)
        if index == 6:
            self.w.studio.mode('heal')

    def open_tool(self, index):
        if self.index != index:
            self.select_tool(index)


class HealWindow:
    """MainWindow's attributes and methods the healing tool uses (app.py set_setting, commit)."""

    def __init__(self, changes, width, height):
        self.settings = normalized({**defaults(), **changes})
        self.last_saved = deepcopy(self.settings)
        self.current_id = 1
        self.load_version = 1
        self.live_source = np.zeros((height, width, 3), np.float32)
        self.source = self.live_source
        self.history = []
        self.messages = []
        self.render_version = 0
        self.closing = False
        self.preview_timer = self.refine_timer = Quiet()
        self.preview_timer.isActive = lambda: True
        self.manager = types.SimpleNamespace(syncing=False)
        self.crop_button = HealCheck()
        self.wb_button = HealCheck()
        self.before_button = HealCheck()
        self.view = HealView()
        self.tabs = HealTabs(self)

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
        self.render_version += 1

    def finish_interaction(self):
        self.commit()

    def load_controls(self):
        pass

    def render(self):
        pass

    def set_mode(self, index):
        pass


class Heal:
    def __init__(self, window):
        self.w = window
        window.studio = window.view.studio = self
        window.view.strokeCancelled.target = self.cancel_stroke
        window.view.brushSizeChanged.target = self.set_brush_size
        window.view.healingApplyRequested = Emitter(self.apply_healing)
        window.view.healingClearRequested = Emitter(self.clear_healing)
        window.before_button.toggled = self.update_healing_overlay
        self.loading = False
        self.photo_id = 1
        self.donor = None
        self.layer_index = 0
        self.coordinate_key = None
        self.coordinates = None
        self.color_pick_generation = 0
        self.pending_stroke = None
        self.stroke_generation = 0
        self.overlay_paused = False
        self.healing_strokes = []
        self.healing_context = None
        self.healing_mask = None
        self.healing_overlay_key = None
        self.healing_generation = 0
        # build_retouch's controls, build_masks' brush size and feather (mode('') reads them).
        self.retouch_radius = HealAdjustment(.002, .12, 1000, .015)
        self.retouch_feather = HealAdjustment(0, 100, 1, 60)
        self.retouch_radius.slider.changed = self.retouch_cursor
        self.retouch_feather.slider.changed = self.retouch_cursor
        self.radius = HealAdjustment(.002, .25, 1000, .025)
        self.feather = HealAdjustment(0, 100, 1, 70)
        self.retouch_source_note = HealText(oracle_tr('힐링 원본: 자동 선택'))
        self.healing_count = HealText()
        self.healing_apply = HealText()
        self.healing_remove = HealText()
        self.healing_clear = HealText()
        self.retouch_count = HealText(oracle_tr('복구 동작 0개'))
        self.update_healing_controls()

    def load_settings(self):
        self.retouch_count.setText(oracle_tr('복구 동작 {0}개', f"{len(self.w.settings['retouch'])}"))


for _name in ('mode', 'tab_changed', 'preview_ready', 'source_point', 'source_points', 'stroke', 'make_retouch',
              'stroke_valid', 'preview_stroke', 'cancel_stroke', 'brush_cursor', 'retouch_cursor', 'set_brush_size',
              'update_healing_controls', 'clear_healing', 'remove_healing', 'apply_healing', 'update_healing_overlay',
              'automatic_healing_source', 'remove_retouch', 'heal_shortcut'):
    setattr(Heal, _name, getattr(StudioTools, _name))

HEAL_KEYS = {'Escape': Qt.Key.Key_Escape, 'Return': Qt.Key.Key_Return, '[': Qt.Key.Key_BracketLeft,
             ']': Qt.Key.Key_BracketRight}


def heal_step(studio, step):
    w = studio.w
    kind, *args = step
    if kind == 'shortcut':
        studio.heal_shortcut()
    elif kind == 'strip':
        w.tabs.select_tool(6)
    elif kind == 'mode':
        studio.mode(args[0])
    elif kind == 'press':
        w.view.press(args[0], args[1])
    elif kind == 'move':
        w.view.move(args[0])
    elif kind == 'release':
        w.view.release(args[0])
    elif kind == 'click':
        w.view.press(args[0], args[1])
        w.view.release(args[0])
    elif kind == 'key':
        w.view.keyPressEvent(HealKey(HEAL_KEYS[args[0]]))
    elif kind == 'button':
        {'apply': studio.apply_healing, 'remove_mark': studio.remove_healing, 'clear_marks': studio.clear_healing,
         'remove_last': studio.remove_retouch, 'automatic': studio.automatic_healing_source,
         'finish': lambda: studio.mode('')}[args[0]]()
    elif kind == 'radius':
        studio.retouch_radius.slider.setValue(args[0])
    elif kind == 'feather':
        studio.retouch_feather.slider.setValue(args[0])
    elif kind == 'before':
        w.before_button.setChecked(args[0])
    else:
        raise ValueError(kind)


def heal_state(studio):
    w, view = studio.w, studio.w.view
    pending = studio.pending_stroke
    return dict(
        mode=view.tool_mode, points=deepcopy(view.tool_points), retouch=deepcopy(w.settings['retouch']),
        staged=deepcopy(studio.healing_strokes),
        pending=deepcopy(pending['component']) if pending is not None else None,
        pending_staged=bool(pending.get('staged')) if pending is not None else False,
        donor=deepcopy(studio.donor), note=studio.retouch_source_note.value,
        waiting=studio.healing_count.value, apply=studio.healing_apply.enabled,
        remove=studio.healing_remove.enabled, clear=studio.healing_clear.enabled,
        radius=studio.retouch_radius.slider.value(), feather=studio.retouch_feather.slider.value(),
        brush_radius=view.brush_radius, brush_feather=view.brush_feather, before=w.before_button.isChecked(),
        history=list(w.history), status=w.messages[-1] if w.messages else None,
        overlay=view.healing_overlay is not None)


def overlay_alpha(image):
    width, height, line = image.width(), image.height(), image.bytesPerLine()
    rows = np.frombuffer(image.constBits(), np.uint8, count=line * height).reshape(height, line)
    return np.ascontiguousarray(rows[:, :width * 4].reshape(height, width, 4)[..., 3])


HEALING_CASES = {
    # Healing brush: staged marks, the overlay, [ ] sizes, Esc during a stroke, remove a mark, Enter applies.
    'heal': ({}, 97, 65, [
        ['shortcut'],
        ['press', [0.3, 0.3], False], ['move', [0.35, 0.32]], ['move', [0.35, 0.32]], ['move', [0.4, 0.35]],
        ['release', [0.42, 0.36]],
        ['click', [0.7, 0.6], False],
        ['key', ']'], ['key', ']'], ['key', '['],
        ['press', [0.2, 0.75], False], ['move', [0.25, 0.8]], ['key', 'Escape'], ['release', [0.3, 0.8]],
        ['press', [0.55, 0.2], False], ['move', [0.6, 0.22]], ['release', [0.6, 0.22]],
        ['before', True], ['before', False],
        ['button', 'remove_mark'],
        ['key', 'Return'],
        ['click', [0.1, 0.1], False],
        ['key', 'Escape'],
        ['key', 'Escape'],
        ['shortcut'],
        ['button', 'remove_last'],
        ['button', 'apply'],
    ]),
    # Clone, inpaint and red eye on a turned and cropped frame; the source by Alt+click; feather and size sliders;
    # a manual healing source; the tool ended by the button and by Esc.
    'clone': ({'rotation': 1, 'crop': [0.1, 0.05, 0.9, 0.95]}, 97, 65, [
        ['strip'],
        ['mode', 'clone'],
        ['click', [0.5, 0.5], False],
        ['click', [0.2, 0.3], True],
        ['feather', 25], ['radius', 40],
        ['press', [0.6, 0.6], False], ['move', [0.65, 0.62]], ['move', [0.7, 0.66]], ['release', [0.7, 0.66]],
        ['press', [0.3, 0.6], False], ['move', [0.35, 0.6]], ['key', 'Escape'], ['release', [0.4, 0.6]],
        ['button', 'automatic'],
        ['mode', 'inpaint'],
        ['press', [0.4, 0.4], False], ['move', [0.45, 0.42]], ['release', [0.45, 0.42]],
        ['mode', 'red_eye'], ['key', '['],
        ['click', [0.8, 0.2], False],
        ['mode', 'heal'],
        ['click', [0.15, 0.85], True],
        ['press', [0.5, 0.8], False], ['move', [0.55, 0.85]], ['release', [0.55, 0.85]],
        ['key', 'Escape'],
        ['key', 'Escape'],
        ['mode', 'heal'],
        ['button', 'remove_last'],
        ['button', 'finish'],
    ]),
    # Straightened: strokes that start on the empty canvas are refused.
    'straighten': ({'straighten': 7.5}, 97, 65, [
        ['shortcut'],
        ['click', [0.0, 0.0], False],
        ['press', [0.5, 0.5], False], ['move', [0.0, 0.02]], ['release', [0.01, 0.0]],
        ['button', 'apply'],
    ]),
}

for name, (changes, width, height, script) in HEALING_CASES.items():
    with Case('app', f'healing_{name}') as case:
        window = HealWindow(changes, width, height)
        studio = Heal(window)
        states = []
        for index, step in enumerate(script):
            heal_step(studio, step)
            states.append(heal_state(studio))
            if window.view.healing_overlay is not None and window.view.tool_mode == 'heal':
                case.array(f'overlay_{index}', overlay_alpha(window.view.healing_overlay))
        case.json(changes=changes, width=width, height=height, script=script, states=states)


# ---- masks: model, panel and gradients (work package 8) -------------------------------------------------------
#
# `StudioTools`' own mask methods run unbound on `MaskStudio`, a stand-in with the widgets they read as plain
# values (the tool options at their defaults, the operation box, the overlay check box) and a window that keeps
# settings and the commit path (`commit` records the label when the settings changed, as `MainWindow.commit`
# does). The view only keeps the tool mode and the handles; `set_tool` reports to `mask_tool_changed` as the
# real view's `toolModeChanged` does. Each step records the `masks` value, the selection and the history labels.

from luma.panel_keys import switch_on  # noqa: E402


class Position:
    def __init__(self, value):
        self.v = value

    def value(self):
        return self.v


class Option:
    """A tool option slider (`Adjustment`): only `slider.value()` is read."""

    def __init__(self, position):
        self.slider = Position(position)


class Choice:
    """The operation / brush range combo boxes."""

    def __init__(self, items, current):
        self.items = items
        self.index = items.index(current)

    def currentData(self):
        return self.items[self.index]

    def findData(self, data):
        return self.items.index(data)

    def setCurrentIndex(self, index):
        self.index = index


class Check:
    def __init__(self, checked):
        self.checked = checked

    def isChecked(self):
        return self.checked


class MaskView:
    def __init__(self, studio):
        self.studio = studio
        self.tool_mode = ''
        self.tool_handles = []
        self.handle_kind = 'linear'
        self.handle_angle = 0.
        self.on_screen = None

    def set_tool(self, mode):
        self.tool_mode = mode
        self.tool_handles = []
        self.studio.mask_tool_changed(mode)

    def viewport(self):
        return Silent()

    def cancel_stroke(self):
        pass


class MaskWindow:
    def __init__(self, settings, width, height):
        self.settings = settings
        self.last_saved = deepcopy(settings)
        self.history = []
        self.load_version = 1
        self.live_source = np.zeros((height, width, 3), np.float32)
        self.source = self.live_source
        self.current_id = 1

    def commit(self, label='보정 변경'):
        if self.settings != self.last_saved:
            self.history.append(label)
            self.last_saved = deepcopy(self.settings)

    def set_setting(self, key, value):
        self.settings[key] = deepcopy(value)
        switch_on(self.settings, (key,))

    def finish_interaction(self):
        self.commit()

    def render(self):
        pass

    def statusBar(self):
        return Silent()


class Silent:
    """Any widget call does nothing."""

    def __getattr__(self, name):
        return lambda *args, **kwargs: None


class MaskStudio:
    for _name in ('source_point', 'source_points', 'frame_affine', 'gradient_point', 'screen_point',
                  'radial_from_screen', 'radial_on_screen', 'gradient_shape', 'component_handles', 'radial_turned',
                  'stroke', 'make_component', 'current_layer', 'new_layer', 'add_component', 'range_component',
                  'ranged', 'remove_component', 'layer_property', 'local_changed', 'remove_layer',
                  'set_layer_enabled', 'set_operation', 'create_mask', 'extend_mask', 'drop_fresh_layer',
                  'mask_tool_changed', 'current_component', 'replace_component', 'duplicate_component',
                  'move_component', 'duplicate_layer', 'move_layer', 'edit_component_handles',
                  'move_component_handles', 'stroke_valid', 'cancel_stroke', 'pause_overlay', 'add_sky'):
        locals()[_name] = getattr(StudioTools, _name)
    brush_range_place = StudioTools.brush_range_place
    BRUSH_RANGES = StudioTools.BRUSH_RANGES
    BRUSH_RANGE_SOFTNESS = StudioTools.BRUSH_RANGE_SOFTNESS

    def __init__(self, settings, width, height):
        self.w = MaskWindow(settings, width, height)
        self.w.view = MaskView(self)
        self.loading = False
        self.range_loading = False
        self.layer_index = 0
        self.component_index = -1
        self.fresh_layer = None
        self.pending_stroke = None
        self.overlay_paused = False
        self.coordinate_key = self.coordinates = self.affine_key = self.affine = None
        self.radius = Option(25)          # 크기 .025 (scale 1000)
        self.feather = Option(70)
        self.flow = Option(100)
        self.density = Option(100)
        self.auto_mask = Check(False)
        self.overlay = Check(True)
        self.operation = Choice(['add', 'subtract', 'intersect'], 'add')
        self.brush_range = Choice([key for key, _, _ in StudioTools.BRUSH_RANGES], 'all')

    def mode(self, kind):
        if kind in ('brush', 'linear', 'radial', 'color_range', 'color_range_add', 'luma_range', 'edit_component'):
            self.overlay_paused = False
        self.w.view.set_tool(kind)

    def select_component(self, index):
        self.overlay_paused = False
        self.w.commit()
        self.component_index = index

    def select_layer(self, index):
        self.overlay_paused = False
        self.w.commit()
        self.layer_index = index
        self.component_index = -1
        self.w.view.set_tool('')

    # Widgets of the panel: nothing to show here.
    def load_layers(self):
        if self.layer_index >= len(self.w.settings['masks']):
            self.layer_index = len(self.w.settings['masks']) - 1

    def load_layer(self):
        pass

    def update_mask_ui(self):
        pass

    def brush_cursor(self):
        pass

    def preview_ready(self, image):
        pass

    def tree_selected(self, layer, component):
        """`tree_selected` on the item (layer, component): a gradient is ready to drag at once."""
        if layer != self.layer_index:
            self.select_layer(layer)
        if component != self.component_index:
            self.select_component(component)
        chosen = self.current_component()
        if chosen is not None and chosen.get('type') in ('linear', 'radial') and self.w.view.tool_mode in ('', 'edit_component'):
            self.edit_component_handles()

    def drag_handle(self, index, start, end):
        """`PhotoView` mouse press / move / release on handle `index` (2: the centre): the handles the view
        sends with `componentEdited`."""
        values = np.array(self.w.view.tool_handles)
        original = values.copy()
        p = np.array(end)
        if index < 2:
            values[index] = p
        else:
            values += p - np.array(start)
        if not np.allclose(values, original):
            self.move_component_handles(values.tolist())


def mask_steps():
    """(name, call) of the scripted session. Points are frame fractions as the view hands them over
    (`free_point` for gradients: off the photo too)."""
    return [
        ('create_linear', lambda m: m.create_mask('linear')),
        ('draw_linear', lambda m: m.stroke([[0.2, 0.3], [0.8, 0.6]], False)),
        ('extend_radial_subtract', lambda m: m.extend_mask('radial', 'subtract')),
        ('draw_radial', lambda m: m.stroke([[0.3, 0.2], [0.7, 0.8]], False)),
        ('extend_radial_intersect', lambda m: m.extend_mask('radial', 'intersect')),
        ('draw_radial_off_photo', lambda m: m.stroke([[-0.1, 0.4], [0.5, 1.2]], False)),
        ('extend_linear_add', lambda m: m.extend_mask('linear', 'add')),
        ('draw_linear_off_photo', lambda m: m.stroke([[1.2, -0.1], [0.4, 0.5]], False)),
        ('draw_linear_click', lambda m: m.stroke([[0.5, 0.5], [0.5, 0.5]], False)),
        ('end_tool', lambda m: m.w.view.set_tool('')),
        ('select_radial', lambda m: m.tree_selected(0, 1)),
        ('drag_radial_corner', lambda m: m.drag_handle(0, m.w.view.tool_handles[0], [0.25, 0.1])),
        ('drag_radial_centre', lambda m: m.drag_handle(2, [0.5, 0.5], [0.55, 0.42])),
        ('turn_radial', lambda m: m.radial_turned('angle', 30.0)),
        ('select_turned_radial', lambda m: m.tree_selected(0, 1)),
        ('drag_turned_corner', lambda m: m.drag_handle(1, m.w.view.tool_handles[1], [0.8, 0.85])),
        ('select_linear', lambda m: m.tree_selected(0, 0)),
        ('drag_linear_end', lambda m: m.drag_handle(1, m.w.view.tool_handles[1], [0.9, 0.2])),
        ('drag_linear_centre', lambda m: m.drag_handle(2, [0.5, 0.5], [0.45, 0.55])),
        ('end_edit', lambda m: m.w.view.set_tool('')),
        ('opacity', lambda m: (m.pause_overlay(), m.layer_property('opacity', 0.5), m.w.commit())),
        ('invert', lambda m: (m.layer_property('invert', True), m.w.commit())),
        ('local_exposure', lambda m: (m.local_changed('exposure', 0.75), m.w.commit())),
        ('local_saturation', lambda m: (m.local_changed('saturation', -40.0), m.w.commit())),
        ('duplicate_layer', lambda m: m.duplicate_layer()),
        ('rename_layer', lambda m: (m.layer_property('name', '하늘 쪽'), m.w.commit())),
        ('move_layer_up', lambda m: m.move_layer(-1)),
        ('move_layer_up_again', lambda m: m.move_layer(-1)),
        ('hide_layer', lambda m: m.set_layer_enabled(0, False)),
        ('select_component_2', lambda m: m.select_component(2)),
        ('duplicate_component', lambda m: m.duplicate_component()),
        ('move_component_up', lambda m: m.move_component(-1)),
        ('move_component_down', lambda m: m.move_component(1)),
        ('remove_component', lambda m: m.remove_component()),
        ('create_radial_then_end', lambda m: m.create_mask('radial')),
        ('drop_fresh', lambda m: m.w.view.set_tool('')),
        ('create_sky', lambda m: m.create_mask('sky')),
        ('extend_sky_subtract', lambda m: m.extend_mask('sky', 'subtract')),
        ('create_brush_mode', lambda m: m.create_mask('brush')),
        ('reuse_fresh', lambda m: m.mask_shortcut_fresh('linear')),
        ('draw_in_fresh', lambda m: m.stroke([[0.1, 0.9], [0.3, 0.7]], False)),
        ('remove_layer', lambda m: m.remove_layer()),
        ('show_layer', lambda m: m.set_layer_enabled(0, True)),
    ]


def mask_shortcut_fresh(self, kind):
    """`mask_shortcut` without opening the tool strip: a fresh empty mask is used again."""
    layers = self.w.settings['masks']
    fresh = self.fresh_layer
    if fresh is not None and fresh == self.layer_index and 0 <= fresh < len(layers) and not layers[fresh].get('components'):
        self.mode(kind)
    else:
        self.create_mask(kind)


MaskStudio.mask_shortcut_fresh = mask_shortcut_fresh

MASK_CASES = {
    'plain': {},
    'rotation_1': {'rotation': 1},
    'straighten_crop': {'straighten': 6.0, 'crop': [0.1, 0.05, 0.9, 0.95]},
}

for name, changes in MASK_CASES.items():
    with Case('app', f'masks_{name}') as case:
        studio = MaskStudio(normalized({**defaults(), **changes}), 97, 65)
        steps = []
        for step, call in mask_steps():
            call(studio)
            steps.append(dict(step=step, masks=deepcopy(studio.w.settings['masks']), layer=studio.layer_index,
                              component=studio.component_index, mode=studio.w.view.tool_mode,
                              handles=[[float(v) for v in p] for p in studio.w.view.tool_handles],
                              angle=float(studio.w.view.handle_angle), history=list(studio.w.history)))
        case.json(changes=changes, width=97, height=65, steps=steps)


# ---- a library built by the oracle ---------------------------------------------------------------------------

from PIL import Image  # noqa: E402
from luma import catalog as catalog_module, library_query, workflow_data  # noqa: E402
from luma.app import MainWindow  # noqa: E402
from luma.catalog import Catalog  # noqa: E402
from luma.engine import thumbnail, to_srgb  # noqa: E402
from luma.manager import EDIT_GROUPS, LibraryManager  # noqa: E402

STAMP = '2026-11-13T01:02:03.456789+00:00'
TOKEN = '0123456789abcdef0123456789abcdef'
ROOT = r'Z:\GrainyGolden'


class Clock(datetime):
    @classmethod
    def now(cls, tz=None):
        value = datetime.fromisoformat(STAMP)
        return value if tz else value.replace(tzinfo=None)


catalog_module.datetime = Clock


class Nop:
    """A widget or timer the commit path touches: every call does nothing and answers falsy."""

    def __getattr__(self, name):
        return Nop()

    def __call__(self, *args, **kwargs):
        return Nop()

    def __bool__(self):
        return False

    def __iter__(self):
        return iter(())


class Manager:
    edit_photo = LibraryManager.edit_photo
    after_commit = LibraryManager.after_commit
    set_label = LibraryManager.set_label
    virtual_copy = LibraryManager.virtual_copy

    def __init__(self, window):
        self.w = window
        self.copy_keys = set(defaults()) - set(EDIT_GROUPS['크롭·회전']) - {'panels_off'}
        self.auto_sync = False
        self.syncing = False

    def refresh_thumbnails(self, ids):
        pass


class Window:
    """The state `MainWindow`'s commit path reads, with its own methods."""

    commit = MainWindow.commit
    save_undo = MainWindow.save_undo
    undo = MainWindow.undo
    redo = MainWindow.redo
    apply_restored = MainWindow.apply_restored
    restore_history = MainWindow.restore_history
    set_setting = MainWindow.set_setting
    set_rating = MainWindow.set_rating
    set_flag = MainWindow.set_flag
    copy_edits = MainWindow.copy_edits
    paste_edits = MainWindow.paste_edits
    reset_edits = MainWindow.reset_edits

    def __init__(self, catalog):
        self.catalog = catalog
        self.current_id = None
        self.settings = defaults()
        self.last_saved = defaults()
        self.undo_stack = []
        self.redo_stack = []
        self.selection = []
        self.copied = None
        self.source = object()
        self.render_version = 0
        self.manager = Manager(self)
        for name in ('commit_timer', 'photo_model', 'view', 'before_button', 'auto_tone_note', 'copy_crop', 'tabs',
                     'preview_timer', 'refine_timer'):
            setattr(self, name, Nop())

    def statusBar(self):
        return Nop()

    def refresh_history(self):
        pass

    def load_controls(self):
        pass

    def refresh_lists(self, *args, **kwargs):
        pass

    def render(self):
        pass

    def selected_ids(self):
        return list(self.selection) or ([self.current_id] if self.current_id else [])

    def activate(self, photo_id):
        """`MainWindow.activate` up to its background load: the stored settings and undo stacks."""
        if photo_id == self.current_id:
            return
        self.commit()
        self.current_id = photo_id
        photo = self.catalog.photo(photo_id)
        self.settings = deepcopy(photo['settings'])
        self.last_saved = deepcopy(self.settings)
        state = self.catalog.preference(f'undo:{photo_id}', {})
        self.undo_stack = [normalized(s) for s in state.get('undo', [])]
        self.redo_stack = [normalized(s) for s in state.get('redo', [])]
        self.selection = [photo_id]

    def history_item(self, index):
        row = self.catalog.histories(self.current_id)[index]

        class Item:
            def data(self, role):
                return row['settings']
        return Item()


def dump(db):
    result = {}
    tables = [r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")]
    for table in tables:
        columns = [r[1] for r in db.execute('PRAGMA table_info(' + table + ')')]
        rows = []
        for row in db.execute('SELECT * FROM ' + table + ' ORDER BY ' + ','.join('"' + c + '"' for c in columns)):
            item = dict(zip(columns, row))
            if table == 'preview_state':
                item['token'] = TOKEN
            item = {k: {'blob': bytes(v).hex()} if isinstance(v, bytes) else v for k, v in item.items()}
            rows.append(item)
        result[table] = rows
    return result


def run_script(window, script):
    """The scripted session: each step names a window action, as the Rust test runs the same steps."""
    c = window.catalog
    for step in script:
        action, a = step[0], step[1:]
        if action == 'open':
            window.activate(a[0])
        elif action == 'set':
            window.set_setting(a[0], a[1])
        elif action == 'commit':
            window.commit(*a)
        elif action == 'undo':
            window.undo()
        elif action == 'redo':
            window.redo()
        elif action == 'select':
            window.selection = list(a[0])
        elif action == 'rating':
            window.set_rating(a[0])
        elif action == 'flag':
            window.set_flag(a[0])
        elif action == 'label':
            window.manager.set_label(a[0])
        elif action == 'copy':
            window.copy_edits()
        elif action == 'paste':
            window.paste_edits()
        elif action == 'restore_history':
            window.restore_history(window.history_item(a[0]))
        elif action == 'virtual_copy':
            window.manager.virtual_copy()
        elif action == 'reset':
            window.reset_edits()
        else:
            raise ValueError(action)
    window.commit()
    return c


PHOTOS = [   # (file, input, gain, format)
    ('photos/garden.jpg', 'garden', 1.0, 'JPEG'),
    ('photos/patches.png', 'patches', 1.0, 'PNG'),
    ('photos/edges.png', 'edges', 1.2, 'PNG'),
    ('photos/roll 2/colours.jpg', 'colours', 0.8, 'JPEG'),
    ('photos/roll 2/ramp.png', 'ramp', 1.0, 'PNG'),
    ('photos/roll 2/noise.jpg', 'noise', 0.6, 'JPEG'),
    ('photos/roll 2/sub/wide.png', 'wide', 0.4, 'PNG'),
]
CAPTURE = {1: '2025:06:01 10:00:00', 2: '2024:01:02 09:00:00', 3: '2025:06:01 08:30:00', 4: '2023:12:24 18:00:00',
           5: '2026:02:14 12:00:00', 6: '2025:01:01 00:00:01', 7: '2024:07:07 07:07:07'}


def build_library(folder):
    """The library: photos imported as `import_next` adds them, then edits, labels, collections, stacks."""
    images = inputs()
    for name, key, gain, kind in PHOTOS:
        path = folder / name
        path.parent.mkdir(parents=True, exist_ok=True)
        pixels = np.uint8(np.clip(to_srgb(images[key] * np.float32(gain)), 0, 1) * 255 + .5)
        Image.fromarray(pixels).save(path, kind, **({'quality': 92} if kind == 'JPEG' else {}))
    c = Catalog(folder / 'Library')
    c.register_folder(str((folder / 'photos').resolve()))
    for name, *_ in sorted(PHOTOS, key=lambda p: str(folder / p[0])):
        path = str(folder / name)
        image, info = thumbnail(path, with_info=True)
        c.add(path, settings=None, metadata=info)
    ids = [p['id'] for p in c.photos()]
    for i in ids:
        c.set_user_metadata(i, {'DateTimeOriginal': CAPTURE[i]})
    w = Window(c)
    # Edits through the window: two photos with history and undo stacks.
    w.activate(1)
    w.set_setting('exposure', 0.4); w.commit()
    w.set_setting('contrast', 15.0); w.commit()
    w.set_setting('vignette', -20.0); w.commit()
    w.undo()
    w.activate(3)
    w.set_setting('monochrome', True); w.commit()
    w.set_setting('film_look', 'kodak-portra-400'); w.commit('필름')
    w.set_setting('temperature', 12.0); w.commit()
    w.activate(5)
    w.set_setting('exposure', -0.7); w.commit()
    w.commit()
    c.update_many([1, 2, 4], 'rating', 4)
    c.update_many([3], 'rating', 2)
    c.update_many([5, 6], 'rating', 5)
    c.update_many([1, 5], 'flag', 1)
    c.update_many([7], 'flag', -1)
    c.update_many([2, 3], 'label', '빨강')
    c.update_many([6], 'label', '파랑')
    c.update(4, keywords='Sunset, 바다')
    c.update(2, keywords='studio')
    plain = c.add_collection('여행', None, None)
    c.collection_add(plain, [2, 4, 6])
    c.add_collection('좋은 사진', {'rating_min': 4}, None)
    c.stack([4, 5, 6])
    workflow_data.stack_order(c, 6, 0)
    c.virtual_copy(1)
    c.db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
    return c


def move_paths(db, old, new):
    """Moves every stored path from `old` to `new` (the photos, the folders, the metadata's fingerprints)."""
    for ident, path, metadata in db.execute('SELECT id,path,metadata FROM photos').fetchall():
        info = json.loads(metadata)
        fingerprint = info.get('file_fingerprint')
        if isinstance(fingerprint, dict) and fingerprint.get('path', '').startswith(old):
            fingerprint['path'] = new + fingerprint['path'][len(old):]
        db.execute('UPDATE photos SET path=?,metadata=? WHERE id=?', (new + path[len(old):], json.dumps(info), ident))
    for (path,) in db.execute('SELECT path FROM folder_roots').fetchall():
        db.execute('UPDATE folder_roots SET path=? WHERE path=?', (new + path[len(old):], path))
    db.commit()


LISTINGS = [   # (name, options as refresh_lists builds them, without stack_orders)
    ('all_imported', {}),
    ('all_descending', {'descending': True}),
    ('by_name', {'sort': 'name'}),
    ('by_name_descending', {'sort': 'name', 'descending': True}),
    ('by_capture', {'sort': 'capture'}),
    ('by_rating', {'sort': 'rating'}),
    ('by_rating_descending', {'sort': 'rating', 'descending': True}),
    ('picked', {'mode': 'picked'}),
    ('rated', {'mode': 'rated'}),
    ('rejected', {'mode': 'rejected'}),
    ('search_name', {'search': 'GARDEN'}),
    ('search_keyword', {'search': '바다'}),
    ('folder_recursive', {'folder': ROOT + r'\photos\roll 2'}),
    ('folder_direct', {'folder': ROOT + r'\photos\roll 2', 'recursive': False}),
    ('collection', {'collection': 1}),
    ('smart_collection', {'collection': 2}),
    ('stacks_collapsed', {'collapsed': True}),
    ('stacks_collapsed_by_name', {'collapsed': True, 'sort': 'name'}),
    ('rules_label', {'rules': {'label': '빨강'}}),
    ('rules_virtual', {'rules': {'virtual': True}}),
    ('rules_extension_rating', {'rules': {'extension': 'png', 'rating_max': 3}}),
    ('picked_by_capture', {'mode': 'picked', 'sort': 'capture', 'descending': True}),
]

SCRIPT = [
    ['open', 1],
    ['set', 'exposure', 0.5], ['commit'],
    ['set', 'contrast', 20.0], ['commit'],
    ['set', 'monochrome', True], ['commit'],
    ['undo'], ['undo'], ['redo'],
    ['set', 'vignette', -30.0], ['commit'],
    ['rating', 3], ['flag', -1], ['label', '초록'],
    ['open', 3],
    ['undo'],
    ['set', 'exposure', -1.0], ['commit'],
    ['copy'],
    ['open', 2],
    ['select', [2, 4]],
    ['paste'],
    ['open', 5],
    ['undo'], ['undo'], ['redo'],
    ['open', 3],
    ['restore_history', 1],
    ['select', [3, 7]],
    ['virtual_copy'],
    ['rating', 1],
    ['open', 6],
    ['reset'],
    ['open', 1],
    ['set', 'highlights', -25.0], ['set', 'shadows', 30.0], ['commit'],
]


def listing_options(options, stack_orders):
    """The options `refresh_lists` passes to `library_query.query`."""
    folder = options.get('folder')
    include = options.get('recursive', True)
    return dict(search=options.get('search', ''), folder=folder,
                recursive=include or bool(folder and PureWindowsPath(folder) == PureWindowsPath(folder).parent),
                mode=options.get('mode', 'all'), rules=options.get('rules', {}), collection=options.get('collection'),
                collapsed=options.get('collapsed', False), stack_orders=stack_orders,
                sort=options.get('sort', 'imported'), descending=options.get('descending', False))


with tempfile.TemporaryDirectory(prefix='grainy-app-golden-', ignore_cleanup_errors=True) as temporary:
    work = Path(temporary).resolve()
    c = build_library(work)
    c.close()
    out = GOLDEN / 'app' / 'library'
    out.mkdir(parents=True, exist_ok=True)
    start = out / 'start.sqlite'
    start.unlink(missing_ok=True)
    shutil.copyfile(work / 'Library' / 'catalog.sqlite', start)
    db = sqlite3.connect(start)
    move_paths(db, str(work), ROOT)
    db.execute('PRAGMA journal_mode=DELETE')
    db.commit()
    db.execute('VACUUM')   # drop the stale pages that still hold this machine's temporary paths
    db.close()

    # Listings, settings and history of the library as built.
    query_copy = work / 'query.sqlite'
    shutil.copyfile(start, query_copy)
    snapshot = work / 'snapshot'
    snapshot.mkdir()
    shutil.copyfile(start, snapshot / 'catalog.sqlite')
    c = Catalog(snapshot)
    stack_orders = c.preference('stack_orders', {})
    listings = []
    for name, options in LISTINGS:
        full = listing_options(options, stack_orders)
        result = library_query.query(query_copy, full, threading.Event())
        listings.append(dict(name=name, options=options, ids=result['ids'], total=result['total'],
                             roots=result['roots'], folders=result['folders']))
    photos = {}
    for p in c.photos():
        ident = p['id']
        photos[str(ident)] = dict(
            settings=c.photo(ident)['settings'],
            history=[(row['label'] if row['label'].startswith('Lightroom 상태 · ') else f"{row['label']} 이전")
                     for row in c.histories(ident)],
            undo=c.preference(f'undo:{ident}', {}),
            name=p['name'], rating=p['rating'], flag=p['flag'], label=p['label'],
            virtual_source=p['virtual_source'], stack_id=p['stack_id'])
    c.close()

    # The scripted session through the window's own methods.
    session = work / 'session'
    session.mkdir()
    shutil.copyfile(start, session / 'catalog.sqlite')
    c = Catalog(session)
    run_script(Window(c), SCRIPT)
    c.db.commit()
    c.close()
    # The browser index is a cache every listing brings up to date: brought up to date here as well.
    db = library_query.connect(session / 'catalog.sqlite')
    library_query.sync_index(db, threading.Event())
    final = dump(db)
    db.close()

    db = sqlite3.connect(start)
    start_tables = dump(db)
    db.close()
    with Case('app', 'library') as case:
        case.json(stamp=STAMP, token=TOKEN, root=ROOT, listings=listings, photos=photos, script=SCRIPT,
                  final_tables=final, start_tables=start_tables)


# ---- presets: save, apply, import, export and delete through the oracle's own methods -------------------------------

from luma import app as app_module, library as library_module, manager as manager_module  # noqa: E402

PRESET_FILES = {
    # Grainy presets
    'json look.json': json.dumps({'name': 'Json look', 'settings': {'exposure': 0.3, 'contrast': 12, 'temperature': 5}}),
    'bare.json': json.dumps({'edits': {'saturation': -20, 'grain': 10}}),
    # not presets
    'no settings.json': json.dumps({'hello': 1}),
    'empty settings.json': json.dumps({'settings': {}}),
    'array.json': json.dumps([1, 2]),
    # Adobe presets
    'adobe look.xmp': (
        '<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>\n<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF '
        'xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" '
        'xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" xmlns:xmp="http://ns.adobe.com/xap/1.0/" '
        'crs:Exposure2012="+0.50" crs:Contrast2012="+10" crs:Highlights2012="-30" crs:Saturation="+5" '
        'crs:WhiteBalance="As Shot" crs:Temperature="5200" crs:Tint="3" crs:HasCrop="True" crs:CropLeft="0.1" '
        'crs:CropTop="0.1" crs:CropRight="0.9" crs:CropBottom="0.9" crs:CropAngle="0" crs:Foo="1"/></rdf:RDF>'
        '</x:xmpmeta>\n<?xpacket end="w"?>'),
    'warm film.lrtemplate': (
        's = { id = "x", title = "T", type = "Develop", version = 0, value = { settings = { Exposure2012 = 0.25, '
        'Contrast2012 = 15, Highlights2012 = -20, Saturation = -8, Dehaze = 12, GrainAmount = 20 } } }'),
    'junk.xmp': (
        '<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">'
        '<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Foo="1"/>'
        '</rdf:RDF></x:xmpmeta>'),
}


def mask_temp(message):
    """Replace the machine's temporary folder (repr-escaped or not) with a fixed `<tmp>` placeholder."""
    return re.sub(r"[A-Za-z]:(?:\\\\|\\|/)[^']*?grainy-app-golden-[a-z0-9_]+", '<tmp>', message)


PRESET_SCRIPT = [
    ['open', 1], ['apply', 'Amber film'],
    ['open', 3], ['apply', 'Quiet blue'],
    ['open', 2], ['set', 'crop', [0.1, 0.1, 0.9, 0.8]], ['set', 'rotation', 1], ['commit'],
    ['set', 'temperature', 9.0], ['set', 'exposure', 0.25], ['save', 'My look'],
    ['open', 4], ['set', 'crop', [0.2, 0.2, 0.7, 0.7]], ['set', 'flip', True], ['commit'], ['apply', 'My look'],
    ['import', 'json look.json'], ['import', 'bare.json'], ['import', 'grainy sidecar.xmp'],
    ['import', 'adobe look.xmp', True], ['import', 'adobe look.xmp', True],
    ['import', 'warm film.lrtemplate', False], ['import', 'warm film.lrtemplate', True],
    ['import', 'no settings.json'], ['import', 'empty settings.json'], ['import', 'array.json'],
    ['import', 'junk.xmp'], ['import', 'missing.json'],
    ['open', 5], ['apply', 'adobe look'], ['apply', 'warm film'], ['apply', 'Json look'],
    ['open', 6], ['apply', 'Silver / B&W'], ['apply', '원본의 색'],
    ['export', 'preset.luma.json'],
    ['delete', 'My look'], ['delete', 'bare'],
    ['save', '  Spaces  '], ['save', '   '], ['save', 'Json look'],
    ['open', 2], ['apply', 'Spaces'], ['undo'], ['redo'],
]


class Answers:
    """What the Qt boxes answer in the scripted session (set by each step)."""
    text = ''
    item = ''
    accept = True
    export_path = ''
    open_path = ''
    review = None


class FakeInput:
    @staticmethod
    def getText(parent, title, label):
        return Answers.text, True

    @staticmethod
    def getItem(parent, title, label, items, editable=False):
        if Answers.item not in items:
            raise AssertionError('the script deletes a preset that does not exist')
        return Answers.item, True


class FakeFileDialog:
    @staticmethod
    def getOpenFileName(parent, title, directory, filter_):
        return Answers.open_path, ''

    @staticmethod
    def getSaveFileName(parent, title, name, filter_):
        return Answers.export_path, ''


class FakeButton:
    def __init__(self, box, which):
        self.box, self.which = box, which

    def setText(self, text):
        self.box.labels[self.which] = text


class FakeBox:
    class StandardButton:
        Save = 1
        Cancel = 2

    def __init__(self, parent=None):
        self.labels = {}
        self.record = {}

    def setWindowTitle(self, text):
        self.record['title'] = text

    def setTextFormat(self, kind):
        pass

    def setText(self, text):
        self.record['text'] = text

    def setDetailedText(self, text):
        self.record['details'] = text

    def setStandardButtons(self, buttons):
        pass

    def button(self, which):
        return FakeButton(self, which)

    def exec(self):
        self.record['labels'] = dict(self.labels)
        Answers.review = self.record
        return self.StandardButton.Save if Answers.accept else self.StandardButton.Cancel


class PresetTree:
    """The preset tree stand-in: `save_preset` opens the last top-level folder."""

    def topLevelItemCount(self):
        return 1

    def topLevelItem(self, index):
        return Nop()

    def __getattr__(self, name):
        return Nop()


class PresetWindow(Window):
    apply_preset = MainWindow.apply_preset
    save_preset = MainWindow.save_preset

    def __init__(self, catalog):
        super().__init__(catalog)
        self.preset_tree = PresetTree()
        self.errors = []
        self.refresh_presets()

    def refresh_presets(self):
        """The first line of `MainWindow.refresh_presets` (the rest draws the Qt tree)."""
        from luma.engine import PRESETS
        self.all_presets = {**{name: normalized({'tone_version': defaults()['tone_version'], **values})
                               for name, values in PRESETS.items()}, **self.catalog.presets()}

    def show_error(self, message):
        self.errors.append(message)


class PresetManager:
    import_preset = LibraryManager.import_preset
    export_preset = LibraryManager.export_preset
    delete_preset = LibraryManager.delete_preset

    def __init__(self, window):
        self.w = window


def preset_session(work, files):
    """The scripted preset session on a copy of the library; returns the folder and a log per step."""
    session = work / 'preset-session'
    session.mkdir()
    shutil.copyfile(GOLDEN / 'app' / 'library' / 'start.sqlite', session / 'catalog.sqlite')
    c = Catalog(session)
    window = PresetWindow(c)
    manager = PresetManager(window)
    built_in = [[name, value] for name, value in window.all_presets.items()]
    log = []
    app_module.QInputDialog = FakeInput
    manager_module.QInputDialog = FakeInput
    manager_module.QFileDialog = FakeFileDialog
    manager_module.QMessageBox = FakeBox
    exported = work / 'exported'
    exported.mkdir()
    for step in PRESET_SCRIPT:
        action, a = step[0], step[1:]
        entry = {'step': step}
        Answers.review = None
        try:
            if action == 'open':
                window.activate(a[0])
            elif action == 'set':
                window.set_setting(a[0], a[1])
            elif action == 'commit':
                window.commit(*a)
            elif action == 'undo':
                window.undo()
            elif action == 'redo':
                window.redo()
            elif action == 'apply':
                window.apply_preset(a[0])
            elif action == 'save':
                Answers.text = a[0]
                window.save_preset()
            elif action == 'import':
                Answers.accept = a[1] if len(a) > 1 else True
                Answers.open_path = str(files / a[0])
                manager.import_preset()
            elif action == 'export':
                Answers.export_path = str(exported / a[0])
                manager.export_preset()
                entry['exported'] = json.loads((exported / a[0]).read_text(encoding='utf-8'))
            elif action == 'delete':
                Answers.item = a[0]
                manager.delete_preset()
            else:
                raise ValueError(action)
        except Exception as error:   # the oracle's exception is the expected value
            entry['error'] = {'type': type(error).__name__, 'message': mask_temp(str(error))}
        if window.errors:
            entry['shown_errors'] = list(window.errors)
            window.errors.clear()
        if Answers.review is not None:
            entry['review'] = Answers.review
        entry['presets'] = list(c.presets())
        log.append(entry)
    window.commit()
    c.db.commit()
    c.close()
    return session, log, built_in


with tempfile.TemporaryDirectory(prefix='grainy-app-golden-', ignore_cleanup_errors=True) as temporary:
    work = Path(temporary).resolve()
    files = work / 'files'
    files.mkdir()
    for name, content in PRESET_FILES.items():
        (files / name).write_text(content, encoding='utf-8', newline='\n')
    # A Grainy sidecar with settings (`write_sidecar`), as Save XMP writes it.
    library_module.write_sidecar(
        {'rating': 3, 'label': '', 'keywords': '', 'user_metadata': {},
         'settings': normalized({'exposure': 0.4, 'vibrance': 18, 'monochrome': True})}, files / 'grainy sidecar.xmp')
    session, log, built_in = preset_session(work, files)
    db = library_query.connect(session / 'catalog.sqlite')
    library_query.sync_index(db, threading.Event())
    tables = dump(db)
    db.close()
    out = GOLDEN / 'app' / 'presets'
    shutil.rmtree(out / 'files', ignore_errors=True)
    (out / 'files').mkdir(parents=True, exist_ok=True)
    names = list(PRESET_FILES) + ['grainy sidecar.xmp']
    for name in names:
        shutil.copyfile(files / name, out / 'files' / name)
    keep = ['presets', 'photos', 'history', 'preferences']
    kept = {k: v for k, v in tables.items() if k in keep}
    # The file fingerprints in `metadata` carry the generating run's file times: not compared.
    kept['photos'] = [{k: v for k, v in row.items() if k != 'metadata'} for row in kept['photos']]
    with Case('app', 'presets') as case:
        case.json(stamp=STAMP, token=TOKEN, root=ROOT, script=PRESET_SCRIPT, files=names, log=log, built_in=built_in,
                  tables=kept, table_names=sorted(tables))


# ---- work package 3: the tone curve editor and the targeted adjustment ---------------------------------------
#
# `golden/app/curve_editor`: scripted pointer events sent to the oracle's own `ToneCurve` widget (offscreen Qt,
# 260 x 165 like a narrow panel), with the point list, the `changed` emissions and the `committed` count after
# each step. `golden/app/curve_targeted`: luma/targeted.py `sample`, `curve_input`, `drag_curve` on a quantized
# display picture, and a session through `TargetedAdjust` (`toggle`, `started`, `dragged`, `finished`) on a
# stand-in window recording the window's commits.

from PySide6.QtCore import QEvent, QPointF, Qt  # noqa: E402
from PySide6.QtGui import QMouseEvent  # noqa: E402
from PySide6.QtWidgets import QApplication  # noqa: E402
from luma import targeted  # noqa: E402
from luma.hdr import curve_extended  # noqa: E402
from luma.engine import to_srgb as _to_srgb  # noqa: E402
from luma.panel_keys import switch_on  # noqa: E402

_qt = QApplication.instance() or QApplication([])

CURVE_SIZE = (260, 165)
_BUTTONS = {'left': Qt.MouseButton.LeftButton, 'right': Qt.MouseButton.RightButton}
_TYPES = {'press': QEvent.Type.MouseButtonPress, 'move': QEvent.Type.MouseMove,
          'release': QEvent.Type.MouseButtonRelease, 'double': QEvent.Type.MouseButtonDblClick}

# (name, hdr, starting points or None for the widget's own, steps [kind, button, x, y, ctrl])
CURVE_SCRIPTS = [
    ('drag_middle', False, None, [
        ['press', 'left', 131, 83, False], ['move', 'left', 140, 60, False], ['move', 'left', 175.5, 33.25, False],
        ['release', 'left', 175.5, 33.25, False]]),
    ('drag_past_neighbours', False, None, [
        ['press', 'left', 72, 118, False], ['move', 'left', 300, -20, False], ['move', 'left', -40, 400, False],
        ['release', 'left', -40, 400, False]]),
    ('endpoints_vertical', False, None, [
        ['press', 'left', 12, 153, False], ['move', 'left', 90, 120, False], ['release', 'left', 90, 120, False],
        ['press', 'left', 247, 13, False], ['move', 'left', 100, 40.7, False], ['release', 'left', 100, 40.7, False]]),
    ('ctrl_add_drag', False, None, [
        ['press', 'left', 50, 40, True], ['move', 'left', 52, 30, False], ['release', 'left', 52, 30, False],
        ['press', 'left', 230, 140, True], ['release', 'left', 230, 140, False],
        ['press', 'left', 3, 20, True], ['release', 'left', 3, 20, False],
        ['press', 'left', 259, 164, True], ['release', 'left', 259, 164, False]]),
    ('plain_click_far', False, None, [
        ['press', 'left', 50, 40, False], ['move', 'left', 60, 50, False], ['release', 'left', 60, 50, False]]),
    ('right_delete', False, [[0.0, 0.05], [0.2, 0.3], [0.45, 0.4], [0.7, 0.8], [1.0, 0.95]], [
        ['press', 'right', 60, 115, False], ['release', 'right', 60, 115, False],
        ['press', 'right', 12, 146, False], ['release', 'right', 12, 146, False],
        ['press', 'right', 200, 100, False], ['release', 'right', 200, 100, False],
        ['press', 'right', 118, 98, False], ['release', 'right', 118, 98, False]]),
    ('double_click_reset', False, [[0.0, 0.1], [0.3, 0.2], [1.0, 0.9]], [
        ['press', 'left', 83, 125, False], ['move', 'left', 83, 100, False], ['release', 'left', 83, 100, False],
        ['double', 'left', 83, 100, False], ['release', 'left', 83, 100, False]]),
    ('two_points', False, [[0.0, 0.0], [1.0, 1.0]], [
        ['press', 'left', 130, 80, True], ['move', 'left', 133, 70, False], ['release', 'left', 133, 70, False],
        ['press', 'right', 133, 70, False], ['release', 'right', 133, 70, False],
        ['press', 'right', 12, 153, False], ['release', 'right', 12, 153, False]]),
    ('hdr_drag_and_add', True, [[0.0, 0.0], [0.5, 0.5], [1.0, 1.0], [2.0, 2.0]], [
        ['press', 'left', 71, 118, False], ['move', 'left', 80, 90, False], ['release', 'left', 80, 90, False],
        ['press', 'left', 200, 40, True], ['move', 'left', 215, 30, False], ['release', 'left', 215, 30, False],
        ['press', 'left', 245, 15, False], ['move', 'left', 230, 20, False], ['release', 'left', 230, 20, False],
        ['press', 'right', 215, 30, False], ['release', 'right', 215, 30, False]]),
]


def _send(widget, step):
    kind, button, x, y, ctrl = step
    qt_button = _BUTTONS[button]
    buttons = Qt.MouseButton.NoButton if kind == 'release' else qt_button
    if kind == 'move':
        qt_button = Qt.MouseButton.NoButton
    modifiers = Qt.KeyboardModifier.ControlModifier if ctrl else Qt.KeyboardModifier.NoModifier
    event = QMouseEvent(_TYPES[kind], QPointF(x, y), QPointF(x, y), qt_button, buttons, modifiers)
    {'press': widget.mousePressEvent, 'move': widget.mouseMoveEvent, 'release': widget.mouseReleaseEvent,
     'double': widget.mouseDoubleClickEvent}[kind](event)


with Case('app', 'curve_editor') as case:
    scripts = []
    for name, hdr, start, steps in CURVE_SCRIPTS:
        widget = widgets.ToneCurve()
        widget.resize(*CURVE_SIZE)
        widget.set_hdr(hdr)
        if start is not None:
            widget.set_points(start)
        emitted, committed = [], [0]
        widget.changed.connect(lambda points, emitted=emitted: emitted.append([list(p) for p in points]))
        widget.committed.connect(lambda committed=committed: committed.__setitem__(0, committed[0] + 1))
        initial = [list(p) for p in widget.points]
        results = []
        for step in steps:
            before = len(emitted)
            _send(widget, step)
            results.append(dict(points=[list(p) for p in widget.points], changed=emitted[before:],
                                committed=committed[0], active=widget.active))
        drawn_x = [widget.axis_value(float(v)) for v in np.linspace(0, 1, 100)]
        drawn_y = curve_extended(np.array(drawn_x, np.float32), widget.points)   # what paintEvent draws
        scripts.append(dict(name=name, hdr=hdr, initial=initial, steps=steps, results=results,
                            axis_samples=drawn_x, drawn=drawn_y))
    case.json(width=CURVE_SIZE[0], height=CURVE_SIZE[1], scripts=scripts)


def _display_picture():
    """A display-referred picture as the view holds it, quantized to 8 bits (the Rust frame is 8-bit)."""
    a = np.clip(_to_srgb(inputs()['colours'][:24, :36]), 0, 1)
    return (np.round(a * 255) / 255).astype(np.float32)


class _NoOp:
    def __getattr__(self, name):
        return self

    def __call__(self, *args, **kwargs):
        return self


class _View(_NoOp):
    def __init__(self, picture):
        self.on_screen = picture
        self.crop_mode = False
        self.sample_mode = False


class _TargetWindow(_NoOp):
    """The attributes of `MainWindow` that `TargetedAdjust` reads; commits are recorded."""

    def __init__(self, picture, settings):
        self.view = _View(picture)
        self.source = picture
        self.settings = settings
        self.last_saved = deepcopy(settings)
        self.commits = []
        self.curve = _NoOp()
        self.wb_button = _NoOp()

    def commit(self, label='보정 변경'):
        if self.settings != self.last_saved:
            self.commits.append(dict(label=label, curve=deepcopy(self.settings['curve'])))
            self.last_saved = deepcopy(self.settings)

    def set_setting(self, key, value):
        self.settings[key] = deepcopy(value)
        switch_on(self.settings, (key,))


TARGET_POINTS = [[0.5, 0.5], [0.0, 0.0], [1.0, 1.0], [0.1, 0.9], [0.73, 0.21], [0.999, 0.5]]
TARGET_CURVES = [
    [[0.0, 0.0], [0.25, 0.25], [0.5, 0.5], [0.75, 0.75], [1.0, 1.0]],
    [[0.0, 0.1], [0.3, 0.2], [0.6, 0.75], [1.0, 0.9]],
    [[0.0, 0.0], [0.4, 0.6], [0.5, 0.5], [1.0, 1.0]],        # falling outputs: curve_input keeps the luminance
    [[0.0, 0.0], [1.0, 1.0]],
]
DRAG_CASES = [   # (curve index, x, delta)
    (0, 0.5, 0.1), (0, 0.52, -0.2), (0, 0.6, 0.05), (0, 0.0, -0.3), (0, 1.0, 0.4), (1, 0.31, 2.0),
    (1, 0.45, -0.01), (2, 0.42, 0.0), (3, 0.5, 0.25), (3, 0.97, -0.5),
]
TARGET_SESSION = [   # ('toggle', checked) | ('started', x, y) | ('dragged', pixels) | ('finished',)
    ('toggle', True), ('started', 0.3, 0.6), ('dragged', 12.0), ('dragged', 45.0), ('finished',),
    ('started', 0.82, 0.15), ('dragged', -30.0), ('dragged', -90.0), ('finished',),
    ('started', 0.3, 0.6), ('dragged', 0.0), ('finished',),
    ('toggle', False), ('started', 0.5, 0.5), ('dragged', 50.0), ('finished',),
]

with Case('app', 'curve_targeted') as case:
    picture = case.array('picture', _display_picture())
    samples = [targeted.sample(picture, x, y).tolist() for x, y in TARGET_POINTS]
    inputs_ = [[targeted.curve_input(curve, np.asarray(rgb, np.float32)) for rgb in samples]
               for curve in TARGET_CURVES]
    drags = [targeted.drag_curve(TARGET_CURVES[i], x, delta) for i, x, delta in DRAG_CASES]
    window = _TargetWindow(picture, normalized(defaults()))
    adjust = targeted.TargetedAdjust(window)
    adjust.buttons = {'curve': _NoOp(), 'hsl': _NoOp()}
    trace = []
    for step in TARGET_SESSION:
        if step[0] == 'toggle':
            adjust.toggle('curve', step[1])
        elif step[0] == 'started':
            adjust.started(step[1], step[2])
        elif step[0] == 'dragged':
            adjust.dragged(step[1])
        else:
            adjust.finished()
        trace.append(dict(curve=deepcopy(window.settings['curve']), commits=len(window.commits)))
    case.json(points=TARGET_POINTS, samples=samples, curves=TARGET_CURVES, curve_inputs=inputs_,
              drag_cases=[list(c) for c in DRAG_CASES], drags=drags, session=[list(s) for s in TARGET_SESSION],
              trace=trace, commits=window.commits, per_pixel=targeted.CURVE_PER_PIXEL)


# ---- the crop tool (work package 2) ----------------------------------------------------------------------------
#
# The crop frame of luma/widgets.py `PhotoView` (`set_crop_mode`, `hit_handle`, `drag_crop`, `resize_crop`,
# `limited_crop`, `set_aspect`, `dragged_angle`, the mouse handlers, Enter/Esc) and the window's crop functions of
# luma/app.py (`toggle_crop`, `apply_crop`, `cancel_crop`, `end_crop`, `crop_changed`, `set_straighten`,
# `drag_straighten`, `fit_crop`, `sync_crop_limit`, `reset_crop`, `rotate_photo`, `preview_size`), run unbound on
# stand-ins. The view maps screen points to its scene (logical pixels of the shown frame) by a scale and an offset;
# `mapFromScene` rounds to whole points as Qt's does. The two job results (`auto_straighten`, `auto_border_crop`)
# are their `finished` closures with a given angle or crop. Each step records the frame, the settings and the
# commits; the Rust test (tests/crop.rs) runs the same steps through the crop tool.

from PySide6.QtCore import QPoint, QPointF, QRectF, Qt  # noqa: E402
from luma.panel_keys import switch_on  # noqa: E402
from luma.widgets import PhotoView  # noqa: E402
from luma import straighten as straighten_module  # noqa: E402


def qround(d):
    """Qt's qRound."""
    return int(d + 0.5) if d >= 0 else int(d - int(d - 1) + 0.5) + int(d - 1)


class Emitter:
    def __init__(self, handler=None):
        self.handler = handler

    def emit(self, *args):
        if self.handler:
            self.handler(*args)


class Pointer:
    def __init__(self, x, y, key=None):
        self.p = QPointF(x, y)
        self.k = key

    def position(self):
        return self.p

    def button(self):
        return Qt.MouseButton.LeftButton

    def buttons(self):
        return Qt.MouseButton.LeftButton

    def modifiers(self):
        return Qt.KeyboardModifier.NoModifier

    def key(self):
        return self.k

    def accept(self):
        pass


class CropView:
    tool_mode = ''
    sample_mode = False
    handle_drag = None
    tool_points = []
    sample_press = None

    def __init__(self):
        self.crop_mode = False
        self.crop_limit = (0., 1., 1.)
        self.canvas = None
        self.straighten_angle = 0.
        self.image_rect = QRectF()
        self.crop_rect = QRectF(0, 0, 1, 1)
        self.drag_handle = None
        self.drag_start = None
        self.drag_rect = None
        self.crop_aspect_pending = False
        self.aspect = 0.
        self.has_photo = True
        self.zoom_press = None
        self.scale, self.offset = 1., (0., 0.)
        for name in ('cropSelected', 'cropChanged', 'cropCancelled', 'straightenDragged', 'straightenFinished'):
            setattr(self, name, Emitter())

    def mapToScene(self, p):
        return QPointF((p.x() - self.offset[0]) / self.scale, (p.y() - self.offset[1]) / self.scale)

    def mapFromScene(self, p):
        return QPoint(qround(p.x() * self.scale + self.offset[0]), qround(p.y() * self.scale + self.offset[1]))

    def viewport(self):
        return Nop()

    def setFocus(self):
        pass

    def set_tool(self, mode):
        pass

    def sync_cursor(self):
        pass

    def fit_photo(self):
        pass

    def crop_cursor(self, handle):
        return None


for _name in ('set_crop_mode', 'crop_values', 'canvas_rect', 'set_crop_limit', 'pointer_angle', 'dragged_angle',
              'drag_crop', 'show_crop', 'limited_crop', 'crop_screen_rect', 'handle_points', 'hit_handle',
              'normalized_point', 'normalized_aspect', 'set_aspect', 'reset_crop', 'accept_crop', 'resize_crop',
              'mousePressEvent', 'mouseMoveEvent', 'mouseReleaseEvent', 'mouseDoubleClickEvent', 'keyPressEvent'):
    setattr(CropView, _name, PhotoView.__dict__[_name])

ASPECTS = [0, 1, 1.5, 4 / 3, 16 / 9, 2 / 3, .8, 9 / 16, 65 / 24]


class AspectBox:
    def __init__(self, window):
        self.w, self.index = window, 0

    def currentData(self):
        return ASPECTS[self.index]

    def setCurrentIndex(self, index):
        if index != self.index:
            self.index = index
            self.w.set_aspect()


class CropWindow:
    toggle_crop = MainWindow.toggle_crop
    sync_crop_limit = MainWindow.sync_crop_limit
    start_crop = MainWindow.start_crop
    fit_crop = MainWindow.fit_crop
    set_straighten = MainWindow.set_straighten
    drag_straighten = MainWindow.drag_straighten
    set_aspect = MainWindow.set_aspect
    end_crop = MainWindow.end_crop
    cancel_crop = MainWindow.cancel_crop
    reset_crop = MainWindow.reset_crop
    apply_crop = MainWindow.apply_crop
    rotate_photo = MainWindow.rotate_photo
    preview_size = MainWindow.preview_size

    def __init__(self, settings, original):
        self.settings = normalized({**defaults(), **settings})
        self.last_saved = deepcopy(self.settings)
        self.original_size = tuple(original)
        self.source = np.zeros((1, 1, 3), np.float32)
        self.current_id = 1
        self.render_version = 0
        self.commits = []
        self.size_text = ''
        self.view = CropView()
        self.aspect = AspectBox(self)
        self.studio = type('Studio', (), {'upright_generation': 0})()
        self.adjustments = {'straighten': Nop()}
        for name in ('crop_button', 'crop_apply', 'crop_cancel', 'tabs', 'before_button', 'wb_button'):
            setattr(self, name, Nop())
        window = self

        class SizeLabel:
            def clear(self):
                window.size_text = ''
        self.crop_size = SizeLabel()
        self.view.cropSelected.handler = self.apply_crop
        self.view.cropChanged.handler = self.crop_changed
        self.view.cropCancelled.handler = self.cancel_crop
        self.view.straightenDragged.handler = self.drag_straighten
        self.view.straightenFinished.handler = self.commit

    def commit(self, label='보정 변경'):
        if self.settings != self.last_saved:
            self.commits.append(label)
            self.last_saved = deepcopy(self.settings)

    def set_setting(self, key, value):
        self.settings[key] = deepcopy(value)
        switch_on(self.settings, (key,))

    def canvas_size(self):
        w, h = self.original_size
        return (h, w) if int(self.settings['rotation']) % 2 else (w, h)

    def crop_changed(self, crop):
        w, h = self.canvas_size()
        self.size_text = f'{round((crop[2]-crop[0])*w):,} × {round((crop[3]-crop[1])*h):,} px'

    def focusWidget(self):
        return None

    def set_mode(self, mode):
        pass

    def load_controls(self):
        pass

    def render(self):
        pass

    def statusBar(self):
        return Nop()

    def frame(self, scale, x, y):
        """A frame arrives (start_preview's sizes, then set_image)."""
        settings = self.settings
        editing = self.view.crop_mode
        use_crop = 'all' if editing and settings['straighten'] and not settings.get('upright') else not editing
        logical = self.preview_size(settings, use_crop)
        canvas = None
        if use_crop == 'all':
            inner = self.preview_size(settings, False)
            canvas = ((logical[0] - inner[0]) / 2, (logical[1] - inner[1]) / 2, *inner)
        self.view.canvas = canvas
        self.view.image_rect = QRectF(0, 0, *logical)
        self.view.scale, self.view.offset = scale, (x, y)
        if self.view.crop_mode and self.view.crop_aspect_pending:
            self.view.crop_aspect_pending = False
            self.view.set_aspect(self.view.aspect)
        return dict(logical=list(logical), canvas=list(canvas) if canvas else None,
                    use_crop=use_crop if use_crop == 'all' else bool(use_crop))

    def step(self, op):
        action, a = op[0], op[1:]
        record = {}
        view = self.view
        if action == 'toggle':
            self.toggle_crop(not view.crop_mode)
        elif action == 'frame':
            record.update(self.frame(*a))
        elif action == 'press':
            view.mousePressEvent(Pointer(*a))
            record['handle'] = view.drag_handle
        elif action == 'move':
            view.mouseMoveEvent(Pointer(*a))
        elif action == 'release':
            view.mouseReleaseEvent(Pointer(*a))
        elif action == 'double':
            view.mouseDoubleClickEvent(Pointer(*a))
        elif action == 'enter':
            view.keyPressEvent(Pointer(0, 0, Qt.Key.Key_Return))
        elif action == 'escape':
            view.keyPressEvent(Pointer(0, 0, Qt.Key.Key_Escape))
        elif action == 'apply':
            view.accept_crop()
        elif action == 'cancel':
            self.cancel_crop()
        elif action == 'aspect':
            self.aspect.setCurrentIndex(a[0])
        elif action == 'slider':
            self.set_straighten('straighten', a[0])
        elif action == 'commit':
            self.commit()
        elif action == 'reset':
            self.reset_crop()
        elif action == 'rotate':
            self.rotate_photo()
        elif action == 'flip':
            self.set_setting('flip', not self.settings['flip'])
            self.commit()
        elif action == 'auto_level':
            # auto_straighten's `finished` with the estimate's angle.
            angle = round(float(np.clip(a[0], -45, 45)), 1)
            self.commit()
            self.start_crop()
            previous = float(self.settings['straighten'])
            self.set_setting('upright', None)
            self.set_setting('straighten', angle)
            self.fit_crop(previous)
            self.commit('자동 수평 맞추기')
        elif action == 'border':
            # auto_border_crop: commit and end the tool, then `finished` with the crop found.
            self.commit()
            self.end_crop()
            self.set_setting('crop', list(a[0]))
            self.commit('스캔 테두리 자동 자르기')
        else:
            raise ValueError(action)
        s = self.settings
        record.update(op=op, frame=view.crop_values(), open=view.crop_mode, angle=view.straighten_angle,
                      limit=list(view.crop_limit), aspect=view.aspect, size=self.size_text,
                      settings={k: s[k] for k in ('crop', 'straighten', 'rotation', 'flip', 'upright')},
                      commits=list(self.commits))
        return record


CROP_SCRIPTS = {
    # A plain photo: move, corners, edges, a drag too small (reverted), an aspect, Enter, reopen, double-click.
    'plain': ((1500, 1000), {}, [
        ['toggle'], ['frame', 0.5, 40, 30],
        ['press', 415, 280], ['move', 430, 290], ['move', 2000, 2000], ['release', 470, 300],
        ['press', 790, 530], ['move', 700, 450], ['release', 690, 440],
        ['press', 40, 250], ['move', 120, 250], ['release', 140, 260],
        ['press', 690, 30], ['move', 640, 120], ['release', 600, 200],
        ['press', 370, 320], ['move', 390, 330], ['release', 395, 335],
        ['aspect', 2],
        ['press', 215, 215], ['move', 250, 240], ['release', 260, 250],
        ['press', 575, 350], ['move', 540, 350], ['release', 530, 352],
        ['aspect', 6],
        ['press', 395, 260], ['move', 395, 230], ['release', 395, 225],
        ['press', 395, 440], ['move', 395, 470], ['release', 396, 480],
        ['aspect', 8],
        ['press', 497, 390], ['move', 200, 200], ['release', 200, 200],
        ['press', 400, 30], ['move', 420, 20], ['release', 430, 25],
        ['frame', 0.5, 40, 30],
        ['press', 470, 400], ['move', 300, 250], ['release', 200, 150],
        ['enter'],
        ['toggle'], ['frame', 0.5, 40, 30],
        ['press', 420, 300], ['release', 420, 300], ['double', 420, 300],
    ]),
    # A straightened photo on the padded canvas: limits while moving and resizing, turning by dragging outside
    # the frame, an aspect, the slider, Esc (cancel back to the entry).
    'straightened': ((1200, 800), {'straighten': 7.5}, [
        ['toggle'], ['frame', 0.5, 20, 10],
        ['press', 360, 250], ['move', 600, 500], ['release', 620, 520],
        ['press', 680, 470], ['move', 760, 560], ['release', 780, 600],
        ['press', 8, 8], ['move', 20, 4], ['move', 40, 2], ['move', 60, 0], ['release', 60, 0],
        ['frame', 0.5, 20, 10],
        ['aspect', 1],
        ['slider', 12.3], ['commit'], ['frame', 0.5, 20, 10],
        ['press', 360, 250], ['move', 380, 240], ['release', 390, 245],
        ['slider', -3.0], ['commit'],
        ['escape'],
    ]),
    # Portrait (turned once) with a placed crop: an edge, a quarter turn while open, automatic level, scan
    # border, reopen with 16:9, Crop reset, flip.
    'rotated': ((1000, 1500), {'rotation': 1, 'straighten': -4.0, 'crop': [0.1, 0.1, 0.8, 0.7]}, [
        ['toggle'], ['frame', 0.25, 10, 10],
        ['press', 200, 40], ['move', 200, 70], ['release', 202, 80],
        ['rotate'], ['frame', 0.25, 10, 10],
        ['auto_level', 2.34], ['frame', 0.25, 10, 10],
        ['border', [0.05, 0.04, 0.97, 0.95]],
        ['aspect', 4], ['toggle'], ['frame', 0.25, 10, 10],
        ['press', 300, 200], ['move', 250, 150], ['release', 240, 140],
        ['apply'],
        ['reset'],
        ['flip'],
    ]),
    # Perspective alignment: no crop limit and the plain uncropped canvas; Crop reset removes a level alignment.
    'upright': ((900, 600), {'straighten': 5, 'upright': {'mode': 'level', 'rotation': 2.0}}, [
        ['toggle'], ['frame', 0.5, 0, 0],
        ['press', 448, 298], ['move', 520, 340], ['release', 520, 340],
        ['cancel'],
        ['reset'],
    ]),
}

for _name, (_original, _settings, _ops) in CROP_SCRIPTS.items():
    with Case('app', f'crop_{_name}') as case:
        window = CropWindow(_settings, _original)
        start = {k: window.settings[k] for k in ('crop', 'straighten', 'rotation', 'flip', 'upright')}
        steps = [window.step(op) for op in _ops]
        case.json(original=list(_original), changes=_settings, start=start, steps=steps)

with Case('app', 'crop_straighten') as case:
    # refit / shrink / limit / padding as the crop tool calls them (the geometry crate has its own goldens).
    rows = []
    for angle, w, h in ((7.5, 1200, 800), (-12.0, 800, 1200), (33.0, 1000, 1000)):
        for crop in ([0.1, 0.1, 0.9, 0.9], [0.0, 0.0, 1.0, 1.0], [0.3, 0.2, 0.5, 0.95]):
            for aspect in (None, 1.5):
                rows.append(dict(angle=angle, width=w, height=h, crop=crop, aspect=aspect,
                                 refit=straighten_module.refit(crop, 0., angle, w, h, aspect),
                                 shrink=straighten_module.shrink(crop, angle, w, h),
                                 limit=straighten_module.limit(crop, [0.4, 0.4, 0.6, 0.6], angle, w, h),
                                 padding=list(straighten_module.padding(angle, w, h))))
    case.json(rows=rows)


# ---- work package 1: the library column and the saved session -------------------------------------------------
# golden/app/libcol_*: the oracle's own FolderPanel.remove / MainWindow.save_folder_state / change_sort /
# DevelopPanels.save / FolderPanel.nodes / import_reason run on stand-ins, on a copy of golden/app/library.

import luma.folder_panel as folder_panel_module  # noqa: E402
from luma import develop_panels as develop_panels_module  # noqa: E402
from luma.app import import_reason  # noqa: E402
from luma.develop_panels import DevelopPanels  # noqa: E402
from luma.folder_panel import FolderPanel  # noqa: E402
from luma.folders import path_key  # noqa: E402

LIBCOL_COLUMNS = {   # the tables compared (photos and history without their large columns)
    'photos': ['id', 'path', 'name', 'virtual_source', 'stack_id'], 'history': ['id', 'photo_id', 'label'],
    'folder_roots': ['path'], 'preferences': ['key', 'value'], 'collection_members': ['collection_id', 'photo_id'],
    'collections': ['id', 'name', 'parent_id', 'rules'], 'preview_state': ['photo_id'],
    'snapshots': ['id', 'photo_id', 'name'],
}


def projected(db):
    out = {}
    for table, columns in LIBCOL_COLUMNS.items():
        cols = ','.join('"' + c + '"' for c in columns)
        out[table] = [dict(zip(columns, row)) for row in db.execute(f'SELECT {cols} FROM {table} ORDER BY {cols}')]
    return out


class Answer:
    """QMessageBox stand-in: every question is answered Yes."""
    class StandardButton:
        Yes = 1
        No = 2

    @staticmethod
    def question(*args, **kwargs):
        return 1

    @staticmethod
    def information(*args, **kwargs):
        return 0


class NoTimer:
    singleShot = staticmethod(lambda *args: None)


class Checked:
    def __init__(self, value):
        self.value = value

    def isChecked(self):
        return self.value


class Choice:
    def __init__(self, value):
        self.value = value

    def currentData(self):
        return self.value


class Quiet:
    """Takes `folder_sync.folder_removed`, `statusBar().showMessage` and friends."""
    watch_paths = []

    def __getattr__(self, name):
        return lambda *args, **kwargs: Quiet()


class FolderWindow(Window):
    save_folder_state = MainWindow.save_folder_state
    change_sort = MainWindow.change_sort

    def __init__(self, catalog):
        super().__init__(catalog)
        self.folder_filter = None
        self.include_subfolders = Checked(True)
        self.expanded_folders = set()
        self.maintenance_running = False
        self.folder_sync = Quiet()
        self.extras = Quiet()
        self.sort_order = Choice('imported')
        self.sort_descending = Checked(False)
        self.cleared = 0
        self.closing = False

    def save_keywords(self):
        pass

    def clear_active_photo(self):
        self.cleared += 1
        self.current_id = None
        self.settings = defaults()
        self.selection = []

    def refresh_lists(self, *args, **kwargs):
        pass


class Panel:
    """What `FolderPanel.remove` / `nodes` read of the panel."""
    nodes = FolderPanel.nodes
    remove = FolderPanel.remove

    def __init__(self, window):
        self.w = window
        self.cancel = threading.Event()
        self.recovery_cancel = threading.Event()
        self.recovery_signature = None
        self.signature = None
        self.photos = []
        self.roots = []
        self.children = []
        self.missing = set()
        self.scanned = set()

    def busy(self):
        return False

    def refresh(self):
        pass


folder_panel_module.QMessageBox = Answer
folder_panel_module.QTimer = NoTimer

SESSION_ROLL2 = ROOT + r'\photos\roll 2'
SESSION_PHOTOS = ROOT + r'\photos'


def libcol_catalog(work, name):
    folder = work / name
    folder.mkdir()
    shutil.copyfile(GOLDEN / 'app' / 'library' / 'start.sqlite', folder / 'catalog.sqlite')
    return folder, Catalog(folder)


def state_values(catalog):
    return {key: catalog.preference(key, None) for key in ('folder_panel', 'browser_view', 'develop_panels')}


with tempfile.TemporaryDirectory(prefix='grainy-app-libcol-', ignore_cleanup_errors=True) as temporary:
    work = Path(temporary).resolve()

    # Removing a folder: backup, photos, roots, preferences, the saved state afterwards.
    folder, c = libcol_catalog(work, 'remove')
    window = FolderWindow(c)
    window.activate(6)                                    # a photo of the folder that goes
    window.folder_filter = SESSION_ROLL2
    window.expanded_folders = {path_key(SESSION_PHOTOS), path_key(SESSION_ROLL2)}
    expanded_before = sorted(window.expanded_folders)
    window.save_folder_state()
    before = projected(c.db)
    panel = Panel(window)
    panel.remove(SESSION_ROLL2)
    window.commit()
    c.db.commit()
    backups = sorted((folder / 'backups').glob('folder-removal-*.sqlite'))
    backup_tables = projected(sqlite3.connect(backups[0])) if backups else None
    final = projected(c.db)
    with Case('app', 'libcol_remove') as case:
        case.json(folder=SESSION_ROLL2, open_photo=6, expanded=expanded_before, columns=LIBCOL_COLUMNS,
                  before=before, final=final, backup=backup_tables, backups=len(backups), cleared=window.cleared,
                  folder_filter_after=window.folder_filter, expanded_after=sorted(window.expanded_folders))
    c.close()

    # The whole `photos` folder goes; a drive root is refused silently.
    folder, c = libcol_catalog(work, 'remove_all')
    window = FolderWindow(c)
    window.folder_filter = SESSION_PHOTOS
    window.expanded_folders = {path_key(SESSION_PHOTOS)}
    panel = Panel(window)
    panel.remove('Z:\\')
    drive_backups = len(list((folder / 'backups').glob('*'))) if (folder / 'backups').exists() else 0
    panel.remove(SESSION_PHOTOS)
    backups = sorted((folder / 'backups').glob('folder-removal-*.sqlite'))
    final = projected(c.db)
    with Case('app', 'libcol_remove_all') as case:
        case.json(folder=SESSION_PHOTOS, columns=LIBCOL_COLUMNS, final=final, backups=len(backups),
                  backups_after_drive=drive_backups, folder_filter_after=window.folder_filter,
                  expanded_after=sorted(window.expanded_folders))
    c.close()

    # The saved state, step by step (each step's three preferences).
    folder, c = libcol_catalog(work, 'state')
    window = FolderWindow(c)
    sections = {key: type('S', (), {'header': Checked(False)})() for key, _ in develop_panels_module.PANELS}
    sections['basic'].header = Checked(True)
    dp = type('DP', (), {})()
    dp.ready = True
    dp.w = window
    dp.solo = True
    dp.sections = sections
    steps = []

    def record(name, **extra):
        steps.append(dict(step=name, values=state_values(c), **extra))

    window.activate(6)
    window.folder_filter = SESSION_ROLL2
    window.expanded_folders = {path_key(SESSION_PHOTOS), path_key(SESSION_ROLL2)}
    window.save_folder_state()
    record('select_folder', folder=SESSION_ROLL2, expanded=sorted(window.expanded_folders), current=6)
    window.include_subfolders = Checked(False)
    window.save_folder_state()
    record('subfolders_off')
    window.sort_order = Choice('name')
    window.sort_descending = Checked(True)
    window.change_sort()
    record('sort_name_descending', sort='name', descending=True)
    dp.solo = False
    sections['curve'].header = Checked(True)
    DevelopPanels.save(dp)
    record('panels_curve_open', solo=False, expanded=['basic', 'curve'])
    window.activate(3)
    window.folder_filter = None
    window.expanded_folders = {path_key(SESSION_PHOTOS)}
    window.save_folder_state()
    record('all_photos', expanded=sorted(window.expanded_folders), current=3)
    sections['basic'].header = Checked(False)
    sections['crop'].header = Checked(True)                # a tool panel is never saved
    dp.solo = True
    DevelopPanels.save(dp)
    record('panels_solo_curve_only', solo=True, expanded=['curve'])
    with Case('app', 'libcol_state') as case:
        case.json(steps=steps, root=ROOT)
    c.close()

    # The folder tree with its marks (FolderPanel.nodes).
    folder, c = libcol_catalog(work, 'tree')
    snapshot = work / 'tree-query.sqlite'
    shutil.copyfile(folder / 'catalog.sqlite', snapshot)
    listing = library_query.query(snapshot, listing_options({}, c.preference('stack_orders', {})), threading.Event())
    rows, roots = listing['folders'], listing['roots']
    sub = SESSION_ROLL2 + r'\sub'
    cases = [
        ('plain', dict(children=[], missing=[], scanned=[], removed=[])),
        ('scanned_children', dict(children=[SESSION_ROLL2 + r'\empty', sub, ROOT + r'\elsewhere\x'], missing=[],
                                  scanned=[path_key(SESSION_PHOTOS), path_key(SESSION_ROLL2)], removed=[])),
        ('missing_folder', dict(children=[], missing=[path_key(SESSION_ROLL2)], scanned=[path_key(SESSION_PHOTOS)],
                                removed=[])),
        ('left_out', dict(children=[SESSION_ROLL2 + r'\empty', SESSION_PHOTOS + r'\other'], missing=[], scanned=[],
                          removed=[SESSION_ROLL2])),
    ]
    results = []
    for name, spec in cases:
        window = FolderWindow(c)
        window.catalog = type('C', (), {'removed_folders': staticmethod(lambda removed=spec['removed']: removed)})()
        panel = Panel(window)
        panel.children, panel.missing, panel.scanned = spec['children'], set(spec['missing']), set(spec['scanned'])
        nodes = panel.nodes(rows, roots)
        results.append(dict(name=name, **spec, nodes=nodes))
    with Case('app', 'libcol_tree') as case:
        case.json(rows=rows, roots=roots, cases=results)
    c.close()

    # import_reason on tracebacks of the kinds the import produces.
    reasons = []
    for path, error in [
        ('C:\\p\\a.jpg', 'Traceback (most recent call last):\n  File "x.py", line 1, in f\nPIL.UnidentifiedImageError: cannot identify image file'),
        ('C:\\p\\a.dng', 'Traceback\nrawpy._rawpy.LibRawFileUnsupportedError: Unsupported file format'),
        ('C:\\p\\a.NEF', 'Traceback\nrawpy._rawpy.LibRawFileUnsupportedError: x'),
        ('C:\\p\\b.nrw', 'rawpy._rawpy.LibRawFileUnsupportedError: x\n'),
        ('C:\\p\\c.png', 'ValueError: 동영상 파일은 지원하지 않습니다.'),
        ('C:\\p\\d.tif', '  \nOSError: [Errno 2] No such file or directory\n\n'),
        ('C:\\p\\e.jpg', 'single line'),
    ]:
        reasons.append(dict(path=path, error=error, reason=import_reason(path, error)))
    with Case('app', 'libcol_import_reason') as case:
        case.json(cases=reasons)
