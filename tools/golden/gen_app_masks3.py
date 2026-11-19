"""Goldens of grainy-app work package 10: the mask fold-out 커브 · 색상 혼합 · 선택 색상 and the component dialog.

* `golden/app/masks3_dialog`: luma/mask_dialog.py `MaskComponentDialog` built headless for components of every
  kind; the fields are edited through their own `QDoubleSpinBox.setValue` (rounding to the field's decimals and
  clamping to its range as Qt does), the check boxes and the 결합 box set; recorded per case: the form's rows
  (label, value), the fields' values as shown, and `result_component()`.
* `golden/app/masks3_local`: luma/studio.py `load_local_curve`, `local_curve_changed`, `load_local_hsl`,
  `local_hsl_changed`, `load_local_points`, `load_local_point`, `local_point_changed`, `remove_local_point` and
  `pick_point_color(local=True)` (its job run at once: luma/point_color.py `sample` with the layer on the oracle's
  develop of a generated source) run unbound on a stand-in with the fold-out's real widgets (combo boxes,
  `Adjustment` sliders, `ToneCurve`, check box, note); after every step the `masks` value, the history labels, the
  selection, what the widgets show, the note and the status. The coverage `point_color.preview` draws for every
  local point of the final settings is stored as `coverage_{layer}_{point}.npy`.

Nothing is written outside `golden/app/masks3*`. Run:
    "$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_app_masks3.py
"""
from copy import deepcopy

from common import Case, inputs, np

from PySide6.QtWidgets import QApplication, QCheckBox, QComboBox, QLabel, QPushButton  # noqa: E402
from luma import point_color  # noqa: E402
from luma.engine import defaults, normalized  # noqa: E402
from luma.mask_dialog import MaskComponentDialog  # noqa: E402
from luma.panel_keys import switch_on  # noqa: E402
from luma.studio import StudioTools  # noqa: E402
from luma.widgets import Adjustment, ToneCurve  # noqa: E402

_qt = QApplication.instance() or QApplication([])


# ---- the component dialog ----------------------------------------------------------------------------------------

DIALOG_CASES = [
    # (name, component, field edits, other edits: enabled / invert / operation / auto_mask)
    ('brush_unchanged', {'type': 'brush', 'points': [[0.2, 0.3], [0.4, 0.5]], 'radius': 0.025, 'feather': 70,
                         'operation': 'add', 'paint_version': 2, 'flow': 100, 'density': 100}, {}, {}),
    ('brush_legacy', {'type': 'brush', 'points': [[0.1, 0.1], [0.3, 0.2]], 'operation': 'add'},
     {'radius': 5.555, 'flow': 55.55}, {'auto_mask': True}),
    ('brush_legacy_untouched', {'type': 'brush', 'points': [[0.1, 0.1]], 'radius': 0.03333, 'feather': 33.33}, {},
     {}),
    ('brush_auto_off', {'type': 'brush', 'points': [[0.5, 0.5], [0.6, 0.6]], 'radius': 0.05, 'feather': 40,
                        'flow': 60, 'density': 80, 'paint_version': 2, 'auto_mask': True, 'auto_tolerance': 0.3,
                        'seed': [0.55, 0.52], 'operation': 'subtract'}, {}, {'auto_mask': False}),
    ('brush_ties', {'type': 'brush', 'points': [[0.2, 0.2]], 'radius': 0.02, 'flow': 100, 'density': 100,
                    'auto_mask': True, 'auto_tolerance': 0.18, 'seed': [0.2, 0.2]},
     {'radius': 2.125, 'flow': 70.25, 'density': 12.35, 'auto_tolerance': 0.125, 'feather': 0.05}, {}),
    ('brush_clamped', {'type': 'brush', 'points': [[0.2, 0.2]], 'radius': 0.5, 'flow': 150, 'density': -3,
                       'feather': 120},
     {'auto_tolerance': 3.0}, {'enabled': False, 'invert': True, 'operation': 'intersect'}),
    ('brush_low_radius', {'type': 'brush', 'points': [[0.2, 0.2]], 'radius': 0.001, 'flow': 100, 'density': 100},
     {'flow': 100.04}, {}),
    ('linear', {'type': 'linear', 'points': [[-0.5, 0.25], [1.7, 0.8]], 'operation': 'add'},
     {'x1': 350.0, 'y0': 12.345}, {'enabled': False, 'invert': True, 'operation': 'subtract'}),
    ('linear_three_points', {'type': 'linear', 'points': [[0.1, 0.2], [0.5, 0.5], [0.9, 0.95]],
                             'operation': 'intersect', 'enabled': False}, {}, {'enabled': True}),
    ('radial', {'type': 'radial', 'points': [[0.2, 0.3], [0.8, 0.9]], 'angle': 33.333, 'operation': 'add'},
     {'angle': -200.0, 'feather': 45.55}, {}),
    ('radial_defaults', {'type': 'radial'}, {}, {}),
    ('luma', {'type': 'luma', 'range': [0.6, 0.2], 'softness': 0.25, 'operation': 'intersect'}, {}, {}),
    ('luma_edited', {'type': 'luma', 'range': [0.2, 0.6], 'softness': 0.25, 'operation': 'add', 'invert': True},
     {'low': 80.0, 'high': 30.0, 'softness': 0.0}, {'invert': False}),
    ('luma_defaults', {'type': 'luma', 'brush_range': 'shadows'}, {'softness': 55.55}, {}),
    ('color', {'type': 'color', 'colors': [[0.5, 0.2, 0.1]], 'tolerance': 0.333, 'operation': 'add'},
     {}, {}),
    ('color_edited', {'type': 'color', 'colors': [[0.5, 0.2, 0.1]], 'operation': 'add'},
     {'tolerance': 2.0}, {'operation': 'subtract'}),
    ('sky', {'type': 'sky', 'operation': 'add'}, {'feather': 12.34}, {}),
    ('sky_stored', {'type': 'sky', 'sensitivity': 72.5, 'feather': 10, 'operation': 'add'}, {'sensitivity': 101.0},
     {}),
    ('unknown', {'type': 'foo', 'operation': 'weird', 'value': 3}, {}, {}),
    ('unknown_ops', {'type': 'foo'}, {}, {'operation': 'intersect'}),
]


