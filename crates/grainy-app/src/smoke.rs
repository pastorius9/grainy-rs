//! `--smoke <folder>`: a non-interactive start for the release smoke test (`tools/release/smoke.py`).
//!
//! The window opens as usual (library, network switch, photos given on the command line are imported). Once the
//! app is idle and the first photo is shown in Develop, that photo is exported with the export dialog's default
//! options into `<folder>`, `<folder>/smoke.json` is written (where the lens database comes from, how many lenses
//! the built-in one holds, the photo's frame size, the files written, failures) and the window closes normally.
//! After [`TIMEOUT`] the report is written with an error and the window closes too. Nothing here runs without the
//! option.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use eframe::egui;
use serde_json::{json, Value};

use crate::commands::lens::{database_source, Source};
use crate::context::{AppContext, Mode};
use crate::export::{self, ExportOptions};

/// How long the smoke run may take before it gives up.
pub const TIMEOUT: Duration = Duration::from_secs(180);

/// The smoke run's state (in the context: `cx.state::<Smoke>()`).
#[derive(Default)]
pub struct Smoke {
    folder: Option<PathBuf>,
    started: Option<Instant>,
    finished: bool,
}

/// Starts a smoke run that reports into `folder`.
pub fn install(cx: &mut AppContext, folder: PathBuf) {
    let state = cx.state::<Smoke>();
    state.folder = Some(folder);
    state.started = Some(Instant::now());
}

/// Every frame: advances the smoke run, if one was asked for.
pub fn frame(cx: &mut AppContext, ctx: &egui::Context) {
    let state = cx.state::<Smoke>();
    let (Some(folder), Some(started), false) = (state.folder.clone(), state.started, state.finished) else {
        return;
    };
    ctx.request_repaint_after(Duration::from_millis(50));
    if started.elapsed() > TIMEOUT {
        finish(cx, ctx, &folder, Err("timeout: the photo was not shown".into()));
        return;
    }
    if !cx.idle() {
        return;
    }
    if cx.doc.photos().is_empty() {
        return;
    }
    if cx.doc.current().is_none() {
        cx.doc.select(0);
        return;
    }
    if !matches!(cx.mode(), Mode::Develop) {
        cx.set_mode(Mode::Develop);
        return;
    }
    let photo = cx.doc.current_id();
    let Some(shown) = cx.frame().filter(|f| f.photo == photo).map(|f| (f.preview.width, f.preview.height)) else {
        return;
    };
    let path = cx.doc.current().map(|p| p.path.clone()).unwrap_or_default();
    let settings = cx.settings().clone();
    let options = ExportOptions { folder: folder.to_string_lossy().into_owned(), ..ExportOptions::default() };
    let (written, failures) = export::run(&[(path.clone(), settings)], &options, &|_, _| {}, &AtomicBool::new(false));
    finish(
        cx,
        ctx,
        &folder,
        Ok(json!({
            "photo": path.to_string_lossy(),
            "shown": [shown.0, shown.1],
            "written": written.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>(),
            "failures": failures,
        })),
    );
}

fn finish(cx: &mut AppContext, ctx: &egui::Context, folder: &std::path::Path, result: Result<Value, String>) {
    cx.state::<Smoke>().finished = true;
    let lens = match database_source(None, None) {
        Source::Builtin => match grainy_lens::Database::builtin() {
            Ok(database) => json!({
                "source": "builtin",
                "files": grainy_lens::Database::builtin_file_count(),
                "lenses": database.lens_records().len(),
                "cameras": database.cameras().len(),
            }),
            Err(error) => json!({"source": "builtin", "error": error.to_string()}),
        },
        Source::Folder(f) => json!({"source": "folder", "folder": f.to_string_lossy()}),
    };
    let mut report = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "offline": grainy_platform::net::offline(),
        "photos": cx.doc.photos().len(),
        "lens_database": lens,
    });
    match result {
        Ok(Value::Object(map)) => report.as_object_mut().into_iter().for_each(|r| r.extend(map.clone())),
        Ok(_) => {}
        Err(message) => report["error"] = Value::String(message),
    }
    let text = serde_json::to_string_pretty(&report).unwrap_or_default();
    if let Err(error) = std::fs::create_dir_all(folder).and_then(|()| std::fs::write(folder.join("smoke.json"), text)) {
        log::error!("smoke: cannot write the report: {error}");
    }
    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
}
