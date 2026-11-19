"""Goldens of grainy-app work package 9: masks by brush, colour and luminance ranges, sky.

`StudioTools`' own methods (`stroke`, `preview_stroke`, `make_component`, `add_component`, `ranged`,
`brush_range_changed`, `add_luma`, `range_changed`, `sky_changed`, `add_sky`, `remove_range_sample`, `pick_range`
with `range_mask.sample_selection` on a real develop, `set_brush_size`) and `PhotoView.resize_brush` run unbound
on `Studio`, a stand-in with the widgets they read as plain values. `pick_range`'s background job runs at once
(`spawn` calls the work, then `ready` or `failed`), on the standard `garden` input as the source. `mode` is the
part of `StudioTools.mode` the mask tools use (the guard of 색상 범위 추가, the pick generation, the overlay, the
view's tool and brush, the range note); `load_layer` is its brush-range part (the range belongs to the mask).

After every step: the `masks` value, the selection, the tool mode, the history labels, the range note, the brush
size slider and the view's brush radius.

Run: "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_masks2.py
"""
from copy import deepcopy

from common import Case, inputs, np
from luma.engine import defaults, normalized
from luma.panel_keys import switch_on
from luma.studio import StudioTools
from luma.widgets import PhotoView


class Silent:
    """Any widget call does nothing."""

    def __getattr__(self, name):
        return lambda *args, **kwargs: None


class Slider:
    """`QSlider`: a clamped integer position; a change calls `on_change` (the `Adjustment.changed` path)."""

    def __init__(self, position, low, high, on_change=None):
        self.v, self.low, self.high, self.on_change = position, low, high, on_change

    def value(self):
        return self.v

    def setValue(self, value):
        value = int(min(max(value, self.low), self.high))
        if value != self.v:
            self.v = value
            if self.on_change:
                self.on_change()


class Option:
    """A tool option (`Adjustment`): only `slider` is used."""

    def __init__(self, position, low, high, on_change=None):
        self.slider = Slider(position, low, high, on_change)

    def set_value(self, value):
        pass


class Choice:
    def __init__(self, items, current):
        self.items = items
        self.index = items.index(current)

    def currentData(self):
        return self.items[self.index]

    def currentIndex(self):
        return self.index

    def findData(self, data):
        return self.items.index(data) if data in self.items else -1

    def setCurrentIndex(self, index):
        self.index = index


class Check:
    def __init__(self, checked):
        self.checked = checked

    def isChecked(self):
        return self.checked


class Note:
    def __init__(self, text):
        self.text = text

    def setText(self, text):
        self.text = text


class Emitter:
    def __init__(self, slot):
        self.slot = slot

    def emit(self, *args):
        self.slot(*args)


class View:
    resize_brush = PhotoView.resize_brush

    def __init__(self, studio):
        self.studio = studio
        self.tool_mode = ''
        self.tool_handles = []
        self.handle_kind = 'linear'
        self.handle_angle = 0.
        self.on_screen = None
        self.tool_shift = False
        self.tool_points = []
        self.fit_mode = True
        self.brush_radius = .025
        self.brush_feather = 70
        self.brushSizeChanged = Emitter(studio.set_brush_size)

    def set_tool(self, mode):
        self.tool_mode = mode
        self.tool_handles = []
        self.tool_points = []
        self.studio.mask_tool_changed(mode)

    def viewport(self):
        return Silent()

    def cancel_stroke(self):
        self.tool_points = []


class Window:
    def __init__(self, settings, source):
        self.settings = settings
        self.last_saved = deepcopy(settings)
        self.history = []
        self.load_version = 1
        self.source = source
        self.full_source = None
        self.original_size = (source.shape[1], source.shape[0])
        h, w = source.shape[:2]
        scale = 720 / max(h, w)
        self.live_source = np.zeros((round(h * scale), round(w * scale), 3), np.float32) if scale < 1 else source
        self.current_id = 1
        self.closing = False
        self.render_version = 0
        self.preview_timer = Silent()
        self.refine_timer = Silent()

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

    def spawn(self, work, ready, failed):
        try:
            result = work()
        except Exception as error:      # noqa: BLE001 - the window hands every error to `failed`
            failed(error)
        else:
            ready(result)


