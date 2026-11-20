//! Port of the bridge-file half of `mcp_service.McpService`: the private folder (`mcp/` in the settings folder) with
//! the stdio bridges, their offline answers, and — only while this process owns them — `endpoint` and
//! `auth-header`, guarded by an exclusive lock on `owner.lock` (`_lock`), so that two Grainy processes never both
//! publish a connection.

use std::fs::File;
use std::path::{Path, PathBuf};

use crate::protocol::Protocol;

/// `BRIDGE_FILES`.
pub const BRIDGE_FILES: [&str; 2] = ["grainy-mcp-bridge.ps1", "grainy-mcp-bridge.sh"];
/// `CONNECTION_FILES`: where and how the bridges reach Grainy; only while it owns them.
pub const CONNECTION_FILES: [&str; 2] = ["endpoint", "auth-header"];
/// `OWNER_LOCK`: never deleted (see `_lock`).
pub const OWNER_LOCK: &str = "owner.lock";

/// `_private_folder`: the folder, created when missing; on POSIX readable by this user only.
pub fn private_folder(folder: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(folder)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// `_write_private`: write a small file atomically (a temporary file renamed over it); on POSIX 0600 from the start.
pub fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut staged = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))?;
    staged.write_all(data)?;
    staged.as_file().sync_all()?;
    staged.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// `_lock`: the open file holding an exclusive lock, or `None` when another process (or another handle) holds it.
/// The operating system drops the lock when the process ends; the file is never deleted.
pub fn lock(path: &Path) -> std::io::Result<Option<File>> {
    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        // A file system that cannot lock at all: owned as before the lock existed.
        Err(std::fs::TryLockError::Error(_)) => Ok(Some(file)),
    }
}

/// The bridge files of one settings folder and their owner lock.
pub struct Bridge {
    folder: PathBuf,
    owner: Option<File>,
}

impl Bridge {
    /// The `mcp` folder below the settings folder (`McpService.folder`).
    pub fn new(settings_folder: &Path) -> Self {
        Self { folder: settings_folder.join("mcp"), owner: None }
    }

    /// `McpService.folder`.
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// Whether this process owns `endpoint` and `auth-header`.
    pub fn owned(&self) -> bool {
        self.owner.is_some()
    }

    /// Port of `claim`: own the connection files; `false` when another running Grainy owns them. Copies no running
    /// Grainy owns (left by a crash) are removed. `true` without owning them only when the folder cannot be used.
    pub fn claim(&mut self) -> bool {
        if self.owner.is_some() {
            return true;
        }
        let handle = private_folder(&self.folder).and_then(|()| lock(&self.folder.join(OWNER_LOCK)));
        match handle {
            Err(_) => true,
            Ok(None) => false,
            Ok(Some(file)) => {
                self.owner = Some(file);
                self.remove_files();
                true
            }
        }
    }

    /// Port of `release`.
    pub fn release(&mut self) {
        self.owner = None;
    }

    /// Port of `remove_files`.
    pub fn remove_files(&self) {
        for name in CONNECTION_FILES {
            let _ = std::fs::remove_file(self.folder.join(name));
        }
    }

    /// Port of `remove_stale`: with the connection off, connection files no running Grainy owns are removed.
    pub fn remove_stale(&mut self) {
        if !CONNECTION_FILES.iter().any(|n| self.folder.join(n).exists()) {
            return;
        }
        if self.claim() {
            self.release();
        }
    }

    /// Port of `write_bridges`: the bridges (from `source`, the program's `assets/mcp`) and their offline answers,
    /// rewritten when they differ. True when they are in place.
    pub fn write_bridges(&self, source: &Path, protocol: &Protocol) -> bool {
        let result = (|| -> std::io::Result<()> {
            private_folder(&self.folder)?;
            for name in BRIDGE_FILES {
                let origin = source.join(name);
                if origin.is_file() {
                    let target = self.folder.join(name);
                    let data = std::fs::read(&origin)?;
                    if std::fs::read(&target).ok().as_deref() != Some(&data[..]) {
                        write_private(&target, &data)?;
                    }
                }
            }
            for (name, text) in protocol.offline_templates() {
                let target = self.folder.join(&name);
                if std::fs::read_to_string(&target).ok().as_deref() != Some(text.as_str()) {
                    write_private(&target, text.as_bytes())?;
                }
            }
            Ok(())
        })();
        result.is_ok()
    }

    /// Port of `write_connection`: while running and owning them, where and how the bridges reach Grainy.
    pub fn write_connection(&self, port: u16, token: &str) -> bool {
        if self.owner.is_none() {
            return false;
        }
        write_private(&self.folder.join("endpoint"), format!("http://127.0.0.1:{port}/mcp/stdio").as_bytes()).is_ok()
            && write_private(&self.folder.join("auth-header"), format!("Authorization: Bearer {token}").as_bytes())
                .is_ok()
    }

    /// Port of `bridge_path`: the bridge this system's AI programs start.
    pub fn bridge_path(&self) -> PathBuf {
        self.folder.join(if cfg!(windows) { BRIDGE_FILES[0] } else { BRIDGE_FILES[1] })
    }
}
