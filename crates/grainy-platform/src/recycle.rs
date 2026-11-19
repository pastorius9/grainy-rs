//! Moving a file or folder to the person's recycle bin (Windows) or Trash (macOS), never deleting it for good: the
//! oracle's `send2trash`.
//!
//! On macOS `send2trash` asks `NSFileManager trashItemAtURL` (no other program, no permission prompt). The `trash`
//! crate's default macOS method drives Finder through Apple Events instead, which asks the person for permission to
//! control Finder and fails silently without it; so the crate's `NsFileManager` method is chosen there. Windows
//! keeps the crate's default (the shell's recycle bin), as before.

use std::path::Path;

/// The recycle bin of this system, as `send2trash` uses it.
fn context() -> trash::TrashContext {
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        let mut context = trash::TrashContext::new();
        context.set_delete_method(DeleteMethod::NsFileManager);
        context
    }
    #[cfg(not(target_os = "macos"))]
    {
        trash::TrashContext::new()
    }
}

/// Port of `send2trash(path)`: moves `path` to the recycle bin / Trash; false when that fails (the file stays).
pub fn recycle(path: &Path) -> bool {
    context().delete(path).is_ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_missing_path_is_not_recycled() {
        // Nothing is created: a path that does not exist fails without touching the recycle bin.
        let missing = std::env::temp_dir().join(format!("grainy-recycle-missing-{}", std::process::id())).join("x");
        assert!(!super::recycle(&missing));
    }
}