def form_rows(dialog):
    """The form's rows: (label, what the row shows)."""
    from PySide6.QtWidgets import QFormLayout
    form = dialog.layout().itemAt(0).layout()
    rows = []
    for i in range(form.rowCount()):
        label = form.itemAt(i, QFormLayout.ItemRole.LabelRole)
        field = form.itemAt(i, QFormLayout.ItemRole.FieldRole) or form.itemAt(i, QFormLayout.ItemRole.SpanningRole)
        widget = field.widget() if field else None
        text = label.widget().text() if label and label.widget() else ''
        if isinstance(widget, QCheckBox):
            shown = dict(check=widget.text(), checked=widget.isChecked())
        elif isinstance(widget, QComboBox):
            shown = dict(choice=widget.currentData(), items=[widget.itemText(j) for j in range(widget.count())])
        elif isinstance(widget, QLabel):
            shown = dict(text=widget.text())
        else:
            shown = dict(value=widget.value(), low=widget.minimum(), high=widget.maximum(), decimals=widget.decimals())
        rows.append(dict(label=text, **shown))
    return rows


with Case('app', 'masks3_dialog') as case:
    records = []
    for name, component, edits, others in DIALOG_CASES:
        dialog = MaskComponentDialog(None, deepcopy(component))
        shown = form_rows(dialog)
        for key, value in edits.items():
            dialog.fields[key].setValue(value)
        if 'enabled' in others:
            dialog.enabled.setChecked(others['enabled'])
        if 'invert' in others:
            dialog.invert.setChecked(others['invert'])
        if 'operation' in others:
            dialog.operation.setCurrentIndex(dialog.operation.findData(others['operation']))
        if 'auto_mask' in others:
            dialog.auto_mask.setChecked(others['auto_mask'])
        try:
            result = dialog.result_component()
            error = ''
        except Exception as e:  # noqa: BLE001 - the oracle's slot would raise
            result, error = None, f'{type(e).__name__}: {e}'
        records.append(dict(name=name, component=component, edits=edits, others=others, title=dialog.windowTitle(),
                            rows=shown, values={k: f.value() for k, f in dialog.fields.items()},
                            result=result, error=error))
        dialog.deleteLater()
    case.json(cases=records)


# ---- the fold-out --------------------------------------------------------------------------------------------------

def local_source():
    """The garden, halved (80 x 120), scene-linear float32: the develop source of the picks."""
    return np.ascontiguousarray(inputs()['garden'][::2, ::2], dtype=np.float32)


