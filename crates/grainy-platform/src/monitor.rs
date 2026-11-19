//! The monitor a window is on and the colour profile the system gives that monitor: the operating-system half of
//! luma/display_profiles.py `windows_profile_path` and of display_color.py `DisplayTarget.screen_name` (on Windows
//! Qt names a screen by its GDI device name, `\\.\DISPLAYn`; here the name comes from `GetMonitorInfoW`).
//!
//! The logic of the oracle's `GetICMProfileW` loop is [`profile_path_with`], over the [`Gdi`] calls, so it is tested
//! against the oracle with a scripted stand-in (`golden/app/monitor_query`). The real calls are in the private
//! module `windows`, the only `unsafe` code of this crate; read-only, as in the oracle: no profile is installed,
//! associated or enumerated. On other systems there is no lookup: every answer is "the system default" (sRGB).

/// The longest profile path accepted, in UTF-16 units (`size.value>32768` in the oracle).
pub const MAX_PATH_UNITS: u32 = 32768;

/// `ERROR_INSUFFICIENT_BUFFER`: the buffer was too small, the size holds the new one.
const INSUFFICIENT_BUFFER: u32 = 122;

/// Last-error codes that mean "no profile" when the size query reports nothing (the oracle's `(0,2,1168,2015)`):
/// none, file not found, element not found, profile not found. sRGB, also in Windows Advanced Color mode.
const NO_PROFILE: [u32; 4] = [0, 2, 1168, 2015];

/// The calls of `windows_profile_path` on one display device context: `CreateDCW`, `GetICMProfileW`, the thread's
/// last-error code and `DeleteDC`. The real ones are the system's; tests script them.
pub trait Gdi {
    /// `CreateDCW(screen, screen, NULL, NULL)`; `false` when no context was made (the last error says why).
    fn create_dc(&mut self, screen: &str) -> bool;
    /// `ctypes.set_last_error`.
    fn set_last_error(&mut self, code: u32);
    /// `ctypes.get_last_error`: the code the last call left.
    fn last_error(&mut self) -> u32;
    /// `GetICMProfileW(dc, &size, buffer)`: `size` is the buffer's length in UTF-16 units on the way in and the
    /// length needed on the way out; `buffer` `None` asks for the size only.
    fn query(&mut self, size: &mut u32, buffer: Option<&mut [u16]>) -> bool;
    /// `DeleteDC` of the context made by [`Gdi::create_dc`].
    fn delete_dc(&mut self);
}

/// The oracle's check `re.fullmatch(r'\\\\\.\\DISPLAY\d+', screen, re.IGNORECASE)`: a GDI display device name such
/// as `\\.\DISPLAY1` (any case; ASCII digits).
pub fn is_display_device(screen: &str) -> bool {
    let Some(rest) = screen.strip_prefix(r"\\.\") else {
        return false;
    };
    let Some((word, digits)) = rest.split_at_checked(7) else {
        return false;
    };
    word.eq_ignore_ascii_case("DISPLAY") && !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// `str(OSError(code, text))`.
fn os_error(code: u32, text: &str) -> String {
    format!("[Errno {code}] {text}")
}

/// Port of luma/display_profiles.py `windows_profile_path` over the given calls: the profile file Windows gives
/// the screen, `None` for the system default (sRGB), or the error text the oracle's `resolve` shows.
pub fn profile_path_with(gdi: &mut dyn Gdi, screen: &str) -> Result<Option<String>, String> {
    if screen.is_empty() {
        return Ok(None);
    }
    if !is_display_device(screen) {
        return Err("Windows 모니터 이름을 확인할 수 없습니다.".into());
    }
    if !gdi.create_dc(screen) {
        let code = gdi.last_error();
        return Err(os_error(code, "모니터 정보를 읽을 수 없습니다."));
    }
    let result = query_path(gdi);
    gdi.delete_dc();
    result
}

/// The body of `windows_profile_path` between `CreateDCW` and `DeleteDC`.
fn query_path(gdi: &mut dyn Gdi) -> Result<Option<String>, String> {
    let failed = |code: u32| os_error(code, "Windows 색상 프로파일을 읽을 수 없습니다.");
    let mut size = 0u32;
    gdi.set_last_error(0);
    let ok = gdi.query(&mut size, None);
    if size == 0 {
        let code = gdi.last_error();
        if ok || NO_PROFILE.contains(&code) {
            return Ok(None);
        }
        return Err(failed(code));
    }
    if size > MAX_PATH_UNITS {
        return Err("모니터 프로파일 경로가 너무 깁니다.".into());
    }
    for _ in 0..2 {
        let capacity = size;
        let mut buffer = vec![0u16; capacity as usize];
        if gdi.query(&mut size, Some(&mut buffer)) {
            // `buffer.value`: up to the first NUL.
            let end = buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len());
            let path = String::from_utf16_lossy(&buffer[..end]);
            return Ok((!path.is_empty()).then_some(path));
        }
        let code = gdi.last_error();
        if code != INSUFFICIENT_BUFFER || !(capacity < size && size <= MAX_PATH_UNITS) {
            return Err(failed(code));
        }
    }
    Err("Windows 색상 프로파일이 변경 중입니다. 다시 확인해 주세요.".into())
}

