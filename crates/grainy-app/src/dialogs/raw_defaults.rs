//! The RAW import defaults box (RAW 가져오기 기본 보정): rules that give newly imported RAW photos their first
//! settings, for all RAW photos, one camera model or one camera body, made from the open photo's settings or a
//! preset, with chosen groups of settings and optionally ISO anchors (detail and blacks blended between ISOs).
//!
//! Ports luma/profile_dialog.py `RawDefaultsDialog` (`populate`, `persist`, `toggle`, `remove`, `save_rule`,
//! `apply_current`) on `grainy_raw::raw_defaults` (`make_rule`, `iso_anchors`, `resolve`, `GROUPS`). The rules are
//! the catalogue preference `raw_defaults_v1` (DCP profiles inside are stored once in `profile_assets` by
//! `grainy_catalog`); the import applies them (`CatalogDocument`'s import preview, `raw_defaults.import_preview`).
//!
//! The box's state is [`RawDefaults`], which works on a [`Host`] (the window, or a test's catalogue);
//! [`RawDefaultsDialog`] draws it.

use std::path::Path;

use eframe::egui;
use grainy_core::{normalized, Settings};
use grainy_raw::raw_defaults::{self, GROUPS};
use serde_json::{json, Value};

use super::Dialog;
use crate::context::AppContext;
use crate::text::tr;
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

/// Port of `raw_defaults.PREFERENCE`.
pub const PREFERENCE: &str = "raw_defaults_v1";

/// What the box needs from the window (`self.w` in the oracle).
pub trait Host {
    /// `w.current_id` (0: none).
    fn current_id(&self) -> u64;
    /// `w.catalog.photo(id)`: at least `id`, `path`, `info` and the stored `settings`.
    fn photo(&self, id: u64) -> Option<Value>;
    /// `w.settings`: the open photo's live settings.
    fn settings(&self) -> Settings;
    /// `w.all_presets[name]`.
    fn preset(&self, name: &str) -> Option<Value>;
    /// `w.source is not None`: the open photo is loaded.
    fn has_source(&self) -> bool;
    /// `w.catalog.save_preference(PREFERENCE, rules)`.
    fn save_rules(&mut self, rules: &Value) -> Result<(), String>;
    /// `w.commit(); w.settings = settings; w.commit(label)` and the refresh of the controls and thumbnail.
    fn apply(&mut self, settings: Settings, label: &str);
}

/// The oracle's Korean messages for the errors of `grainy_raw::raw_defaults` (`ValueError` texts of
/// luma/raw_defaults.py).
pub fn korean_error(error: &grainy_raw::Error, info: &Value) -> String {
    let text = error.to_string();
    let korean = match text.as_str() {
        "select at least two RAW photos with different ISO" => "서로 다른 ISO의 RAW 사진을 두 장 이상 선택하세요.",
        "ISO anchors must use the same camera" => "같은 카메라 모델의 사진을 선택하세요.",
        "invalid ISO anchor" | "ISO anchors must be RAW" => "ISO를 읽을 수 있는 RAW 사진만 선택하세요.",
        "invalid ISO adjustment" => "ISO 보정값이 유효하지 않습니다.",
        "conflicting adjustments at the same ISO" => {
            "같은 ISO에 서로 다른 보정값이 있습니다. 기준 사진을 한 장씩 선택하세요."
        }
        "at least two distinct ISO values required" => "서로 다른 ISO가 두 개 이상 필요합니다.",
        "invalid rule scope" => "기본값 적용 대상이 잘못되었습니다.",
        "missing camera model or serial" if raw_defaults::camera_key(info).is_empty() => {
            "카메라 모델을 읽을 수 없습니다."
        }
        "missing camera model or serial" => "이 RAW에서 바디 일련번호를 읽을 수 없습니다.",
        "invalid rule groups" => "저장할 보정 항목을 선택하세요.",
        "DCP camera mismatch" => "현재 카메라와 다른 DCP입니다.",
        _ => return text,
    };
    korean.to_owned()
}