LOCAL_SETTINGS = dict(exposure=0.3, contrast=10.0, saturation=5.0, masks=[
    dict(name='마스크 1', enabled=True, opacity=1, components=[
        dict(type='linear', points=[[0.0, 0.5], [0.7, 0.5]], radius=0.025, feather=70, operation='add')],
        adjustments=dict(exposure=0.25, saturation=-10.0)),
    dict(name='마스크 2', enabled=True, opacity=0.8, components=[
        dict(type='radial', points=[[0.45, 0.1], [1.0, 0.95]], radius=0.025, feather=50, operation='add')],
        adjustments=dict(temperature=15.0, point_colors=[
            dict(version=2, rgb=[0.4, 0.5, 0.2], range=0.15, saturation_range=0.6, lightness_range=0.5, hue=10,
                 saturation=0, lightness=0)])),
    dict(name='마스크 3', enabled=False, opacity=1, components=[
        dict(type='linear', points=[[0.5, 0.0], [0.5, 1.0]], radius=0.025, feather=70, operation='add')],
        adjustments={}),
])

HSL_TITLES = ['색상표 색상', '색상표 채도', '색상표 명도']
POINT_CONTROLS = [('range', '선택 색상 범위', .01, .5, 100), ('saturation_range', '선택 채도 범위', .01, 1, 100),
                  ('lightness_range', '선택 명도 범위', .01, 1, 100), ('hue', '선택 색상 이동', -100, 100, 1),
                  ('saturation', '선택 채도 보정', -100, 100, 1), ('lightness', '선택 명도 보정', -100, 100, 1)]


class Check:
    def __init__(self, checked):
        self.checked = checked

    def isChecked(self):
        return self.checked


class View:
    def __init__(self):
        self.tool_mode = ''
        self.fit_mode = True
        self.on_screen = None


class Window:
    def __init__(self, settings, source):
        self.settings = settings
        self.last_saved = deepcopy(settings)
        self.history = []
        self.messages = []
        self.source = source
        self.full_source = None
        self.original_size = (source.shape[1], source.shape[0])
        self.current_id = 1
        self.load_version = 1
        self.closing = False
        self.view = View()

    def statusBar(self):
        return self

    def showMessage(self, text, *args):
        self.messages.append(text)

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

    def spawn(self, work, ready, failed):
        try:
            value = work()
        except Exception as error:   # noqa: BLE001 - the oracle's job reports any failure
            failed(error)
        else:
            ready(value)


class LocalStudio:
    """The fold-out of build_masks (studio.py) with its real widgets; the StudioTools methods run unbound."""
    for _name in ('current_layer', 'layer_property', 'load_local_hsl', 'local_hsl_changed', 'local_point_values',
                  'load_local_points', 'load_local_point', 'local_point_changed', 'remove_local_point',
                  'pick_point_color', 'load_local_curve', 'local_curve_changed', 'pause_overlay'):
        locals()[_name] = getattr(StudioTools, _name)

    def __init__(self, window):
        self.w = window
        self.loading = False
        self.layer_index = 0
        self.component_index = -1
        self.local_point_index = 0
        self.overlay_paused = False
        self.overlay = Check(True)
        self.color_pick_generation = 0
        self.color_pick_running = False
        self.color_pick_token = None
        self.point_note = QLabel('')
        self.local_curve_channel = QComboBox()
        self.local_curve_channel.addItems(['부분 커브 · RGB', '부분 커브 · 빨강', '부분 커브 · 초록', '부분 커브 · 파랑'])
        self.local_curve = ToneCurve()
        self.local_curve_channel.currentIndexChanged.connect(self.load_local_curve)
        self.local_curve.changed.connect(self.local_curve_changed)
        self.local_curve.committed.connect(self.w.finish_interaction)
        self.local_hsl_channel = QComboBox()
        self.local_hsl_channel.addItems(['빨강', '주황', '노랑', '초록', '청록', '파랑', '보라', '자홍'])
        self.local_hsl_channel.currentIndexChanged.connect(self.load_local_hsl)
        self.local_hsl_controls = []
        for column, title in enumerate(HSL_TITLES):
            c = Adjustment(str(column), title, -100, 100)
            c.changed.connect(self.local_hsl_changed)
            c.committed.connect(self.w.finish_interaction)
            self.local_hsl_controls.append(c)
        self.local_point_add = QPushButton('부분 선택 색상 · 사진에서 찍기')
        self.local_point_note = QLabel('선택한 마스크 안에서 색을 고르세요.')
        self.local_points = QComboBox()
        self.local_points.currentIndexChanged.connect(self.load_local_point)
        self.local_point_controls = {}
        for key, title, lo, hi, scale in POINT_CONTROLS:
            c = Adjustment(key, title, lo, hi, scale)
            self.local_point_controls[key] = c
            c.changed.connect(self.local_point_changed)
            c.committed.connect(self.w.finish_interaction)
            c.default_value = {'range': .12, 'saturation_range': .6, 'lightness_range': .5}.get(key, 0)
        self.local_point_remove = QPushButton('부분 선택 색상 제거')
        self.local_point_preview = QCheckBox('부분 선택 색상 범위 표시')

    def mode(self, kind):
        """`StudioTools.mode`'s part for the local pick: the generation, the refusal, the view's tool, the status."""
        self.color_pick_generation += 1
        if kind == 'local_point_color' and self.current_layer() is None:
            self.w.statusBar().showMessage('먼저 마스크와 선택 영역을 추가하세요.')
            return
        self.w.view.tool_mode = kind
        if kind == 'local_point_color':
            self.w.statusBar().showMessage('선택한 마스크 안에서 보정할 색을 클릭하세요.')
        elif not kind:
            self.w.statusBar().showMessage('도구를 종료했습니다.')

    def preview_ready(self, *args):
        pass

    def load_layer(self):
        """`load_layer`'s fold-out part."""
        if self.layer_index >= len(self.w.settings['masks']):
            self.layer_index = len(self.w.settings['masks']) - 1
        self.loading = True
        self.load_local_curve()
        self.loading = False
        self.load_local_hsl()
        self.load_local_points()

    def select_layer(self, index):
        """`select_layer`."""
        self.color_pick_generation += 1
        self.overlay_paused = False
        self.w.commit()
        self.layer_index = index
        self.component_index = -1
        self.w.view.tool_mode = ''
        self.load_layer()


