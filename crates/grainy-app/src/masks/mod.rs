//! Local adjustment masks (마스크): the layer model as the settings key `masks` stores it, the gradients drawn on the
//! photo and the red coverage overlay. Port of the mask part of luma/studio.py `StudioTools` (`build_masks` and the
//! methods it calls) and of the gradient handles of luma/widgets.py `PhotoView`.
//!
//! * [`layers`]: the layer list as JSON values, with the oracle's edits (new layer, add a component with its
//!   brush range, duplicate, move, remove, enable, layer properties and adjustments).
//! * [`gradient`]: linear and radial gradients between the screen and the source (`gradient_shape`,
//!   `radial_from_screen`, `radial_on_screen`, `component_handles`, `move_component_handles`) and the handle hit
//!   test and drag of `PhotoView`.
//! * [`actions`]: the `StudioTools` methods on the [`AppContext`] (create / extend a mask, the tool mode, strokes
//!   of gradients, the handles, the panel's edits), with the oracle's commits.
//! * [`overlay`]: the red overlay of the selected mask, computed on a background job with grainy-local's
//!   `layer_mask` through the engine's geometry.
//!
//! The panel is `panels/masks.rs`, the canvas tool `tools/masks.rs`, the `mask.*` commands `commands/masks.rs`.
//!
//! **Extension points.** Work package 9 (brush, colour and luminance ranges, sky editor) adds its canvas tools
//! (`tools/brush.rs`, `tools/range_pick.rs`) *before* the `masks` line of `tools::registry()`, active while
//! [`MaskState::mode`] is [`Mode::Brush`] or one of the range modes, and draws the overlay with
//! [`overlay::paint`]; strokes go through [`actions::add_component`] (which already applies the brush range,
//! `ranged`) and read the tool options in [`MaskState::options`]. Its sub-body `panels/masks_ranges.rs` is called
//! where `panels/masks.rs` says so. Image-dependent layers (ranges, sky) need the developed image for their
//! overlay: [`overlay::OverlayInput::image`]. Work package 10 adds `panels/masks_local.rs` (the fold-out
//! 커브 · 색상 혼합 · 선택 색상, which shows a placeholder until then) and the component dialog, opened by
//! [`actions::edit_component`].

pub mod actions;
pub mod gradient;
pub mod layers;
pub mod overlay;

use crate::imagespace::CoordinateMap;

/// The mask kinds in the menus' order (`None` = separator). Port of `StudioTools.MASK_KINDS`.
pub const MASK_KINDS: [(Option<&str>, &str); 8] = [
    (Some("sky"), "하늘 선택"),
    (None, ""),
    (Some("brush"), "브러시"),
    (Some("linear"), "선형 그레이디언트"),
    (Some("radial"), "방사형 그레이디언트"),
    (None, ""),
    (Some("color_range"), "색상 범위"),
    (Some("luma_range"), "광도 범위"),
];

/// A brush range: (key, title, luminance span).
pub type BrushRange = (&'static str, &'static str, Option<(f64, f64)>);

/// The tones a brushed mask can be limited to. Port of `StudioTools.BRUSH_RANGES`.
pub const BRUSH_RANGES: [BrushRange; 4] = [
    ("all", "전체", None),
    ("shadows", "어두운 곳", Some((0.0, 0.2))),
    ("midtones", "중간 밝기", Some((0.45, 0.55))),
    ("highlights", "밝은 곳", Some((0.8, 1.0))),
];

/// Port of `StudioTools.BRUSH_RANGE_SOFTNESS`.
pub const BRUSH_RANGE_SOFTNESS: f64 = 0.25;

/// Component names in the mask list. Port of luma/mask_dialog.py `NAMES`.
pub const NAMES: [(&str, &str); 6] = [
    ("brush", "브러시"),
    ("linear", "선형 그레이디언트"),
    ("radial", "방사형 그레이디언트"),
    ("luma", "광도 범위"),
    ("color", "색상 범위"),
    ("sky", "하늘"),
];