/// The box's state (`RawDefaultsDialog`'s attributes and widgets' values).
#[derive(Debug, Clone)]
pub struct RawDefaults {
    /// `catalog.photo(current_id)`, `None` without an open photo.
    pub photo: Option<Value>,
    /// The rules being edited (the preference).
    pub rules: Vec<Value>,
    /// `w.selected_ids()` when the box opened.
    pub photo_ids: Vec<u64>,
    /// The scope choice: (text, value) and the chosen one.
    pub scopes: Vec<(String, &'static str)>,
    pub scope: usize,
    /// The settings choice: (text, preset name or `None` for the open photo) and the chosen one.
    pub sources: Vec<(String, Option<String>)>,
    pub source: usize,
    /// 규칙 이름.
    pub name: String,
    /// The group check boxes, in `GROUPS` order.
    pub groups: Vec<bool>,
    /// 선택한 사진으로 ISO별 보정 만들기.
    pub iso: bool,
    /// The status line.
    pub status: String,
    /// The list's current row.
    pub current: Option<usize>,
    /// A warning box to show (title, text).
    pub warning: Option<(String, String)>,
}

fn info_of(photo: &Option<Value>) -> Value {
    photo.as_ref().map(|p| p["info"].clone()).unwrap_or_else(|| json!({}))
}

fn path_of(photo: &Value) -> String {
    photo["path"].as_str().unwrap_or_default().to_owned()
}

impl RawDefaults {
    /// Port of `RawDefaultsDialog.__init__` (after its `w.commit()`): `presets` are the names of `w.all_presets`.
    pub fn new(host: &dyn Host, rules: Vec<Value>, photo_ids: Vec<u64>, presets: &[String]) -> Self {
        let current = host.current_id();
        let photo = if current != 0 { host.photo(current) } else { None };
        let info = info_of(&photo);
        let mut scopes = vec![(tr("모든 RAW"), "master")];
        let mut scope = 0;
        if let Some(p) = &photo {
            if raw_defaults::is_raw(&info, Some(Path::new(&path_of(p)))) && !raw_defaults::camera_key(&info).is_empty()
            {
                let camera = crate::commands::metadata::python_str(&info["camera"]);
                scopes.push((tr("이 카메라 모델 · ") + &camera, "model"));
                let serial = raw_defaults::serial(&info);
                if !serial.is_empty() {
                    scopes.push((tr("이 바디 · ") + &serial, "serial"));
                }
                scope = 1;
            }
        }
        let mut sources = vec![(tr("현재 사진의 보정"), None)];
        sources.extend(presets.iter().map(|name| (tr("프리셋 · ") + name, Some(name.clone()))));
        let source = if photo.is_none() { 1.min(sources.len() - 1) } else { 0 };
        let groups =
            GROUPS.iter().map(|(title, _)| ["RAW 프로파일", "화이트 밸런스", "디테일"].contains(title)).collect();
        Self {
            photo,
            rules,
            photo_ids,
            scopes,
            scope,
            sources,
            source,
            name: String::new(),
            groups,
            iso: false,
            status: tr("규칙은 이 카탈로그에 저장되며 백업에 포함됩니다."),
            current: None,
            warning: None,
        }
    }

    /// Port of `populate`'s item texts.
    pub fn items(&self) -> Vec<String> {
        self.rules
            .iter()
            .map(|r| {
                let enabled = r.get("enabled").is_none_or(grainy_core::settings::truthy);
                let groups: Vec<String> = r
                    .get("groups")
                    .and_then(Value::as_array)
                    .map(|g| g.iter().map(crate::commands::metadata::python_str).collect())
                    .unwrap_or_default();
                let iso = r.get("iso").and_then(Value::as_array).filter(|a| !a.is_empty());
                format!(
                    "{}{}  /  {}{}",
                    if enabled { "사용 · " } else { "해제 · " },
                    crate::commands::metadata::python_str(&r["name"]),
                    groups.join(", "),
                    iso.map(|a| format!(" · ISO {}개 기준", a.len())).unwrap_or_default()
                )
            })
            .collect()
    }

    /// Whether 현재 사진에 저장된 항목 적용 is enabled.
    pub fn apply_enabled(&self) -> bool {
        self.photo.as_ref().is_some_and(|p| raw_defaults::is_raw(&p["info"], Some(Path::new(&path_of(p)))))
    }

    /// Port of `persist` (`populate` keeps no selection: the list is rebuilt).
    fn persist(&mut self, host: &mut dyn Host) {
        if let Err(message) = host.save_rules(&Value::Array(self.rules.clone())) {
            self.warning = Some((tr("기본 보정"), message));
        }
        self.current = None;
    }

