//! The export dialog (luma/app.py `ExportDialog`): where to save, format (JPEG, AVIF, JPEG XL, PNG, TIFF, 원본 +
//! 설정), quality, long side, colour space, metadata, grayscale, dithering and naming; 저장 starts the export of the
//! selected photos (a background job). Also the box listing the photos an export could not write
//! (`QMessageBox.warning(self, '일부 파일 내보내기 실패', ...)`) and the wait for the folder of a quick export.

use eframe::egui::{self, Ui, Vec2};

use std::sync::mpsc::{channel, Receiver, TryRecvError};

use super::native::{self, Chosen, Purpose};
use super::Dialog;
use crate::context::AppContext;
use crate::export::{format_label, ExportOptions, COLOR_SPACES, FORMATS};
use crate::text::{tr, tr_with};
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

const LABEL_WIDTH: f32 = 150.0;

fn row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(LABEL_WIDTH, 30.0), egui::Sense::hover());
        widgets::paint_left(ui, rect, label, t::font(t::FONT_SIZE), t::TEXT);
        add(ui);
    });
}

/// The export dialog with the values being edited.
pub struct ExportDialog {
    pub options: ExportOptions,
    /// The last validation message (Korean source text).
    pub message: Option<&'static str>,
    /// How many photos will be exported.
    pub count: usize,
}

impl Dialog for ExportDialog {
    fn id(&self) -> &'static str {
        "export"
    }

    fn receive(&mut self, chosen: &Chosen) {
        if let (Purpose::ExportFolder, Some(folder)) = (chosen.purpose, chosen.paths.first()) {
            self.options.folder = folder.to_string_lossy().into_owned();
        }
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        let frame = egui::Frame::new()
            .fill(t::BG)
            .stroke(egui::Stroke::new(1.0, t::MENU_BORDER))
            .inner_margin(egui::Margin::same(18));
        let mut action = None;
        let mut choose = false;
        let response = egui::Modal::new(egui::Id::unique("export_dialog")).frame(frame).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 8.0);
            ui.label(egui::RichText::new(tr("사진 내보내기")).font(t::bold(14.0)).color(t::BRAND));
            widgets::muted(ui, &tr_with("선택한 사진 {0}장을 새 파일로 저장합니다.", &[&self.count.to_string()]));
            (action, choose) = form(ui, &mut self.options);
            if let Some(message) = self.message {
                ui.label(egui::RichText::new(tr(message)).color(t::BRAND));
            }
        });
        if choose {
            // The folder dialog opens where the folder field points (`getExistingDirectory` starts in the
            // folder given; the window keeps this only while the dialog is being opened).
            let kept = std::mem::replace(&mut cx.export_options.folder, self.options.folder.clone());
            cx.choose(Purpose::ExportFolder);
            cx.export_options.folder = kept;
        }
        if response.should_close() && action.is_none() {
            action = Some(false);
        }
        match action {
            Some(true) => match self.options.validate() {
                Ok(()) => {
                    cx.start_export(self.options.clone());
                    false
                }
                Err(message) => {
                    self.message = Some(message);
                    true
                }
            },
            Some(false) => false,
            None => true,
        }
    }
}