/// The Korean name of a component kind (`NAMES.get(kind, '선택 영역')`).
pub fn kind_name(kind: &str) -> &'static str {
    NAMES.iter().find(|(k, _)| *k == kind).map_or("선택 영역", |(_, n)| *n)
}

/// Sky detection defaults (luma/sky_mask.py `DEFAULTS`).
pub const SKY_DEFAULTS: [(&str, f64); 2] = [("sensitivity", 50.0), ("feather", 30.0)];

/// The view's tool mode while the Masks tool is open (`PhotoView.tool_mode`, mask values only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// No tool: the view pans.
    #[default]
    None,
    Brush,
    Linear,
    Radial,
    ColorRange,
    ColorRangeAdd,
    LumaRange,
    /// The handles of the selected gradient (`edit_component`).
    EditComponent,
}

impl Mode {
    /// The oracle's name of the mode.
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::None => "",
            Mode::Brush => "brush",
            Mode::Linear => "linear",
            Mode::Radial => "radial",
            Mode::ColorRange => "color_range",
            Mode::ColorRangeAdd => "color_range_add",
            Mode::LumaRange => "luma_range",
            Mode::EditComponent => "edit_component",
        }
    }

    /// The mode of a kind name (`''` and unknown names: none).
    pub fn from_kind(kind: &str) -> Mode {
        match kind {
            "brush" => Mode::Brush,
            "linear" => Mode::Linear,
            "radial" => Mode::Radial,
            "color_range" => Mode::ColorRange,
            "color_range_add" => Mode::ColorRangeAdd,
            "luma_range" => Mode::LumaRange,
            "edit_component" => Mode::EditComponent,
            _ => Mode::None,
        }
    }

    /// Whether the mode draws a gradient (`gradient_tool` without the handles).
    pub fn gradient(self) -> bool {
        matches!(self, Mode::Linear | Mode::Radial)
    }

    /// The modes with a 완료 button and that bring the paused overlay back.
    pub fn drawing(self) -> bool {
        !matches!(self, Mode::None)
    }

    /// The range-picking modes (work package 9).
    pub fn picking(self) -> bool {
        matches!(self, Mode::ColorRange | Mode::ColorRangeAdd | Mode::LumaRange)
    }
}

/// The operation a new component combines with (the hidden `operation` combo box).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Operation {
    #[default]
    Add,
    Subtract,
    Intersect,
}

impl Operation {
    /// The stored value.
    pub fn as_str(self) -> &'static str {
        match self {
            Operation::Add => "add",
            Operation::Subtract => "subtract",
            Operation::Intersect => "intersect",
        }
    }

    /// From the stored value (unknown: add).
    pub fn parse(text: &str) -> Operation {
        match text {
            "subtract" => Operation::Subtract,
            "intersect" => Operation::Intersect,
            _ => Operation::Add,
        }
    }

    /// The Korean verb of the tool hint (`추가`, `빼기`, `교차`).
    pub fn verb(self) -> &'static str {
        match self {
            Operation::Add => "추가",
            Operation::Subtract => "빼기",
            Operation::Intersect => "교차",
        }
    }
}

/// The tool options (`도구 옵션`) as slider positions, as the oracle reads them (`slider.value()`).
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOptions {
    /// 크기: thousandths of the short side (`radius` 0.002–0.25, scale 1000).
    pub radius: i64,
    /// 페더 0–100.
    pub feather: i64,
    /// 플로우 1–100.
    pub flow: i64,
    /// 밀도 1–100.
    pub density: i64,
    /// 밝기 제한: a key of [`BRUSH_RANGES`].
    pub brush_range: &'static str,
    /// 자동 마스크.
    pub auto_mask: bool,
    /// 자동 마스크 허용량: hundredths (0.02–1, scale 100).
    pub auto_tolerance: i64,
    /// 다듬기 of a colour range pick: hundredths (0.02–1.7, scale 100); work package 9.
    pub tolerance: i64,
}

