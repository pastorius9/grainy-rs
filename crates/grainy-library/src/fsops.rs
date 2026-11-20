//! Guarded file operations. Nothing here overwrites an existing file: a move or copy to an existing target
//! fails, copies are created exclusively, and a source is only removed after a copy has been verified byte for
//! byte (SHA-256).

use crate::{Error, Result};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

/// `library.file_digest`: SHA-256 of a file as lower-case hex.
pub fn file_digest(path: &Path) -> io::Result<String> {
    Ok(hex(&digest_bytes(path)?))
}

/// SHA-256 of a file as raw bytes (`import_copy._digest`).
pub fn digest_bytes(path: &Path) -> io::Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut block = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut block)?;
        if n == 0 {
            break;
        }
        hasher.update(&block[..n]);
    }
    Ok(hasher.finalize().into())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `shutil.copystat`: modification and access time, then permissions.
pub fn copy_stat(source: &Path, target: &Path) -> io::Result<()> {
    let meta = fs::metadata(source)?;
    let file = OpenOptions::new().write(true).open(target)?;
    let mut times = fs::FileTimes::new();
    if let Ok(modified) = meta.modified() {
        times = times.set_modified(modified);
    }
    if let Ok(accessed) = meta.accessed() {
        times = times.set_accessed(accessed);
    }
    file.set_times(times)?;
    drop(file);
    fs::set_permissions(target, meta.permissions())
}

/// Copies `source` to a new file `target` (exclusive create, never overwrites) with its stat, and verifies the
/// copy. On a mismatch or failure the half-written target is removed (it is ours: it did not exist before).
pub fn copy_new_verified(source: &Path, target: &Path) -> Result<()> {
    let mut input = File::open(source).map_err(|e| Error::os(&e, source, true))?;
    let mut publish = || -> io::Result<()> {
        let parent = target.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let mut stage = tempfile::NamedTempFile::new_in(parent)?;
        io::copy(&mut input, stage.as_file_mut())?;
        stage.as_file_mut().flush()?;
        stage.as_file().sync_all()?;
        if digest_bytes(source)? != digest_bytes(stage.path())? {
            return Err(io::Error::other("copy differs from the source"));
        }
        copy_stat(source, stage.path())?;
        // Windows tempfile publication normalizes the temporary file's attributes.
        #[cfg(windows)]
        let permissions = input.metadata()?.permissions();
        let _published = stage.persist_noclobber(target).map_err(|e| e.error)?;
        #[cfg(windows)]
        _published.set_permissions(permissions)?;
        Ok(())
    };
    publish().map_err(|e| Error::os(&e, target, true))
}

/// `shutil.move` for one file, without ever replacing an existing target: a rename, or, across volumes, a
/// verified copy followed by removing the source.
pub fn move_file(source: &Path, target: &Path) -> Result<()> {
    if target.exists() {
        return Err(Error::Os {
            message: format!("[Errno 17] File exists: {}", crate::pathx::py_repr(&crate::pathx::text(target))),
            kind: io::ErrorKind::AlreadyExists,
        });
    }
    match grainy_platform::rename_new(source, target) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            publish_from_held_source(source, |original| copy_new_verified(original, target))
        }
        Err(error) => Err(Error::os(&error, source, false)),
    }
}

fn tree(root: &Path, base: &Path, out: &mut Vec<(std::path::PathBuf, Option<[u8; 32]>)>) -> io::Result<()> {
    for entry in fs::read_dir(base)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let relative = entry.path().strip_prefix(root).map_err(io::Error::other)?.to_owned();
        if kind.is_symlink() {
            return Err(io::Error::other("a symbolic link cannot be moved across volumes"));
        }
        if kind.is_dir() {
            out.push((relative, None));
            tree(root, &entry.path(), out)?;
        } else {
            out.push((relative, Some(digest_bytes(&entry.path())?)));
        }
    }
    Ok(())
}

fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir(target).map_err(|e| Error::os(&e, target, false))?;
    for entry in fs::read_dir(source).map_err(|e| Error::os(&e, source, false))? {
        let entry = entry.map_err(|e| Error::os(&e, source, false))?;
        let kind = entry.file_type().map_err(|e| Error::os(&e, source, false))?;
        let destination = target.join(entry.file_name());
        if kind.is_symlink() {
            return Err(Error::Os {
                message: "a symbolic link cannot be moved across volumes".into(),
                kind: io::ErrorKind::Unsupported,
            });
        }
        if kind.is_dir() {
            copy_tree(&entry.path(), &destination)?;
        } else {
            copy_new_verified(&entry.path(), &destination)?;
        }
    }
    if let Ok(meta) = fs::metadata(source) {
        let _ = fs::set_permissions(target, meta.permissions());
    }
    Ok(())
}