/// The profile file Windows gives the monitor named `screen` (`\\.\DISPLAYn`), `None` for the system default
/// (sRGB) and on systems other than Windows (the oracle's `os.name!='nt' or not screen`). Read-only; cheap but it
/// opens a device context: call it from a background job, as the oracle does.
pub fn windows_profile_path(screen: &str) -> Result<Option<String>, String> {
    #[cfg(windows)]
    {
        profile_path_with(&mut windows::SystemGdi::default(), screen)
    }
    #[cfg(not(windows))]
    {
        let _ = screen;
        Ok(None)
    }
}

/// The device name (`\\.\DISPLAYn`) of the monitor that holds most of the window `handle` (a Windows `HWND` as a
/// number, from the window toolkit), `None` without one or on other systems. Cheap: fine once per frame.
pub fn screen_of_window(handle: isize) -> Option<String> {
    #[cfg(windows)]
    {
        windows::screen_of_window(handle)
    }
    #[cfg(not(windows))]
    {
        let _ = handle;
        None
    }
}

/// The device name of the monitor at (or nearest to) the desktop point `x`, `y` in physical pixels, `None` on
/// other systems.
pub fn screen_at(x: i32, y: i32) -> Option<String> {
    #[cfg(windows)]
    {
        windows::screen_at(x, y)
    }
    #[cfg(not(windows))]
    {
        let _ = (x, y);
        None
    }
}