def slide(control, value):
    control.slider.setValue(round(value * control.scale))


def local_steps():
    def hsl(column, value):
        return lambda m: slide(m.local_hsl_controls[column], value)

    def point(key, value):
        return lambda m: slide(m.local_point_controls[key], value)

    def release_point(key):
        return lambda m: m.local_point_controls[key].committed.emit()

    def curve(points):
        return lambda m: m.local_curve.changed.emit(points)

    return [
        ('load', lambda m: m.load_layer()),
        ('curve_rgb', curve([[0.0, 0.0], [0.3, 0.4], [1.0, 1.0]])),
        ('curve_rgb_more', curve([[0.0, 0.05], [0.3, 0.45], [1.0, 0.95]])),
        ('curve_release', lambda m: m.local_curve.committed.emit()),
        ('curve_channel_blue', lambda m: m.local_curve_channel.setCurrentIndex(3)),
        ('curve_blue', curve([[0.0, 0.0], [0.5, 0.42], [1.0, 1.0]])),
        ('curve_blue_release', lambda m: m.local_curve.committed.emit()),
        ('curve_channel_red', lambda m: m.local_curve_channel.setCurrentIndex(1)),
        ('overlay_back', lambda m: setattr(m, 'overlay_paused', False)),
        ('hsl_channel_green', lambda m: m.local_hsl_channel.setCurrentIndex(3)),
        ('hsl_hue', hsl(0, 25.0)),
        ('hsl_lightness', hsl(2, -40.0)),
        ('hsl_release', lambda m: m.local_hsl_controls[2].committed.emit()),
        ('hsl_channel_red', lambda m: m.local_hsl_channel.setCurrentIndex(0)),
        ('hsl_saturation', hsl(1, 10.0)),
        ('hsl_same', hsl(1, 10.0)),
        ('hsl_reset', lambda m: m.local_hsl_controls[1].reset()),
        ('hsl_channel_green_again', lambda m: m.local_hsl_channel.setCurrentIndex(3)),
        ('pick_start', lambda m: m.mode('local_point_color')),
        ('pick', lambda m: m.pick_point_color([0.2, 0.5], local=True)),
        ('point_range', point('range', 0.2)),
        ('point_hue', point('hue', 30.0)),
        ('point_release', release_point('hue')),
        ('pick_far', lambda m: m.pick_point_color([0.95, 0.5], local=True)),
        ('pick_outside_photo', lambda m: m.pick_point_color([1.2, 0.5], local=True)),
        ('pick_second', lambda m: m.pick_point_color([0.1, 0.2], local=True)),
        ('select_first', lambda m: m.local_points.setCurrentIndex(0)),
        ('point_saturation_range', point('saturation_range', 0.35)),
        ('point_lightness', point('lightness', -20.0)),
        ('point_lightness_release', release_point('lightness')),
        ('point_reset_range', lambda m: m.local_point_controls['range'].reset()),
        ('remove_first', lambda m: m.remove_local_point()),
        ('layer_2', lambda m: m.select_layer(1)),
        ('layer_2_point', point('saturation', 15.0)),
        ('layer_2_release', release_point('saturation')),
        ('layer_2_pick', lambda m: m.pick_point_color([0.8, 0.6], local=True)),
        ('layer_2_pick_outside_mask', lambda m: m.pick_point_color([0.05, 0.05], local=True)),
        ('layer_2_hsl', hsl(0, -12.0)),
        ('layer_2_hsl_release', lambda m: m.local_hsl_controls[0].committed.emit()),
        ('layer_2_curve', curve([[0.0, 0.0], [0.6, 0.5], [1.0, 1.0]])),
        ('layer_2_curve_release', lambda m: m.local_curve.committed.emit()),
        ('layer_3', lambda m: m.select_layer(2)),
        ('layer_3_pick_disabled', lambda m: m.pick_point_color([0.5, 0.5], local=True)),
        ('layer_3_remove_nothing', lambda m: m.remove_local_point()),
        ('layer_1', lambda m: m.select_layer(0)),
        ('curve_channel_rgb', lambda m: m.local_curve_channel.setCurrentIndex(0)),
        ('remove_last', lambda m: m.remove_local_point()),
        ('remove_none', lambda m: m.remove_local_point()),
        ('no_layer', lambda m: m.select_layer(5)),
        ('masks_gone', lambda m: (m.w.settings.update(masks=[]), m.w.commit(), m.load_layer())),
        ('pick_start_refused', lambda m: m.mode('local_point_color')),
    ]


