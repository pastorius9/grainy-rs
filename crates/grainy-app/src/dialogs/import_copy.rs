//! 사진 복사해서 가져오기 (luma/import_copy.py `dialog`): where to copy from (a card or another folder) and to,
//! dated subfolders, the file-name rule, a verified second copy, keywords, a develop preset and whether suspected
//! duplicates are left out. [`Form`] is the dialog's model (its fields, the checks of the oracle's `while True`
//! loop and the saved `import_copy` preference); [`ImportCopyDialog`] draws it; [`CopyProgress`] is the
//! `QProgressDialog` of the copy job with its 취소. The job itself is in `commands/import.rs`.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui::{self, Ui, Vec2};
use grainy_catalog::folders::Platform;
use grainy_library::pathx;
use serde_json::{json, Value};

use super::import_errors::Notice;
use super::Dialog;
use crate::context::AppContext;
use crate::jobs::JobId;
use crate::text::tr;
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

/// The choices of 정리 in the oracle's order: `(key, label)`.
pub const SUBFOLDERS: [(&str, &str); 3] =
    [("year/date", "연도/날짜 폴더 (2026/2026-09-25)"), ("date", "날짜 폴더 (2026-09-25)"), ("none", "하위 폴더 없음")];

/// The dialog's fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Form {
    /// 가져올 곳 (카드·폴더).
    pub source: String,
    /// 복사할 곳.
    pub destination: String,
    /// 정리: a key of [`SUBFOLDERS`].
    pub subfolders: String,
    /// 파일 이름 (the name template).
    pub template: String,
    /// 두 번째 사본(백업)도 만들기.
    pub use_backup: bool,
    /// 백업 위치.
    pub backup: String,
    /// 키워드 (comma separated).
    pub keywords: String,
    /// 현상 프리셋 (`""`: 없음).
    pub preset: String,
    /// 중복으로 보이는 사진은 가져오지 않기.
    pub skip: bool,
}

impl Default for Form {
    fn default() -> Self {
        Self::from_saved(&Value::Null)
    }
}

/// What the copies get when they are imported (`import_extras`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Extras {
    /// `keywords.text().strip()`.
    pub keywords: String,
    /// The preset's name (`""`: none).
    pub preset: String,
}

impl Form {
    /// The fields the dialog opens with: the saved `import_copy` preference (destination, subfolders, template,
    /// backup); the rest empty, 중복 … 가져오지 않기 on. An unknown saved subfolder choice shows the first one
    /// (`setCurrentIndex(max(0, findData(...)))`).
    pub fn from_saved(saved: &Value) -> Self {
        let text = |key: &str, default: &str| saved.get(key).and_then(Value::as_str).unwrap_or(default).to_owned();
        let subfolders = text("subfolders", "year/date");
        let subfolders =
            if SUBFOLDERS.iter().any(|(k, _)| *k == subfolders) { subfolders } else { SUBFOLDERS[0].0.to_owned() };
        Self {
            source: String::new(),
            destination: text("destination", ""),
            subfolders,
            template: text("template", "{stem}"),
            use_backup: false,
            backup: text("backup", ""),
            keywords: String::new(),
            preset: String::new(),
            skip: true,
        }
    }

    /// `str(Path(source.text()))`.
    pub fn source_folder(&self) -> String {
        pathx::norm_str(Platform::native(), &self.source)
    }

    /// `str(Path(destination.text()))`.
    pub fn destination_folder(&self) -> String {
        pathx::norm_str(Platform::native(), &self.destination)
    }

    /// The backup folder when a second copy is asked for (`backup.text() if use_backup.isChecked() else None`).
    pub fn backup_folder(&self) -> Option<String> {
        self.use_backup.then(|| self.backup.clone())
    }

    /// The keywords and preset the copies get.
    pub fn extras(&self) -> Extras {
        Extras { keywords: self.keywords.trim().to_owned(), preset: self.preset.clone() }
    }

    /// The checks after 복사하고 가져오기 (the oracle's `while True` loop): the message of the first problem, or
    /// `None` when the copy can start. An empty source field is refused here; the oracle reads it as `Path('')`,
    /// the current folder, and would copy from there.
    pub fn problem(&self) -> Option<String> {
        let platform = Platform::native();
        if let Err(error) = grainy_library::import_copy::check_template(&self.template) {
            return Some(error.to_string());
        }
        let source = self.source_folder();
        if self.source.trim().is_empty() || !std::path::Path::new(&source).is_dir() {
            return Some(tr("가져올 폴더를 선택하세요."));
        }
        if self.destination.trim().is_empty() {
            return Some(tr("복사할 폴더를 선택하세요."));
        }
        let (src, dst) = (pathx::resolve(platform, &source), pathx::resolve(platform, &self.destination_folder()));
        if pathx::same(platform, &dst, &src) || pathx::is_parent_of(platform, &src, &dst) {
            return Some(tr("복사할 곳은 가져올 폴더 밖이어야 합니다."));
        }
        if self.use_backup
            && (self.backup.trim().is_empty() || pathx::same(platform, &pathx::resolve(platform, &self.backup), &dst))
        {
            return Some(tr("백업 위치는 복사할 곳과 다른 폴더여야 합니다."));
        }
        None
    }