/// The system calls: the only `unsafe` code of grainy-platform.
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows {
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{GetLastError, SetLastError, HWND, POINT};
    use windows_sys::Win32::Graphics::Gdi::{
        CreateDCW, DeleteDC, GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HDC, HMONITOR, MONITORINFO,
        MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
    };
    use windows_sys::Win32::UI::ColorSystem::GetICMProfileW;

    use super::Gdi;

    /// A display device context, deleted at the latest when dropped.
    pub(super) struct SystemGdi {
        dc: HDC,
    }

    impl Default for SystemGdi {
        fn default() -> Self {
            Self { dc: null_mut() }
        }
    }

    impl Gdi for SystemGdi {
        fn create_dc(&mut self, screen: &str) -> bool {
            self.delete_dc();
            let name: Vec<u16> = screen.encode_utf16().chain(std::iter::once(0)).collect();
            // SAFETY: `name` is a NUL-terminated UTF-16 string that lives until the call returns; the port and the
            // device mode may be null. The returned handle is owned by `self` and deleted once by `delete_dc`.
            self.dc = unsafe { CreateDCW(name.as_ptr(), name.as_ptr(), null(), null()) };
            !self.dc.is_null()
        }

        fn set_last_error(&mut self, code: u32) {
            // SAFETY: sets the calling thread's last-error value; no memory is involved.
            unsafe { SetLastError(code) }
        }

        fn last_error(&mut self) -> u32 {
            // SAFETY: reads the calling thread's last-error value; no memory is involved.
            unsafe { GetLastError() }
        }

        fn query(&mut self, size: &mut u32, buffer: Option<&mut [u16]>) -> bool {
            if self.dc.is_null() {
                return false;
            }
            let pointer = match buffer {
                Some(buffer) => {
                    // Never let the system write past the buffer.
                    *size = (*size).min(u32::try_from(buffer.len()).unwrap_or(u32::MAX));
                    buffer.as_mut_ptr()
                }
                None => null_mut(),
            };
            // SAFETY: `self.dc` is a live device context from `CreateDCW`; `size` is a valid `u32`; `pointer` is
            // null (size query) or points to a writable buffer of at least `*size` UTF-16 units (clamped above).
            unsafe { GetICMProfileW(self.dc, size, pointer) != 0 }
        }

        fn delete_dc(&mut self) {
            if !self.dc.is_null() {
                // SAFETY: `self.dc` came from `CreateDCW` and is deleted exactly once (set to null right after).
                unsafe { DeleteDC(self.dc) };
                self.dc = null_mut();
            }
        }
    }

    impl Drop for SystemGdi {
        fn drop(&mut self) {
            self.delete_dc();
        }
    }

    /// `GetMonitorInfoW` with the extended structure: the monitor's device name.
    fn device_name(monitor: HMONITOR) -> Option<String> {
        if monitor.is_null() {
            return None;
        }
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: `monitor` is a monitor handle from `MonitorFromWindow` / `MonitorFromPoint`; `info` is a
        // `MONITORINFOEXW` whose `cbSize` says so, which is how the system knows it may write the device name.
        let ok = unsafe { GetMonitorInfoW(monitor, (&raw mut info).cast::<MONITORINFO>()) };
        if ok == 0 {
            return None;
        }
        let end = info.szDevice.iter().position(|&unit| unit == 0).unwrap_or(info.szDevice.len());
        let name = String::from_utf16_lossy(&info.szDevice[..end]);
        (!name.is_empty()).then_some(name)
    }

    pub(super) fn screen_of_window(handle: isize) -> Option<String> {
        if handle == 0 {
            return None;
        }
        // SAFETY: the window handle is only looked up by the system (a closed or foreign window gives the nearest
        // monitor or null; nothing is dereferenced); no memory is passed.
        let monitor = unsafe { MonitorFromWindow(handle as HWND, MONITOR_DEFAULTTONEAREST) };
        device_name(monitor)
    }

    pub(super) fn screen_at(x: i32, y: i32) -> Option<String> {
        // SAFETY: a point passed by value; no memory is involved.
        let monitor = unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST) };
        device_name(monitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// The scripted gdi32 of `tools/golden/gen_app_monitor.py`.
    struct Script {
        create: bool,
        create_error: u32,
        answers: Vec<Value>,
        error: u32,
        queries: usize,
        created: bool,
        deleted: bool,
    }

    impl Script {
        fn new(case: &Value) -> Self {
            Self {
                create: case.get("create").and_then(Value::as_u64).unwrap_or(1) != 0,
                create_error: case.get("create_error").and_then(Value::as_u64).unwrap_or(0) as u32,
                answers: case.get("answers").and_then(Value::as_array).cloned().unwrap_or_default(),
                error: 0,
                queries: 0,
                created: false,
                deleted: false,
            }
        }
    }

    impl Gdi for Script {
        fn create_dc(&mut self, _screen: &str) -> bool {
            self.created = true;
            if !self.create {
                self.error = self.create_error;
            }
            self.create
        }

        fn set_last_error(&mut self, code: u32) {
            self.error = code;
        }

        fn last_error(&mut self) -> u32 {
            self.error
        }

        fn query(&mut self, size: &mut u32, buffer: Option<&mut [u16]>) -> bool {
            let answer = self.answers.remove(0);
            self.queries += 1;
            *size = answer.get("size").and_then(Value::as_u64).unwrap_or(0) as u32;
            if let Some(code) = answer.get("error").and_then(Value::as_u64) {
                self.error = code as u32;
            }
            let ok = answer.get("ok").and_then(Value::as_u64).unwrap_or(0) != 0;
            if let (true, Some(buffer)) = (ok, buffer) {
                let value = answer.get("value").and_then(Value::as_str).unwrap_or("");
                let units: Vec<u16> = value.encode_utf16().collect();
                buffer.fill(0);
                buffer[..units.len()].copy_from_slice(&units);
            }
            ok
        }

        fn delete_dc(&mut self) {
            self.deleted = true;
        }
    }

    #[test]
    fn the_profile_query_follows_the_oracles_windows_profile_path() {
        let golden = grainy_testkit::Case::open("app", "monitor_query");
        let cases = golden.json()["cases"].as_array().expect("cases");
        assert!(cases.len() >= 20);
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let screen = case["screen"].as_str().unwrap();
            let mut script = Script::new(case);
            let result = profile_path_with(&mut script, screen);
            match case.get("error").and_then(Value::as_str) {
                Some(error) => assert_eq!(result, Err(error.to_owned()), "{name}"),
                None => assert_eq!(result, Ok(case["path"].as_str().map(str::to_owned)), "{name}"),
            }
            assert_eq!(script.queries as u64, case["queries"].as_u64().unwrap(), "{name}: queries");
            assert_eq!(script.deleted, case["deleted"].as_bool().unwrap(), "{name}: DeleteDC");
            assert_eq!(script.created, case["created"].as_bool().unwrap(), "{name}: CreateDCW");
        }
    }

    #[test]
    fn device_names() {
        for good in [r"\\.\DISPLAY1", r"\\.\display12", r"\\.\Display0"] {
            assert!(is_display_device(good), "{good}");
        }
        for bad in [
            "",
            "DISPLAY1",
            r"\\.\DISPLAY",
            r"\\.\DISPLAY1 ",
            r"\\.\DISPLAYx",
            r"\\?\DISPLAY1",
            r"\\.\DISPLAY-1",
            r"\\.\디스플레이1",
        ] {
            assert!(!is_display_device(bad), "{bad}");
        }
    }

    /// The smoke test of the real calls: whatever this machine's monitors and profiles are, the calls return.
    #[test]
    fn the_system_answers_without_crashing() {
        let screen = screen_at(0, 0);
        if cfg!(windows) {
            let screen = screen.expect("Windows names the primary monitor");
            assert!(is_display_device(&screen), "{screen}");
            match windows_profile_path(&screen) {
                Ok(Some(path)) => assert!(!path.is_empty()),
                Ok(None) => {}
                // An answer, not a crash (an unusual driver may refuse); the text is what the button would show.
                Err(error) => eprintln!("the profile query answered with an error: {error}"),
            }
        } else {
            assert_eq!(screen, None);
            assert_eq!(windows_profile_path(r"\\.\DISPLAY1"), Ok(None));
        }
        // A handle that is no window: the nearest monitor or nothing, never a crash.
        let _ = screen_of_window(0x7fff_fff0);
        assert_eq!(screen_of_window(0), None);
        assert_eq!(windows_profile_path(""), Ok(None));
    }
}