    fn current_id_of_list(&self) -> Option<Value> {
        self.current.and_then(|i| self.rules.get(i)).map(|r| r["id"].clone())
    }

    /// Port of `toggle`: the selected rule on or off.
    pub fn toggle(&mut self, host: &mut dyn Host) {
        let Some(id) = self.current_id_of_list() else { return };
        for rule in &mut self.rules {
            if rule["id"] == id {
                let enabled = rule.get("enabled").is_none_or(grainy_core::settings::truthy);
                rule["enabled"] = Value::Bool(!enabled);
            }
        }
        self.persist(host);
    }

    /// Port of `remove`: the selected rule deleted.
    pub fn remove(&mut self, host: &mut dyn Host) {
        let Some(id) = self.current_id_of_list() else { return };
        self.rules.retain(|r| r["id"] != id);
        self.persist(host);
    }

    /// Port of `save_rule`: a rule for the chosen scope from the chosen settings and groups (with ISO anchors from
    /// the selected photos when asked) replaces the rule of the same target.
    pub fn save_rule(&mut self, host: &mut dyn Host) {
        let preset = self.sources.get(self.source).and_then(|(_, name)| name.clone());
        let settings = match &preset {
            Some(name) => host.preset(name).map(|v| match v {
                Value::Object(map) => Settings(map),
                _ => Settings(Default::default()),
            }),
            None => {
                self.photo.as_ref().filter(|p| p["id"].as_u64() == Some(host.current_id())).map(|_| host.settings())
            }
        };
        let Some(settings) = settings else { return };
        let info = info_of(&self.photo);
        let scope = self.scopes.get(self.scope).map_or("master", |s| s.1);
        let made = (|| -> Result<Value, String> {
            if self.iso && self.photo_ids.len() > 64 {
                return Err("ISO 기준 사진은 2–64장으로 선택하세요.".to_owned());
            }
            let photos: Vec<Value> =
                if self.iso { self.photo_ids.iter().filter_map(|id| host.photo(*id)).collect() } else { Vec::new() };
            let anchors = if self.iso {
                raw_defaults::iso_anchors(&photos).map_err(|e| korean_error(&e, &info))?
            } else {
                Vec::new()
            };
            if !anchors.is_empty()
                && scope == "serial"
                && photos.iter().any(|p| raw_defaults::serial(&p["info"]) != raw_defaults::serial(&info))
            {
                return Err("바디별 ISO 기본값에는 같은 일련번호의 사진만 선택하세요.".to_owned());
            }
            let groups: Vec<String> =
                GROUPS.iter().zip(&self.groups).filter(|(_, on)| **on).map(|((g, _), _)| (*g).to_owned()).collect();
            raw_defaults::make_rule(&info, &settings, scope, Some(&groups), &anchors, python_strip(&self.name))
                .map_err(|e| korean_error(&e, &info))
        })();
        let rule = match made {
            Ok(rule) => rule,
            Err(message) => {
                self.warning = Some((tr("기본 보정"), message));
                return;
            }
        };
        self.rules.retain(|old| old["id"] != rule["id"]);
        let name = crate::commands::metadata::python_str(&rule["name"]);
        self.rules.push(rule);
        self.persist(host);
        self.status = name + &tr(" 저장 완료 · 다음에 새로 가져오는 RAW부터 적용됩니다.");
    }

    /// Port of `apply_current`: the rules resolved for the open photo (over its settings) as one undo step.
    pub fn apply_current(&mut self, host: &mut dyn Host) {
        let Some(photo) = self.photo.clone() else { return };
        if photo["id"].as_u64() != Some(host.current_id()) || !host.has_source() {
            return;
        }
        let base = host.settings();
        let path = path_of(&photo);
        let resolved = raw_defaults::resolve(&photo["info"], &self.rules, Some(Path::new(&path)), Some(&base));
        let (settings, notes) = match resolved {
            Ok(done) => done,
            Err(error) => {
                self.warning = Some((tr("기본 보정"), korean_error(&error, &photo["info"])));
                return;
            }
        };
        if notes.is_empty() {
            self.status = tr("현재 사진에 적용할 규칙이 없습니다.");
            return;
        }
        host.apply(settings, "RAW 기본 보정 적용");
        self.status = notes.join(" · ") + &tr(" · 실행 취소 가능");
    }
}

/// `str.strip()`.
fn python_strip(text: &str) -> &str {
    grainy_adobe::pyutil::strip(text)
}

// ---- the window -------------------------------------------------------------------------------------------------

/// The window as the box's [`Host`].
struct Window<'a>(&'a mut AppContext);