class Studio:
    for _name in ('source_point', 'source_points', 'frame_affine', 'gradient_point', 'screen_point',
                  'radial_from_screen', 'radial_on_screen', 'gradient_shape', 'component_handles',
                  'stroke', 'preview_stroke', 'make_component', 'current_layer', 'new_layer', 'add_component',
                  'range_component', 'ranged', 'remove_component', 'layer_property', 'local_changed',
                  'set_operation', 'create_mask', 'extend_mask', 'drop_fresh_layer', 'mask_tool_changed',
                  'current_component', 'replace_component', 'stroke_valid', 'cancel_stroke', 'pause_overlay',
                  'add_sky', 'brush_range_changed', 'add_luma', 'range_changed', 'sky_changed', 'remove_range_sample',
                  'pick_range', 'set_brush_size', 'edit_component_handles'):
        locals()[_name] = getattr(StudioTools, _name)
    brush_range_place = staticmethod(StudioTools.brush_range_place)
    BRUSH_RANGES = StudioTools.BRUSH_RANGES
    BRUSH_RANGE_SOFTNESS = StudioTools.BRUSH_RANGE_SOFTNESS

    def __init__(self, settings, source):
        self.w = Window(settings, source)
        self.loading = False
        self.range_loading = False
        self.layer_index = 0
        self.component_index = -1
        self.fresh_layer = None
        self.pending_stroke = None
        self.overlay_paused = False
        self.stroke_generation = 0
        self.color_pick_generation = 0
        self.color_pick_running = False
        self.color_pick_token = None
        self.coordinate_key = self.coordinates = self.affine_key = self.affine = None
        self.w.view = View(self)
        self.radius = Option(25, 2, 250, self.brush_cursor)
        self.feather = Option(70, 0, 100, self.brush_cursor)
        self.flow = Option(100, 1, 100)
        self.density = Option(100, 1, 100)
        self.auto_mask = Check(False)
        self.auto_tolerance = Option(18, 2, 100)
        self.tolerance = Option(30, 2, 170)
        self.low = Option(25, 0, 100)
        self.high = Option(75, 0, 100)
        self.retouch_radius = Option(15, 2, 120)
        self.retouch_feather = Option(60, 0, 100)
        self.overlay = Check(True)
        self.operation = Choice(['add', 'subtract', 'intersect'], 'add')
        self.brush_range = Choice([key for key, _, _ in StudioTools.BRUSH_RANGES], 'all')
        self.range_controls = {key: Option(0, -10 ** 6, 10 ** 6) for key in ('low', 'high', 'softness', 'tolerance')}
        self.range_samples = Choice([0, 1, 2, 3, 4], 0)
        self.range_pick_note = Note('점이나 작은 사각형을 선택하세요. Shift+클릭: 같은 범위에 색 추가(최대 5개) · Esc: 종료')

    def mode(self, kind):
        """`StudioTools.mode` for the mask kinds (no crop, white balance or before view here)."""
        self.color_pick_generation += 1
        if kind == 'color_range_add' and (self.current_component() or {}).get('type') != 'color':
            self.range_pick_note.setText('목록에서 색상 범위를 먼저 선택하세요.')
            return
        if kind in ('brush', 'linear', 'radial', 'color_range', 'color_range_add', 'luma_range', 'edit_component'):
            self.overlay_paused = False
        self.w.view.set_tool(kind)
        self.w.view.brush_radius = self.radius.slider.value() / 1000
        self.w.view.brush_feather = self.feather.slider.value()
        if kind in ('color_range', 'color_range_add', 'luma_range'):
            self.range_pick_note.setText('사진의 점이나 작은 사각형을 선택하세요. Shift+클릭으로 색을 더하고 Esc로 마칩니다.')

    def select_component(self, index):
        self.overlay_paused = False
        self.w.commit()
        self.component_index = index

    def select_layer(self, index):
        self.color_pick_generation += 1
        self.overlay_paused = False
        self.w.commit()
        self.layer_index = index
        self.component_index = -1
        self.w.view.set_tool('')
        self.load_layer()

    def load_layers(self):
        self.load_layer()

    def load_layer(self):
        """The part of `load_layer` the brush uses: the range belongs to the mask."""
        if self.layer_index >= len(self.w.settings['masks']):
            self.layer_index = len(self.w.settings['masks']) - 1
        layer = self.current_layer() or {}
        components = layer.get('components', [])
        place = self.brush_range_place(components)
        if place is not None or components or not self.w.view.tool_mode == 'brush':
            self.brush_range.setCurrentIndex(max(0, self.brush_range.findData(components[place]['brush_range'] if place is not None else 'all')))

    def update_mask_ui(self):
        pass

    def brush_cursor(self):
        component = self.pending_stroke['component'] if self.pending_stroke else {}
        self.w.view.brush_radius = component.get('radius', self.radius.slider.value() / 1000)
        self.w.view.brush_feather = component.get('feather', self.feather.slider.value())

    def preview_ready(self, image):
        pass

    def choose_range(self, key):
        """The 밝기 제한 box changed (`currentIndexChanged` → `brush_range_changed`)."""
        index = self.brush_range.findData(key)
        if index != self.brush_range.index:
            self.brush_range.setCurrentIndex(index)
            self.brush_range_changed()

    def press(self, points, alt=False, shift=False):
        """A stroke through the view: `strokeProgress` at the press and each move (brush), then `strokeCompleted`."""
        self.w.view.tool_shift = shift
        kept = []
        for point in points:                       # the view keeps a point only when it moved
            if not kept or kept[-1] != point:
                kept.append(point)
                if self.w.view.tool_mode == 'brush':
                    self.preview_stroke(list(kept), alt)
        self.stroke(kept, alt)


