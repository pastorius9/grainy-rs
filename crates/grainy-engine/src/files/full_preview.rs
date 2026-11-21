//! Share the original RAW decode already owned by the preview worker with an export.
use super::{extension, fingerprint, load_photo, Metadata, Photo, Result, RAW_EXTENSIONS};
use grainy_core::Settings;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

struct Entry {
    path: PathBuf,
    stamp: Value,
    digest: [u8; 32],
    space: String,
    camera: bool,
    photo: Weak<Photo>,
}

// Dropping the preview releases the frame even if the last filename stays here.
static LAST: Mutex<Option<Entry>> = Mutex::new(None);

fn content_digest(path: &Path) -> Option<[u8; 32]> {
    let mut hash = Sha256::new();
    std::io::copy(&mut File::open(path).ok()?, &mut hash).ok()?;
    Some(hash.finalize().into())
}

/// Load an online, uncropped, original-resolution preview. The export path may borrow
/// its immutable pixels while the caller keeps them. Reduced and offline previews use
/// their existing loaders.
pub fn load_full_preview(path: &Path, settings: &Settings) -> Result<(Arc<Photo>, Metadata)> {
    let is_raw = RAW_EXTENSIONS.contains(&extension(path).as_str());
    let (digest, loaded) =
        rayon::join(|| is_raw.then(|| content_digest(path)).flatten(), || load_photo(path, None, settings));
    let loaded = loaded?;
    let photo = Arc::new(loaded.photo);
    if let (Some(digest), Some(stamp)) = (digest, loaded.metadata.get("file_fingerprint")) {
        let entry = Entry {
            path: path.to_owned(),
            stamp: stamp.clone(),
            digest,
            space: settings.str("working_space").to_owned(),
            camera: grainy_raw::rawcolor::enabled(settings),
            photo: Arc::downgrade(&photo),
        };
        *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(entry);
    }
    Ok((photo, loaded.metadata))
}

pub(super) fn for_export(path: &Path, settings: &Settings) -> Option<Arc<Photo>> {
    let (photo, stamp, expected_digest) = {
        let guard = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let entry = guard.as_ref()?;
        if entry.path != path
            || entry.space != settings.str("working_space")
            || entry.camera != grainy_raw::rawcolor::enabled(settings)
        {
            return None;
        }
        (entry.photo.upgrade()?, entry.stamp.clone(), entry.digest)
    };
    let info = std::fs::metadata(path).ok()?;
    if fingerprint(path, info.len(), info.modified().ok()?).ok()? != stamp {
        return None;
    }
    // A replaced original can retain its filename, size and modification timestamp.
    (content_digest(path)? == expected_digest).then_some(photo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::{export_photo, ExportRequest};
    use serde_json::json;

    #[test]
    fn export_reuses_only_live_matching_original_and_renders_current_edits() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("source.dng");
        std::fs::copy(grainy_testkit::root().join("golden/rawload/_files/bayer0.dng"), &path).unwrap();
        let settings = Settings(grainy_core::normalized(&Default::default()).0);
        let mut edited = settings.clone();
        edited.0.insert("exposure".into(), json!(0.68));
        let request = ExportRequest { format: grainy_codecs::ExportFormat::Png, ..Default::default() };
        let output = |name: &str| {
            let dest = tmp.path().join(name);
            export_photo(&path, &dest, &edited.0, &request).unwrap();
            grainy_codecs::load_image(&dest, &Default::default(), &super::super::Cms::for_loading()).unwrap().pixels
        };
        let cold = output("cold.png");
        let (pixels, _) = load_full_preview(&path, &settings).unwrap();
        assert!(Arc::ptr_eq(&pixels, &for_export(&path, &edited).unwrap()));
        assert_eq!(cold, output("warm.png"));
        let mut other = edited.clone();
        other.0.insert("working_space".into(), json!("ProPhoto"));
        assert!(for_export(&path, &other).is_none());
        other = edited.clone();
        other.0.insert("raw_mode".into(), json!("legacy"));
        other.0.insert("dcp_profile".into(), json!(false));
        other.0.insert("negative".into(), json!(!grainy_raw::rawcolor::enabled(&edited)));
        assert!(for_export(&path, &other).is_none());
        drop(pixels);
        assert!(for_export(&path, &edited).is_none());
        let (_keep, _) = load_full_preview(&path, &settings).unwrap();
        let mtime = path.metadata().unwrap().modified().unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[0] ^= 1;
        std::fs::write(&path, bytes).unwrap();
        File::options().write(true).open(&path).unwrap().set_modified(mtime).unwrap();
        assert!(for_export(&path, &edited).is_none());
    }
}