    /// The `import_copy` preference saved when the copy starts.
    pub fn saved(&self) -> Value {
        json!({
            "destination": self.destination_folder(),
            "subfolders": self.subfolders,
            "template": self.template,
            "backup": self.backup,
        })
    }
}

/// Which folder field a folder dialog fills.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Source,
    Destination,
    Backup,
}

/// The dialog.
pub struct ImportCopyDialog {
    pub form: Form,
    picking: Option<(Field, Receiver<Option<PathBuf>>)>,
}

impl ImportCopyDialog {
    /// The dialog with `form`'s fields.
    pub fn new(form: Form) -> Self {
        Self { form, picking: None }
    }

    fn pick(&mut self, field: Field) {
        let (title, start) = match field {
            Field::Source => ("가져올 폴더", &self.form.source),
            Field::Destination => ("복사할 폴더", &self.form.destination),
            Field::Backup => ("백업 폴더", &self.form.backup),
        };
        let (title, start) = (tr(title), PathBuf::from(start.trim()));
        let (sender, receiver) = channel();
        let work = move || {
            let mut dialog = rfd::AsyncFileDialog::new().set_title(&title);
            if start.is_dir() {
                dialog = dialog.set_directory(&start);
            }
            let chosen = pollster::block_on(dialog.pick_folder());
            let _ = sender.send(chosen.map(|f| f.path().to_path_buf()));
        };
        match std::thread::Builder::new().name("grainy-dialog".into()).spawn(work) {
            Ok(_) => self.picking = Some((field, receiver)),
            Err(error) => log::error!("cannot open the folder dialog: {error}"),
        }
    }

    /// A folder dialog answered (`getExistingDirectory(...) or edit.text()`).
    fn receive_folder(&mut self, ctx: &egui::Context) {
        let Some((field, receiver)) = &self.picking else { return };
        match receiver.try_recv() {
            Ok(answer) => {
                if let Some(folder) = answer {
                    let text = pathx::text(&folder);
                    match field {
                        Field::Source => self.form.source = text,
                        Field::Destination => self.form.destination = text,
                        Field::Backup => self.form.backup = text,
                    }
                }
                self.picking = None;
            }
            Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
            Err(TryRecvError::Disconnected) => self.picking = None,
        }
    }
}

const LABEL_WIDTH: f32 = 150.0;
const FIELD_WIDTH: f32 = 380.0;

fn row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(LABEL_WIDTH, 28.0), egui::Sense::hover());
        widgets::paint_left(ui, rect, label, t::font(t::FONT_SIZE), t::TEXT);
        add(ui);
    });
}

fn folder_row(ui: &mut Ui, label: &str, text: &mut String, clicked: &mut bool) {
    row(ui, label, |ui| {
        let label = label.to_owned();
        ui.add_sized(Vec2::new(FIELD_WIDTH - 96.0, 26.0), egui::TextEdit::singleline(text))
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, &label));
        if widgets::button(ui, &tr("폴더 선택")).clicked() {
            *clicked = true;
        }
    });
}

/// The form; returns Some(true) for 복사하고 가져오기, Some(false) for 취소, and the folder button clicked.
fn form_ui(ui: &mut Ui, form: &mut Form, presets: &[String]) -> (Option<bool>, Option<Field>) {
    let mut pick = None;
    let (mut source, mut destination, mut backup) = (false, false, false);
    folder_row(ui, &tr("가져올 곳 (카드·폴더)"), &mut form.source, &mut source);
    folder_row(ui, &tr("복사할 곳"), &mut form.destination, &mut destination);
    row(ui, &tr("정리"), |ui| {
        let names: Vec<String> = SUBFOLDERS.iter().map(|(_, label)| tr(label)).collect();
        let current = SUBFOLDERS.iter().position(|(k, _)| *k == form.subfolders).unwrap_or(0);
        if let Some(i) =
            crate::panels::controls::choice(ui, "import_copy_subfolders", &tr("정리"), &names, current, FIELD_WIDTH)
        {
            form.subfolders = SUBFOLDERS[i].0.to_owned();
        }
    });
    row(ui, &tr("파일 이름"), |ui| {
        let label = tr("파일 이름");
        ui.add_sized(Vec2::new(FIELD_WIDTH, 26.0), egui::TextEdit::singleline(&mut form.template))
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, &label));
    });
    widgets::muted(ui, &tr("{stem} 원래 이름 · {date} 촬영일 · {time} 촬영 시각 · {n:04d} 순번 · {camera} 카메라"));
    widgets::checkbox(ui, &mut form.use_backup, &tr("두 번째 사본(백업)도 만들기"));
    folder_row(ui, &tr("백업 위치"), &mut form.backup, &mut backup);
    row(ui, &tr("키워드"), |ui| {
        ui.add_sized(
            Vec2::new(FIELD_WIDTH, 26.0),
            egui::TextEdit::singleline(&mut form.keywords).hint_text(tr("쉼표로 구분")),
        );
    });
    row(ui, &tr("현상 프리셋"), |ui| {
        let mut names = vec![tr("없음")];
        names.extend(presets.iter().cloned());
        let current = presets.iter().position(|p| *p == form.preset).map_or(0, |i| i + 1);
        if let Some(i) =
            crate::panels::controls::choice(ui, "import_copy_preset", &tr("현상 프리셋"), &names, current, FIELD_WIDTH)
        {
            form.preset = if i == 0 { String::new() } else { presets[i - 1].clone() };
        }
    });
    widgets::checkbox(ui, &mut form.skip, &tr("중복으로 보이는 사진은 가져오지 않기 (같은 이름·촬영 시각)"));
    ui.add_space(6.0);
    let mut result = None;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
        if widgets::button(ui, &tr("취소")).clicked() {
            result = Some(false);
        }
        ui.add_space(6.0);
        if Button::new(&tr("복사하고 가져오기")).kind(Kind::Primary).show(ui).clicked() {
            result = Some(true);
        }
    });
    if source {
        pick = Some(Field::Source);
    } else if destination {
        pick = Some(Field::Destination);
    } else if backup {
        pick = Some(Field::Backup);
    }
    (result, pick)
}