def widget_state(m):
    return dict(
        curve_channel=m.local_curve_channel.currentIndex(), curve=[list(p) for p in m.local_curve.points],
        hsl_channel=m.local_hsl_channel.currentIndex(),
        hsl=[dict(value=c.slider.value() / c.scale, enabled=c.isEnabled()) for c in m.local_hsl_controls],
        items=[m.local_points.itemText(i) for i in range(m.local_points.count())],
        current=m.local_points.currentIndex(), index=m.local_point_index,
        points={k: dict(value=c.slider.value() / c.scale, enabled=c.isEnabled())
                for k, c in m.local_point_controls.items()},
        add_enabled=m.local_point_add.isEnabled(), remove_enabled=m.local_point_remove.isEnabled(),
        preview_enabled=m.local_point_preview.isEnabled(), note=m.local_point_note.text())


with Case('app', 'masks3_local') as case:
    source = case.array('source', local_source())
    window = Window(normalized({**defaults(), **deepcopy(LOCAL_SETTINGS)}), source)
    studio = LocalStudio(window)
    records = []
    for step, call in local_steps():
        call(studio)
        records.append(dict(step=step, masks=deepcopy(window.settings['masks']), history=list(window.history),
                            layer=studio.layer_index, mode=window.view.tool_mode,
                            message=window.messages[-1] if window.messages else '',
                            overlay_paused=studio.overlay_paused, **widget_state(studio)))
    case.json(settings=LOCAL_SETTINGS, steps=records)


with Case('app', 'masks3_coverage') as case:
    source = local_source()     # the source of masks3_local
    final = normalized({**defaults(), **deepcopy(LOCAL_SETTINGS)})
    final['masks'][0]['adjustments']['local_curves'] = [[[0.0, 0.05], [0.3, 0.45], [1.0, 0.95]],
                                                        [[0.0, 0.0], [1.0, 1.0]], [[0.0, 0.0], [1.0, 1.0]],
                                                        [[0.0, 0.0], [0.5, 0.42], [1.0, 1.0]]]
    final['masks'][0]['adjustments']['point_colors'] = [
        dict(version=2, rgb=[0.55, 0.42, 0.3], range=0.2, saturation_range=0.6, lightness_range=0.5, hue=30,
             saturation=0, lightness=0),
        dict(rgb=[0.3, 0.45, 0.2], range=0.1, hue=-15, saturation=20, lightness=0)]
    targets = [(0, 0), (0, 1), (1, 0)]
    for layer, index in targets:
        case.array(f'coverage_{layer}_{index}',
                   point_color.preview(source, final, (layer, index)).astype(np.float32))
    samples = []
    for layer, position in [(0, [0.2, 0.5]), (0, [0.05, 0.9]), (1, [0.8, 0.6]), (0, [0.9, 0.5]), (1, [0.05, 0.05]), (2, [0.5, 0.5])]:
        try:
            samples.append(dict(layer=layer, point=position, rgb=point_color.sample(source, final, position, layer),
                                error=''))
        except ValueError as error:
            samples.append(dict(layer=layer, point=position, rgb=None, error=str(error)))
    case.json(settings=final, targets=targets, samples=samples)
