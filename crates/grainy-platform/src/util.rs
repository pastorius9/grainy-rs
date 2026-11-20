//! Small helpers shared by the modules.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// 64 random bits (the standard library seeds its hasher from the operating system).
pub(crate) fn random_u64() -> u64 {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    hasher.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    hasher.finish()
}

/// 32 random hex digits, like `uuid4().hex`.
pub(crate) fn unique_hex() -> String {
    format!("{:016x}{:016x}", random_u64(), random_u64())
}

/// A uniform number below `limit`.
pub(crate) fn random_below(limit: usize) -> usize {
    let limit = limit as u64;
    let zone = u64::MAX - (u64::MAX % limit);
    loop {
        let value = random_u64();
        if value < zone {
            return (value % limit) as usize;
        }
    }
}

/// Python's `str.strip()` whitespace: Unicode white space plus the four separators U+001C to U+001F.
pub(crate) fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Python `str.strip()`.
pub(crate) fn py_strip(text: &str) -> &str {
    text.trim_matches(is_py_space)
}

/// Whether a process is still running; `sysinfo` keeps this free of `unsafe`.
pub(crate) fn process_running(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let Ok(mut system) = System::new() else { return false };
    let pid = Pid::from_u32(pid);
    system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, ProcessRefreshKind::nothing());
    system.process(pid).is_some()
}

/// Port of luma/updater.py `wait_for_exit`: true once the process is gone, false if it still runs after
/// `timeout` (a process number of 0, or one that cannot be a process, counts as gone).
pub fn wait_for_exit(pid: i64, timeout: Duration) -> bool {
    let Ok(pid) = u32::try_from(pid) else { return true };
    if pid == 0 {
        return true;
    }
    let deadline = Instant::now() + timeout;
    loop {
        if !process_running(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Rename on one volume only, atomically refusing to replace a destination that appeared concurrently.
pub fn rename_new(source: &std::path::Path, target: &std::path::Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        fn wide(path: &std::path::Path) -> std::io::Result<Vec<u16>> {
            let mut name: Vec<_> = std::path::absolute(path)?.as_os_str().encode_wide().collect();
            if name.contains(&0) {
                return Err(std::io::ErrorKind::InvalidInput.into());
            }
            name.push(0);
            Ok(name)
        }
        let from = wide(source)?;
        let to = wide(target)?;
        // SAFETY: both buffers are NUL-terminated, live and immutable throughout the call.
        // Zero flags disallow replacement and the API's unverified cross-volume copy fallback.
        #[allow(unsafe_code)]
        let moved = unsafe { windows_sys::Win32::Storage::FileSystem::MoveFileExW(from.as_ptr(), to.as_ptr(), 0) };
        if moved == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    {
        use rustix::fs::{renameat_with, RenameFlags, CWD};
        renameat_with(CWD, source, CWD, target, RenameFlags::NOREPLACE).map_err(Into::into)
    }
    #[cfg(not(any(windows, target_vendor = "apple", target_os = "linux")))]
    Err(std::io::ErrorKind::Unsupported.into())
}
