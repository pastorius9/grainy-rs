//! The 1800-pixel analysis source of the automatic buttons. A click and an idle
//! preparation can join the same decode. Only the current slot stays in the context.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use grainy_core::Settings;
use grainy_engine::files::{load_photo, AnalysisSeed, Photo, RAW_EXTENSIONS};

use crate::AppContext;

pub(crate) const SIZE: usize = 1800;

/// Inputs read by files::load_photo: the raster working space and the effective RAW decoder.
#[derive(PartialEq, Eq)]
pub(crate) struct LoadSettings {
    space: String,
    camera: bool,
}

impl LoadSettings {
    pub(crate) fn new(path: &Path, s: &Settings) -> Self {
        let raw = path
            .extension()
            .and_then(|v| v.to_str())
            .is_some_and(|ext| RAW_EXTENSIONS.iter().any(|raw| raw.eq_ignore_ascii_case(ext)));
        Self { space: s.str("working_space").to_owned(), camera: raw && grainy_raw::rawcolor::enabled(s) }
    }
}

pub(super) struct Slot {
    path: PathBuf,
    settings: LoadSettings,
    seed: Mutex<Option<AnalysisSeed>>,
    loaded: OnceLock<Result<Photo, String>>,
}

impl Slot {
    pub(super) fn load(&self, settings: &Settings) -> Result<&Photo, String> {
        let loaded = self
            .loaded
            .get_or_init(|| {
                let seed = self.seed.lock().unwrap_or_else(|e| e.into_inner()).take();
                match seed {
                    Some(seed) => seed.finish(),
                    None => load_photo(&self.path, Some(SIZE), settings).map(|loaded| loaded.photo),
                }
                .map_err(|e| e.to_string())
            })
            .as_ref()
            .map_err(Clone::clone);
        // A click can start from disk just before the visible decode hands off its seed.
        self.seed.lock().unwrap_or_else(|e| e.into_inner()).take();
        loaded
    }
}

#[derive(Default)]
struct Sources {
    slot: Option<Arc<Slot>>,
    pending: bool,
}

impl Sources {
    fn slot(&mut self, path: &Path, s: &Settings, retry: bool) -> Arc<Slot> {
        let key = LoadSettings::new(path, s);
        if let Some(slot) = &self.slot {
            if slot.path == path && slot.settings == key && !(retry && slot.loaded.get().is_some_and(Result::is_err)) {
                return slot.clone();
            }
        }
        let slot =
            Arc::new(Slot { path: path.to_owned(), settings: key, loaded: OnceLock::new(), seed: Mutex::default() });
        self.slot = Some(slot.clone());
        slot
    }
}

pub(super) fn for_click(cx: &mut AppContext, path: &Path, s: &Settings) -> Arc<Slot> {
    source(cx, path, s, true)
}

fn source(cx: &mut AppContext, path: &Path, s: &Settings, retry: bool) -> Arc<Slot> {
    let slot = cx.state::<Sources>().slot(path, s, retry);
    if let Some(seed) = cx.renderer.take_analysis_seed(path, s) {
        let mut pending = slot.seed.lock().unwrap_or_else(|e| e.into_inner());
        if slot.loaded.get().is_none() {
            *pending = Some(seed);
        }
    }
    slot
}

pub(crate) fn clear(cx: &mut AppContext) {
    cx.jobs.cancel_kind("prepare-auto");
    *cx.state::<Sources>() = Sources::default();
}

/// Called once the visible picture has arrived. A pending button click takes priority.
/// LibRaw cannot interrupt an unpack already in flight; an obsolete result dies with its slot.
pub(crate) fn prepare(cx: &mut AppContext) {
    if cx.jobs.running("prepare-auto") {
        cx.state::<Sources>().pending = true;
        return;
    }
    if cx.jobs.running("auto") {
        return;
    }
    let Some(photo) = cx.doc.current() else { return };
    let path = photo.path.clone();
    let s = cx.settings().clone();
    let slot = source(cx, &path, &s, false);
    if slot.loaded.get().is_some() {
        return;
    }
    let weak = Arc::downgrade(&slot);
    cx.jobs.spawn(
        "prepare-auto",
        "",
        move |handle| {
            // Spare the interactive Rayon pool. The resize uses at most four background lanes,
            // and an arriving photo can cancel this job before its decode or resize starts.
            static IDLE: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
            let pool = IDLE.get_or_init(|| {
                let available = std::thread::available_parallelism().map_or(1, usize::from);
                rayon::ThreadPoolBuilder::new()
                    .num_threads((available / 4).clamp(1, 4))
                    .thread_name(|i| format!("grainy-analysis-{i}"))
                    .build()
                    .ok()
            });
            let prepare = || {
                if let Some(slot) = weak.upgrade().filter(|_| !handle.cancelled()) {
                    let _ = slot.load(&s);
                }
            };
            if let Some(pool) = pool {
                pool.install(prepare);
            } else {
                prepare();
            }
            Ok(())
        },
        |cx, _| {
            if std::mem::take(&mut cx.state::<Sources>().pending) {
                prepare(cx);
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_working_space_and_effective_decoder_select_the_analysis_source() {
        let mut sources = Sources::default();
        let mut s = grainy_core::settings::defaults();
        let path = Path::new("generated.NEF");
        let first = sources.slot(path, &s, false);
        s.set("exposure", 1.);
        assert!(Arc::ptr_eq(&first, &sources.slot(path, &s, true)));
        s.set("negative", true);
        let camera = sources.slot(path, &s, true);
        assert!(!Arc::ptr_eq(&first, &camera));
        s.set("negative", false);
        s.set("raw_mode", "native");
        assert!(Arc::ptr_eq(&camera, &sources.slot(path, &s, true)));
        s.set("working_space", "ProPhoto");
        assert!(!Arc::ptr_eq(&camera, &sources.slot(path, &s, true)));
        let raster = sources.slot(Path::new("generated.png"), &s, true);
        s.set("raw_mode", "legacy");
        assert!(Arc::ptr_eq(&raster, &sources.slot(Path::new("generated.png"), &s, false)));
        assert!(!Arc::ptr_eq(&raster, &sources.slot(Path::new("other.png"), &s, true)));
    }

    #[test]
    fn a_click_joins_the_preparation_and_a_new_photo_has_an_empty_slot() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../golden/app/display_inputs/photo.png");
        let mut sources = Sources::default();
        let s = grainy_core::defaults();
        let slot = sources.slot(&path, &s, false);
        let click = sources.slot(&path, &s, true);
        std::thread::scope(|threads| {
            let prepare = threads.spawn(|| slot.load(&s).unwrap());
            let loaded = click.load(&s).unwrap();
            assert!(std::ptr::eq(prepare.join().unwrap(), loaded));
            assert_eq!(loaded.pixels(), load_photo(&path, Some(SIZE), &s).unwrap().photo.pixels());
        });
        let next = sources.slot(Path::new("different.png"), &s, false);
        assert!(next.loaded.get().is_none());
    }
}