impl Dialog for ImportCopyDialog {
    fn id(&self) -> &'static str {
        "import_copy"
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        self.receive_folder(ctx);
        let presets = crate::commands::import::preset_names(cx);
        let mut action = None;
        let mut pick = None;
        let response = egui::Modal::new(egui::Id::unique("import_copy_dialog")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(LABEL_WIDTH + FIELD_WIDTH + 24.0);
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 8.0);
            ui.label(egui::RichText::new(tr("사진 복사해서 가져오기")).font(t::bold(14.0)).color(t::BRAND));
            ui.add_space(4.0);
            (action, pick) = form_ui(ui, &mut self.form, &presets);
        });
        if let Some(field) = pick {
            if self.picking.is_none() {
                self.pick(field);
            }
        }
        if response.should_close() && action.is_none() && !cx.dialog_open("import_copy_problem") {
            action = Some(false);
        }
        match action {
            Some(true) => match crate::commands::import::submit(cx, &self.form) {
                Ok(()) => false,
                Err(problem) => {
                    cx.open_dialog(Box::new(Notice {
                        id: "import_copy_problem",
                        title: tr("사진 복사해서 가져오기"),
                        text: problem,
                    }));
                    true
                }
            },
            Some(false) => false,
            None => true,
        }
    }
}

/// What the copy job tells its progress box.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CopyState {
    /// Photos handled.
    pub done: usize,
    /// Photos found.
    pub total: usize,
    /// The job is over: the box closes.
    pub finished: bool,
}

/// The state the copy job and its box share.
#[derive(Debug, Clone, Default)]
pub struct CopyShared(Arc<Mutex<CopyState>>);

impl CopyShared {
    /// Changes the state.
    pub fn update(&self, change: impl FnOnce(&mut CopyState)) {
        if let Ok(mut state) = self.0.lock() {
            change(&mut state);
        }
    }

    /// A copy of the state.
    pub fn get(&self) -> CopyState {
        self.0.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

/// The window-modal `QProgressDialog('사진을 복사하는 중…', '취소', 0, max(1, len(files)))` of the copy: 취소 (or
/// Esc) stops the copy after the photo it is on; what was copied is imported.
pub struct CopyProgress {
    job: JobId,
    shared: CopyShared,
}

impl CopyProgress {
    /// The box of copy job `job`.
    pub fn new(job: JobId, shared: CopyShared) -> Self {
        Self { job, shared }
    }
}

impl Dialog for CopyProgress {
    fn id(&self) -> &'static str {
        "import_copy_progress"
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        let state = self.shared.get();
        if state.finished {
            return false;
        }
        let mut cancel = false;
        let response =
            egui::Modal::new(egui::Id::unique("import_copy_progress")).frame(super::frame()).show(ctx, |ui| {
                ui.set_width(380.0);
                widgets::label(ui, &tr("사진을 복사하는 중…"), t::TEXT, t::FONT_SIZE);
                ui.add_space(8.0);
                let total = state.total.max(1);
                let fraction = state.done.min(total) as f32 / total as f32;
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .desired_width(ui.available_width())
                        .fill(t::ACCENT)
                        .text(format!("{}/{}", state.done, state.total)),
                );
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if Button::new(&tr("취소")).show(ui).clicked() {
                        cancel = true;
                    }
                });
            });
        if cancel || response.should_close() {
            cx.jobs.cancel(self.job);
        }
        ctx.request_repaint_after(Duration::from_millis(100));
        true
    }
}