/// The form; returns Some(true) for 저장, Some(false) for 취소, and whether the folder button was clicked.
fn form(ui: &mut Ui, options: &mut ExportOptions) -> (Option<bool>, bool) {
    let mut choose = false;
    let fields = options.fields();
    let field = 360.0;
    row(ui, &tr("저장 위치"), |ui| {
        ui.add_sized(Vec2::new(field - 100.0, 28.0), egui::TextEdit::singleline(&mut options.folder));
        if widgets::button(ui, &tr("폴더 선택")).clicked() {
            choose = true;
        }
    });
    row(ui, &tr("파일 형식"), |ui| {
        let listed = crate::export::listed_formats();
        let names: Vec<String> = listed.iter().map(|i| tr(format_label(FORMATS[*i]))).collect();
        let current = listed.iter().position(|i| *i == options.format).unwrap_or(0);
        if let Some(i) = crate::panels::controls::choice(ui, "export_format", &tr("파일 형식"), &names, current, field)
        {
            options.set_format(listed[i]);
        }
    });
    row(ui, &tr("품질"), |ui| {
        if options.format_name() == "JPEG XL" {
            widgets::label(ui, &tr("16비트 무손실"), t::MUTED, t::FONT_SIZE);
        } else {
            ui.add_enabled(fields.quality, egui::DragValue::new(&mut options.quality).range(40..=100));
        }
    });
    row(ui, &tr("긴 변 (px)"), |ui| {
        ui.add_enabled_ui(fields.longest, |ui| {
            ui.add(egui::DragValue::new(&mut options.longest).range(0..=30000).speed(50.0));
            if options.longest == 0 {
                widgets::label(ui, &tr("원본 크기"), t::MUTED, t::FONT_SIZE);
            }
        });
    });
    row(ui, &tr("출력 색공간"), |ui| {
        if options.format_name() == "AVIF HDR 10-bit" {
            widgets::label(ui, "Rec.2100 PQ · BT.2020", t::TEXT, t::FONT_SIZE);
            return;
        }
        let names: Vec<String> = COLOR_SPACES.iter().map(|c| (*c).to_owned()).collect();
        ui.add_enabled_ui(fields.color_space, |ui| {
            if let Some(i) = crate::panels::controls::choice(
                ui,
                "export_space",
                &tr("출력 색공간"),
                &names,
                options.color_space,
                field,
            ) {
                options.color_space = i;
            }
        });
    });
    ui.add_enabled_ui(fields.keep_metadata, |ui| {
        widgets::checkbox(ui, &mut options.keep_metadata, &tr("EXIF·IPTC·GPS 메타데이터 유지"));
    });
    let (grayscale, dither) = (fields.grayscale, fields.dither);
    ui.add_enabled_ui(grayscale, |ui| {
        widgets::checkbox(ui, &mut options.grayscale, &tr("흑백 사진은 흑백 파일로 저장 (sRGB)"))
            .on_hover_text(tr("모든 픽셀이 무채색이면 1채널 흑백 파일과 흑백 ICC로 저장합니다. 화면에 보이는 밝기는 같고 파일이 작아집니다."));
    });
    ui.add_enabled_ui(dither, |ui| {
        widgets::checkbox(ui, &mut options.dither, &tr("8비트 저장 시 계단 현상 방지 (디더링)"))
            .on_hover_text(tr("8비트로 줄일 때 눈에 띄지 않는 고정 노이즈를 더해 하늘 같은 부드러운 영역의 띠를 막습니다. 순수한 검정·흰색은 그대로입니다."));
    });
    row(ui, &tr("파일 이름 규칙"), |ui| {
        ui.add_sized(Vec2::new(field, 28.0), egui::TextEdit::singleline(&mut options.naming));
    });
    widgets::muted(
        ui,
        &tr("이름 규칙: {stem} 원본 이름 / {n:03d} 순번\nICC 프로파일 포함 · 같은 이름이 있으면 번호를 붙여 저장합니다."),
    );
    let mut result = None;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
        if widgets::button(ui, &tr("취소")).clicked() {
            result = Some(false);
        }
        ui.add_space(6.0);
        if Button::new(&tr("저장")).kind(Kind::Primary).show(ui).clicked() {
            result = Some(true);
        }
    });
    if crate::hdr::enabled() {
        // `hdr_note`, below the buttons.
        widgets::muted(
            ui,
            &tr("HDR TIFF: HDR 모드 사진만 저장 · 선형 작업 색공간 · 32비트
JPEG·일반 PNG/TIFF는 SDR 미리보기 설정으로 저장합니다."),
        );
    }
    (result, choose)
}

/// The box listing the photos an export could not write: `QMessageBox.warning(self, '일부 파일 내보내기 실패',
/// '\n'.join(result['failures']))`.
pub struct ExportFailures {
    /// `name: message` per photo.
    pub lines: Vec<String>,
}

impl Dialog for ExportFailures {
    fn id(&self) -> &'static str {
        "export_failures"
    }

    fn show(&mut self, ctx: &egui::Context, _cx: &mut AppContext) -> bool {
        let mut closed = false;
        let response = egui::Modal::new(egui::Id::unique("export_failures")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.label(egui::RichText::new(tr("일부 파일 내보내기 실패")).font(t::bold(14.0)).color(t::BRAND));
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(300.0).auto_shrink([false, true]).show(ui, |ui| {
                for line in &self.lines {
                    widgets::label(ui, line, t::TEXT, t::FONT_SIZE);
                }
            });
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if Button::new(&tr("확인")).kind(Kind::Primary).show(ui).clicked() {
                    closed = true;
                }
            });
        });
        !(closed || response.should_close())
    }
}

/// The wait for the answer of the folder dialog of a quick export
/// (`QFileDialog.getExistingDirectory(self, title, start)`): draws nothing; the answer starts the export with the
/// options of the quick export in that folder, a cancel does nothing.
pub struct QuickFolder {
    options: ExportOptions,
    answer: Receiver<Chosen>,
}

impl QuickFolder {
    /// Opens the native folder dialog (on its own thread) and waits for it.
    pub fn new(cx: &AppContext, title: &str, start: String, options: ExportOptions) -> Self {
        let (tx, answer) = channel();
        let start = (!start.is_empty()).then(|| std::path::PathBuf::from(start));
        native::open(Purpose::ExportFolder, tr(title), start, tx, cx.waker.clone());
        Self { options, answer }
    }
}

impl Dialog for QuickFolder {
    fn id(&self) -> &'static str {
        "export_quick_folder"
    }

    fn show(&mut self, _ctx: &egui::Context, cx: &mut AppContext) -> bool {
        match self.answer.try_recv() {
            Ok(chosen) => {
                crate::commands::export::finish_quick(cx, self.options.clone(), chosen.paths.into_iter().next());
                false
            }
            Err(TryRecvError::Empty) => true,
            Err(TryRecvError::Disconnected) => false,
        }
    }
}