def steps():
    """(name, call) of the session."""
    def option(name, position):
        return lambda m: getattr(m, name).slider.setValue(position)

    return [
        ('create_color', lambda m: m.create_mask('color_range')),
        ('pick_color_click', lambda m: m.press([[0.3, 0.4]])),
        ('add_mode', lambda m: m.mode('color_range_add')),
        ('pick_color_add_rect', lambda m: m.press([[0.1, 0.1], [0.2, 0.3], [0.55, 0.6]])),
        ('back_to_color', lambda m: m.mode('color_range')),
        ('pick_color_shift_full', lambda m: m.press([[0.7, 0.2], [0.9, 0.5]], shift=True)),
        ('tolerance', lambda m: (m.range_changed('tolerance', 0.55), m.w.finish_interaction())),
        ('remove_sample', lambda m: (m.range_samples.setCurrentIndex(1), m.remove_range_sample())),
        ('pick_color_shift', lambda m: m.press([[0.45, 0.8]], shift=True)),
        ('tolerance_option', option('tolerance', 64)),
        ('pick_color_new', lambda m: m.press([[0.8, 0.85], [0.95, 0.95]])),
        ('extend_luma', lambda m: m.extend_mask('luma_range', 'intersect')),
        ('pick_luma', lambda m: m.press([[0.2, 0.2], [0.6, 0.7]])),
        ('add_mode_refused', lambda m: m.mode('color_range_add')),
        ('range_low', lambda m: m.range_changed('low', 30.0)),
        ('range_high_below', lambda m: m.range_changed('high', 20.0)),
        ('range_softness', lambda m: (m.range_changed('softness', 25.0), m.w.finish_interaction())),
        ('layer_exposure', lambda m: (m.local_changed('exposure', 0.8), m.w.commit())),
        ('create_luma', lambda m: m.create_mask('luma_range')),
        ('pick_luma_after_layer', lambda m: m.press([[0.5, 0.5]])),
        ('create_color_2', lambda m: m.create_mask('color_range')),
        ('pick_color_after_layers', lambda m: m.press([[0.15, 0.6], [0.4, 0.9]])),
        ('end_pick', lambda m: m.w.view.set_tool('')),
        ('create_sky', lambda m: m.create_mask('sky')),
        ('sky_sensitivity', lambda m: m.sky_changed('sensitivity', 70.0)),
        ('sky_feather', lambda m: (m.sky_changed('feather', 12.0), m.w.finish_interaction())),
        ('create_brush', lambda m: m.create_mask('brush')),
        ('brush', lambda m: m.press([[0.2, 0.2], [0.3, 0.25], [0.4, 0.3]])),
        ('brush_alt', lambda m: m.press([[0.25, 0.22], [0.35, 0.27]], alt=True)),
        ('brush_options', lambda m: (m.radius.slider.setValue(50), m.feather.slider.setValue(30),
                                     m.flow.slider.setValue(40), m.density.slider.setValue(60))),
        ('brush_with_options', lambda m: m.press([[0.6, 0.6], [0.62, 0.7], [0.7, 0.75], [0.7, 0.75]])),
        ('range_shadows', lambda m: m.choose_range('shadows')),
        ('brush_in_range', lambda m: m.press([[0.1, 0.8], [0.2, 0.85]])),
        ('brush_alt_in_range', lambda m: m.press([[0.12, 0.8]], alt=True)),
        ('range_highlights', lambda m: m.choose_range('highlights')),
        ('range_all', lambda m: m.choose_range('all')),
        ('auto_mask', lambda m: (setattr(m.auto_mask, 'checked', True), m.auto_tolerance.slider.setValue(25))),
        ('brush_auto', lambda m: m.press([[0.5, 0.3], [0.55, 0.35]])),
        ('auto_mask_off', lambda m: setattr(m.auto_mask, 'checked', False)),
        ('create_brush_ranged', lambda m: m.create_mask('brush')),
        ('range_midtones_first', lambda m: m.choose_range('midtones')),
        ('brush_ranged_first', lambda m: m.press([[0.4, 0.4], [0.5, 0.5]])),
        ('brush_ranged_second', lambda m: m.press([[0.45, 0.6]])),
        ('subtract_brush', lambda m: m.extend_mask('brush', 'subtract')),
        ('brush_subtract_in_range', lambda m: m.press([[0.3, 0.3], [0.35, 0.3]])),
        ('bigger', lambda m: m.w.view.resize_brush(True)),
        ('bigger_again', lambda m: m.w.view.resize_brush(True)),
        ('smaller', lambda m: m.w.view.resize_brush(False)),
        ('smaller_many', lambda m: [m.w.view.resize_brush(False) for _ in range(30)]),
        ('largest', lambda m: [m.w.view.resize_brush(True) for _ in range(40)]),
        ('brush_after_resize', lambda m: m.press([[0.8, 0.2]])),
        ('end_brush', lambda m: m.w.view.set_tool('')),
        ('resize_without_brush', lambda m: m.w.view.resize_brush(True)),
        ('add_luma', lambda m: m.add_luma()),
    ]


CASES = {
    'plain': {'exposure': 0.4, 'contrast': 10},
    'straighten': {'straighten': 8.0, 'vibrance': 20},
}

source = np.ascontiguousarray(inputs()['garden'].astype(np.float32))
for name, changes in CASES.items():
    with Case('app', f'masks2_{name}') as case:
        studio = Studio(normalized({**defaults(), **changes}), source)
        records = []
        for step, call in steps():
            call(studio)
            records.append(dict(
                step=step, masks=deepcopy(studio.w.settings['masks']), layer=studio.layer_index,
                component=studio.component_index, mode=studio.w.view.tool_mode, history=list(studio.w.history),
                note=studio.range_pick_note.text, brush_range=studio.brush_range.currentData(),
                radius=studio.radius.slider.value(), view_radius=float(studio.w.view.brush_radius)))
        case.json(changes=changes, width=int(source.shape[1]), height=int(source.shape[0]), steps=records)
