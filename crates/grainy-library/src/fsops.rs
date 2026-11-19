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
    let mut created = false;
    let result = (|| -> io::Result<()> {
        let mut input = File::open(source)?;
        let mut output = OpenOptions::new().write(true).create_new(true).open(target)?;
        created = true;
        io::copy(&mut input, &mut output)?;
        output.flush()?;
        output.sync_all()?;
        drop(output);
        copy_stat(source, target)?;
        if digest_bytes(source)? != digest_bytes(target)? {
            return Err(io::Error::other("copy differs from the source"));
        }
        Ok(())
    })();
    if let Err(error) = result {
        // Only remove what this call created.
        if created {
            let _ = fs::remove_file(target);
        }
        return Err(Error::os(&error, target, true));
    }
    Ok(())
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
    match fs::rename(source, target) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            copy_new_verified(source, target)?;
            fs::remove_file(source).map_err(|e| Error::os(&e, source, false))
        }
        Err(error) => Err(Error::os(&error, source, false)),
    }
}

fn tree(root: &Path, base: &Path, out: &mut Vec<(String, Option<u64>)>) -> io::Result<()> {
    for entry in fs::read_dir(base)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let relative = entry.path().strip_prefix(root).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        if kind.is_symlink() {
            return Err(io::Error::other("a symbolic link cannot be moved across volumes"));
        }
        if kind.is_dir() {
            out.push((relative, None));
            tree(root, &entry.path(), out)?;
        } else {
            out.push((relative, Some(entry.metadata()?.len())));
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
    match fs::rename(source, target) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            copy_tree(source, target)?;
            let (mut a, mut b) = (Vec::new(), Vec::new());
            tree(source, source, &mut a).map_err(|e| Error::os(&e, source, false))?;
            tree(target, target, &mut b).map_err(|e| Error::os(&e, target, false))?;
            a.sort();
            b.sort();
            if a != b {
                return Err(Error::Os {
                    message: "the copied folder differs from the source".into(),
                    kind: io::ErrorKind::Other,
                });
            }
            fs::remove_dir_all(source).map_err(|e| Error::os(&e, source, false))
        }
        Err(error) => Err(Error::os(&error, source, false)),
    }
}