impl Host for Window<'_> {
    fn current_id(&self) -> u64 {
        self.0.doc.current_id()
    }

    fn photo(&self, id: u64) -> Option<Value> {
        let index = self.0.doc.index_of(id)?;
        let path = self.0.doc.photos().get(index)?.path.clone();
        let settings = self.0.doc.stored_settings_of(id).ok()?;
        Some(json!({
            "id": id,
            "path": path.to_string_lossy(),
            "info": self.0.doc.photo_info(id),
            "settings": Value::Object(settings.0),
        }))
    }

    fn settings(&self) -> Settings {
        self.0.settings().clone()
    }

    fn preset(&self, name: &str) -> Option<Value> {
        let user =
            &self.0.state_ref::<crate::commands::presets::PresetState>().map(|s| s.user.clone()).unwrap_or_default();
        crate::commands::presets::all_presets(user).into_iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    fn has_source(&self) -> bool {
        self.0.doc.current().is_some()
    }

    fn save_rules(&mut self, rules: &Value) -> Result<(), String> {
        self.0.doc.save_preference(PREFERENCE, rules)
    }

    fn apply(&mut self, settings: Settings, label: &str) {
        self.0.doc.edit_mut().replace(normalized(&settings.0), label);
        self.0.before = false;
    }
}

/// The names of `w.all_presets` (built-in, then the library's).
fn preset_names(cx: &mut AppContext) -> Vec<String> {
    crate::commands::presets::ensure_loaded(cx);
    let user = cx.state::<crate::commands::presets::PresetState>().user.clone();
    crate::commands::presets::all_presets(&user).into_iter().map(|(n, _)| n).collect()
}

/// The RAW import defaults window.
pub struct RawDefaultsDialog {
    pub model: RawDefaults,
}

impl RawDefaultsDialog {
    /// Opens the box: the open photo's pending change is stored first (`w.commit()`).
    pub fn open(cx: &mut AppContext) -> Self {
        cx.doc.edit_mut().commit("보정 변경");
        let rules = cx.doc.preference(PREFERENCE).and_then(|v| v.as_array().cloned()).unwrap_or_default();
        let ids: Vec<u64> =
            cx.doc.selected_indices().iter().filter_map(|i| cx.doc.photos().get(*i)).map(|p| p.id).collect();
        let ids = if ids.is_empty() && cx.doc.current_id() != 0 { vec![cx.doc.current_id()] } else { ids };
        let presets = preset_names(cx);
        let model = RawDefaults::new(&Window(cx), rules, ids, &presets);
        Self { model }
    }

    /// 사용 / 해제, 선택 규칙 삭제, 저장, 적용: the model on the window.
    pub fn toggle(&mut self, cx: &mut AppContext) {
        self.model.toggle(&mut Window(cx));
    }

    pub fn remove(&mut self, cx: &mut AppContext) {
        self.model.remove(&mut Window(cx));
    }

    pub fn save_rule(&mut self, cx: &mut AppContext) {
        self.model.save_rule(&mut Window(cx));
    }

    pub fn apply_current(&mut self, cx: &mut AppContext) {
        self.model.apply_current(&mut Window(cx));
    }

    /// The preset choices follow the library's presets once they are read (the chosen one stays chosen).
    fn follow_presets(&mut self, cx: &mut AppContext) {
        let names = preset_names(cx);
        let current: Vec<&String> = self.model.sources.iter().filter_map(|(_, n)| n.as_ref()).collect();
        if current.len() == names.len() && current.iter().zip(&names).all(|(a, b)| *a == b) {
            return;
        }
        let chosen = self.model.sources.get(self.model.source).and_then(|(_, n)| n.clone());
        let mut sources = vec![(tr("현재 사진의 보정"), None)];
        sources.extend(names.iter().map(|name| (tr("프리셋 · ") + name, Some(name.clone()))));
        self.model.source = chosen.and_then(|c| sources.iter().position(|(_, n)| n.as_ref() == Some(&c))).unwrap_or(0);
        self.model.sources = sources;
    }
}