impl Default for ToolOptions {
    fn default() -> Self {
        Self {
            radius: 25,
            feather: 70,
            flow: 100,
            density: 100,
            brush_range: "all",
            auto_mask: false,
            auto_tolerance: 18,
            tolerance: 30,
        }
    }
}

/// A gradient being drawn (`pending_stroke` of a gradient): the settings' masks before it, the layer it goes to
/// and the component as made at the press.
#[derive(Debug, Clone)]
pub struct Pending {
    pub photo: u64,
    pub base: serde_json::Value,
    pub layer: isize,
    pub component: serde_json::Map<String, serde_json::Value>,
}

/// A handle being dragged: (index, the pointer where it was pressed, the handles before).
#[derive(Debug, Clone, PartialEq)]
pub struct HandleDrag {
    pub index: usize,
    pub start: [f64; 2],
    pub original: Vec<[f64; 2]>,
}

/// What the Masks tool keeps between frames (`StudioTools`' mask attributes and the view's tool state), stored in
/// the context with `cx.state::<MaskState>()`.
pub struct MaskState {
    /// The photo the selection belongs to (another photo starts again at its first mask).
    pub photo: u64,
    /// `layer_index` (−1 when there is no mask).
    pub layer: isize,
    /// `component_index` (−1: the layer itself).
    pub component: isize,
    /// `fresh_layer`: a mask made by New Mask that has no component yet.
    pub fresh_layer: Option<isize>,
    /// The operation of the next component.
    pub operation: Operation,
    pub mode: Mode,
    pub options: ToolOptions,
    /// 오버레이 표시 (O); on by default.
    pub overlay_off: bool,
    /// The overlay steps aside while the mask's sliders move.
    pub overlay_paused: bool,
    /// `tool_points` of the gradient being drawn (frame fractions, off the photo too).
    pub points: Vec<[f64; 2]>,
    pub pending: Option<Pending>,
    /// The gradient's handles on screen (frame fractions) while editing it.
    pub handles: Vec<[f64; 2]>,
    /// `handle_kind`.
    pub handle_kind: &'static str,
    /// `handle_angle`: a turned radial's angle on screen, degrees.
    pub handle_angle: f64,
    pub handle_drag: Option<HandleDrag>,
    /// The coordinate map and the key it was built for (`coordinate_key`).
    pub(crate) map: Option<(String, std::sync::Arc<CoordinateMap>)>,
    /// The overlay on screen and what is being computed.
    pub(crate) overlay: overlay::OverlayState,
    /// The name being typed in the rename box (`rename_layer`), when open.
    pub rename: Option<String>,
    /// The frame the panel was last drawn (leaving the tool ends its mode).
    pub(crate) seen: u64,
}

impl Default for MaskState {
    /// The oracle's start: `layer_index = 0`, `component_index = -1`, overlay on.
    fn default() -> Self {
        Self {
            photo: 0,
            layer: 0,
            component: -1,
            fresh_layer: None,
            operation: Operation::Add,
            mode: Mode::None,
            options: ToolOptions::default(),
            overlay_off: false,
            overlay_paused: false,
            points: Vec::new(),
            pending: None,
            handles: Vec::new(),
            handle_kind: "linear",
            handle_angle: 0.0,
            handle_drag: None,
            map: None,
            overlay: overlay::OverlayState::default(),
            rename: None,
            seen: 0,
        }
    }
}

impl MaskState {
    /// Whether an overlay picture is ready to show (computed for the current mask).
    pub fn overlay_ready(&self) -> bool {
        self.overlay.texture.is_some()
    }

    /// Whether the overlay check box is on.
    pub fn overlay_shown(&self) -> bool {
        !self.overlay_off
    }
}