/// `shutil.move` for a folder to a new place: a rename (the parent folders are created first, as the oracle's
/// copy fallback does), or across volumes a verified copy of the whole tree followed by removing the source.
pub fn move_dir(source: &Path, target: &Path) -> Result<()> {
    if target.exists() {
        return Err(Error::Os {
            message: format!("[Errno 17] File exists: {}", crate::pathx::py_repr(&crate::pathx::text(target))),
            kind: io::ErrorKind::AlreadyExists,
        });
    }
    if let Some(parent) = target.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent).map_err(|e| Error::os(&e, parent, false))?;
        }
    }
    match grainy_platform::rename_new(source, target) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => move_dir_across_volumes(source, target),
        Err(error) => Err(Error::os(&error, source, false)),
    }
}

fn move_dir_across_volumes(source: &Path, target: &Path) -> Result<()> {
    let parent = |p: &Path| p.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new(".")).to_path_buf();
    let stage = tempfile::Builder::new()
        .prefix(".grainy-copy-")
        .tempdir_in(parent(target))
        .map_err(|e| Error::os(&e, target, false))?;
    let incoming = stage.path().join("incoming");
    copy_tree(source, &incoming)?;
    publish_from_held_source(source, |original| {
        let (mut a, mut b) = (Vec::new(), Vec::new());
        tree(original, original, &mut a).map_err(|e| Error::os(&e, source, false))?;
        tree(&incoming, &incoming, &mut b).map_err(|e| Error::os(&e, target, false))?;
        a.sort();
        b.sort();
        if a != b {
            return Err(Error::value("the copied folder differs from the source"));
        }
        grainy_platform::rename_new(&incoming, target).map_err(|e| Error::os(&e, target, false))
    })
}

fn publish_from_held_source(source: &Path, publish: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
    let parent = source.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    // Move the intact source out of the public namespace in one operation. Never put the only
    // original inside an auto-deleting guard, even during an unwind or failed rollback.
    let hold = tempfile::Builder::new()
        .prefix(".grainy-move-")
        .tempdir_in(parent)
        .map_err(|e| Error::os(&e, source, false))?
        .keep();
    let original = hold.join("original");
    if let Err(e) = grainy_platform::rename_new(source, &original) {
        let _ = fs::remove_dir(&hold);
        return Err(Error::os(&e, source, false));
    }
    if let Err(error) = publish(&original) {
        if let Err(rollback) = grainy_platform::rename_new(&original, source) {
            return Err(Error::value(format!("{error}; original retained at {}: {rollback}", original.display())));
        }
        let _ = fs::remove_dir(&hold);
        return Err(error);
    }
    // The complete verified destination is now authoritative. An undeletable old file can
    // only leave redundant material in the private recovery directory.
    let _ = fs::remove_dir_all(hold);
    Ok(())
}

#[cfg(test)]
#[test]
fn cross_volume_file_move_preserves_a_concurrent_replacement() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source.jpg");
    let target = tmp.path().join("target.jpg");
    fs::write(&source, b"original").unwrap();
    publish_from_held_source(&source, |held| {
        fs::write(&source, b"new at old path").unwrap();
        copy_new_verified(held, &target)
    })
    .unwrap();
    assert_eq!(fs::read(source).unwrap(), b"new at old path");
    assert_eq!(fs::read(target).unwrap(), b"original");
}

#[cfg(test)]
#[test]
fn cross_volume_publication_race_preserves_the_original_tree() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let target = dir.path().join("target");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&target).unwrap();
    fs::write(source.join("photo.jpg"), b"original").unwrap();
    assert!(move_dir_across_volumes(&source, &target).is_err());
    assert_eq!(fs::read(source.join("photo.jpg")).unwrap(), b"original");
    assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
    fs::remove_dir(&target).unwrap();
    move_dir_across_volumes(&source, &target).unwrap();
    assert_eq!(fs::read(target.join("photo.jpg")).unwrap(), b"original");
    assert!(!source.exists());
}
