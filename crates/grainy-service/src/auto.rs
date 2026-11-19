//! Automatic values (`photo_service` "automatic values", worker thread): the per-photo loop of the batch auto
//! tone/colour/negative commands. Loading pixels and the maths are injected.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};

use crate::pyutil::get;
use crate::targets::{access, Access};
use crate::{object, Error, Failure, Result};

/// Port of `photo_service.reason`: why one photo could not be read or computed, in English.
pub fn reason(error: &Error, path: &str) -> String {
    match access(path) {
        Access::Missing => "original missing and no offline preview".into(),
        Access::Denied => "Grainy may not read the original (system permission)".into(),
        Access::Present => {
            if matches!(error, Error::OriginalChanged(_)) {
                "the original changed while it was read".into()
            } else {
                format!("could not read or compute ({})", error.python_name())
            }
        }
    }
}

/// What `auto_values` needs from the engine: the source pixels of a photo and the automatic values for them.
pub trait AutoSource {
    /// The loaded source (the oracle's 720-pixel float image).
    type Image;
    /// The computed value for one photo.
    type Value;
    /// `engine.load_image(path, 720, settings['working_space'], raw_options=settings)` (or the window's loader).
    fn load(&mut self, record: &Value, path: &Path) -> std::result::Result<Self::Image, Failure>;
    /// `preview_store.load_offline(directory, record['id'], settings)` for an original that is not present.
    fn offline(&mut self, record: &Value) -> std::result::Result<Self::Image, Failure>;
    /// `compute(resize_float(source, 720), settings)`.
    fn compute(&mut self, image: Self::Image, settings: &Value) -> std::result::Result<Self::Value, Failure>;
    /// `preview_store.signature(path)`: changes when the original changes.
    fn signature(&mut self, path: &Path) -> Option<Value> {
        grainy_catalog::preview::signature(path)
    }
}

/// `(values, errors)` of [`auto_values`]: `values` is `None` when cancelled.
pub type AutoValues<T> = (Option<Vec<Option<T>>>, Vec<Value>);

/// Port of `photo_service.auto_values`: the computed value of every record, as the menus' batch auto tone does.
/// `strict` lets the first failure through; otherwise that photo gets `None` and an `{id, error}`.
pub fn auto_values<S: AutoSource>(
    records: &[Value],
    source: &mut S,
    cancel: Option<&AtomicBool>,
    mut progress: Option<&mut dyn FnMut(usize)>,
    strict: bool,
) -> Result<AutoValues<S::Value>> {
    let cancelled = || cancel.is_some_and(|c| c.load(Ordering::SeqCst));
    let (mut values, mut errors) = (Vec::new(), Vec::new());
    for (index, record) in records.iter().enumerate() {
        if cancelled() {
            return Ok((None, errors));
        }
        let settings = get(record, "settings");
        let path_text = crate::text_of(record, "path")?;
        let path = Path::new(path_text);
        let result = (|| -> std::result::Result<S::Value, Error> {
            let image = if access(path_text) == Access::Present {
                let before = source.signature(path);
                let image = source.load(record, path).map_err(Error::Failed)?;
                if source.signature(path) != before {
                    return Err(Error::OriginalChanged("원본이 변경되었습니다.".into()));
                }
                image
            } else {
                source.offline(record).map_err(Error::Failed)?
            };
            source.compute(image, settings).map_err(Error::Failed)
        })();
        match result {
            Ok(v) => values.push(Some(v)),
            Err(error) => {
                if strict {
                    return Err(error);
                }
                values.push(None);
                errors.push(json!({"id": get(record, "id"), "error": reason(&error, path_text)}));
            }
        }
        if let Some(p) = progress.as_mut() {
            p(index + 1);
        }
    }
    if cancelled() {
        return Ok((None, errors));
    }
    Ok((Some(values), errors))
}

/// The kinds of `photo_service.auto_job`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoKind {
    /// `engine.auto_tone_settings`.
    Tone,
    /// `auto_color.auto_color_settings(source, settings, amount)`.
    Color,
    /// `negative.converted` then `{'status': 'applied'}`.
    Negative,
    /// `negative.sample`, combined later with `negative.converted_together`.
    Roll,
}
impl AutoKind {
    /// The oracle's kind string; `ValueError(kind)` for any other.
    pub fn parse(kind: &str) -> Result<Self> {
        Ok(match kind {
            "tone" => Self::Tone,
            "color" => Self::Color,
            "negative" => Self::Negative,
            "roll" => Self::Roll,
            other => return Err(Error::Value(other.into())),
        })
    }
}

/// Port of the record half of `photo_service.auto_job`: the records to load. Negatives and rolls are loaded as
/// negatives (`negative: true`), so camera RAW files arrive in the camera's own colours. The compute half is the
/// engine's function for the kind.
pub fn auto_records(kind: AutoKind, records: &[Value]) -> Result<Vec<Value>> {
    if matches!(kind, AutoKind::Tone | AutoKind::Color) {
        return Ok(records.to_vec());
    }
    records
        .iter()
        .map(|r| {
            let mut record = object(r)?.clone();
            let mut settings = object(get(r, "settings"))?.clone();
            settings.insert("negative".into(), Value::Bool(true));
            record.insert("settings".into(), Value::Object(settings));
            Ok(Value::Object(record))
        })
        .collect()
}