impl Dialog for RawDefaultsDialog {
    fn id(&self) -> &'static str {
        "raw_defaults"
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        self.follow_presets(cx);
        if let Some((title, text)) = self.model.warning.take() {
            cx.open_dialog(Box::new(super::import_errors::Notice { id: "raw_warning", title, text: tr(&text) }));
        }
        let mut close = false;
        let mut action: Option<fn(&mut Self, &mut AppContext)> = None;
        let response = egui::Modal::new(egui::Id::unique("raw_defaults")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(700.0);
            ui.label(egui::RichText::new(tr("RAW 가져오기 기본 보정")).font(t::bold(14.0)).color(t::BRAND));
            ui.add_space(6.0);
            widgets::label(ui, &tr("새로 가져오는 RAW의 기본 보정"), t::TEXT, t::FONT_SIZE);
            widgets::muted(
                ui,
                &tr("바디 일련번호 → 카메라 모델 → 모든 RAW 순서로 우선 적용합니다. 저장해도 기존 사진은 바뀌지 않습니다."),
            );
            ui.add_space(4.0);
            let items = self.model.items();
            let mut clicked = None;
            egui::Frame::new().fill(t::LIST_BG).stroke(egui::Stroke::new(1.0, t::INPUT_BORDER)).show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("raw_rules")
                    .min_scrolled_height(150.0)
                    .max_height(150.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (index, item) in items.iter().enumerate() {
                            if ui.selectable_label(self.model.current == Some(index), item).clicked() {
                                clicked = Some(index);
                            }
                        }
                    });
            });
            if let Some(index) = clicked {
                self.model.current = Some(index);
            }
            ui.add_space(4.0);
            let labels = [tr("사용 / 해제"), tr("선택 규칙 삭제")];
            match widgets::button_row(ui, &[Button::new(&labels[0]), Button::new(&labels[1])], ui.available_width(), 6.0) {
                Some(0) => action = Some(Self::toggle),
                Some(_) => action = Some(Self::remove),
                None => {}
            }
            ui.add_space(4.0);
            let width = ui.available_width();
            let names: Vec<String> = self.model.scopes.iter().map(|(n, _)| n.clone()).collect();
            if let Some(i) = crate::panels::controls::choice(ui, "raw_defaults_scope", "", &names, self.model.scope, width) {
                self.model.scope = i;
            }
            let names: Vec<String> = self.model.sources.iter().map(|(n, _)| n.clone()).collect();
            if let Some(i) = crate::panels::controls::choice(ui, "raw_defaults_source", "", &names, self.model.source, width)
            {
                self.model.source = i;
            }
            widgets::line_edit(ui, "raw_defaults_name", &mut self.model.name, &tr("규칙 이름 (선택 사항)"));
            egui::Grid::new("raw_defaults_groups").num_columns(3).min_col_width(width / 3.0 - 8.0).show(ui, |ui| {
                for (i, (title, _)) in GROUPS.iter().enumerate() {
                    widgets::checkbox(ui, &mut self.model.groups[i], &tr(title));
                    if i % 3 == 2 {
                        ui.end_row();
                    }
                }
            });
            widgets::checkbox(ui, &mut self.model.iso, &tr("선택한 사진으로 ISO별 보정 만들기"));
            widgets::muted(ui, &tr("같은 카메라에서 서로 다른 ISO로 촬영한 기준 RAW 2–64장을 선택하세요. 노이즈 제거·샤프닝·선명도·텍스처·안개 제거·검정값을 ISO 사이에서 연결합니다. 크롭·마스크·복구는 저장하지 않습니다."));
            if Button::new(&tr("선택한 대상으로 저장 / 교체")).width(width).show(ui).clicked() {
                action = Some(Self::save_rule);
            }
            widgets::muted(ui, &self.model.status);
            ui.add_space(6.0);
            let labels = [tr("현재 사진에 저장된 항목 적용"), tr("닫기")];
            let row = [
                Button::new(&labels[0]).enabled(self.model.apply_enabled()),
                Button::new(&labels[1]).kind(Kind::Primary),
            ];
            match widgets::button_row(ui, &row, width, 6.0) {
                Some(0) => action = Some(Self::apply_current),
                Some(_) => close = true,
                None => {}
            }
        });
        if let Some(action) = action {
            action(self, cx);
        }
        !(close || response.should_close())
    }
}
