//! The monitor a window is on and the colour profile the system gives that monitor: the operating-system half of
//! luma/display_profiles.py (`windows_profile_path`, `mac_display_id`, `mac_profile_data`) and of display_color.py
//! `DisplayTarget.screen_name`.
//!
//! * Windows: Qt names a screen by its GDI device name, `\\.\DISPLAYn` (here from `GetMonitorInfoW`); the logic of
//!   the oracle's `GetICMProfileW` loop is [`profile_path_with`], over the [`Gdi`] calls, tested against the oracle
//!   with a scripted stand-in (`golden/app/monitor_query`).
//! * macOS: the system hands over the profile's data, not a file. A screen is named `'Name (id)'` with its
//!   CoreGraphics display number; [`display_of_window`] picks the display under the window from the window's place
//!   and its monitor's size (what egui knows), and [`profile_data_with`] is `mac_profile_data` over the
//!   [`ColorSync`] calls (`CGDisplayCopyColorSpace`, `CGColorSpaceCopyICCData`, `CFData`, `CFRelease`). Both pure
//!   parts are tested on every system.
//!
//! The real calls are in the private modules `windows` and `macos`, the only `unsafe` code of this crate;
//! read-only, as in the oracle: no profile is installed, associated or enumerated. On other systems there is no
//! lookup: every answer is "the system default" (sRGB).

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

// ---- macOS: the display under the window and the profile ColorSync gives it ------------------------------------------

/// `display_profiles.MAX_PROFILE_BYTES`: the largest display profile accepted (16 MB).
pub const MAX_PROFILE_BYTES: usize = 16 * 1024 * 1024;

/// A display's bounds in CoreGraphics' global display space (`CGDisplayBounds`): points, the origin at the top left
/// of the main display, y growing downwards (the space winit's window positions are in on macOS).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayBounds {
    /// The `CGDirectDisplayID`.
    pub id: u32,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// `CGDisplayIsBuiltin`: the computer's own screen.
    pub builtin: bool,
}

/// The match of luma/display_profiles.py `mac_display_id`: the first display whose bounds, each rounded as Python's
/// `round` does (halves to even), equal the screen geometry `x`, `y`, `width`, `height`.
pub fn display_matching(displays: &[DisplayBounds], x: i64, y: i64, width: i64, height: i64) -> Option<u32> {
    let round = |v: f64| v.round_ties_even();
    displays
        .iter()
        .find(|d| {
            [round(d.x), round(d.y), round(d.width), round(d.height)]
                == [x as f64, y as f64, width as f64, height as f64]
        })
        .map(|d| d.id)
}

/// The display a window is on, from what the window toolkit tells: its outer rectangle `[left, top, right, bottom]`
/// and the size of its monitor `[width, height]`, both in points of the global display space. Qt's `QWindow.screen`
/// on macOS is the screen holding the largest part of the window, and the oracle asks for the display whose bounds
/// are that screen's geometry (`mac_display_id`); egui knows the monitor's size but not its place, so: of the
/// displays of that size (rounded as the oracle rounds; all of them when the size is unknown or matches none), the
/// one the window overlaps most, the first of equals. `None` when the window overlaps no display.
pub fn display_of_window(displays: &[DisplayBounds], window: [f64; 4], monitor: Option<[f64; 2]>) -> Option<u32> {
    let size = |d: &DisplayBounds| [d.width.round_ties_even(), d.height.round_ties_even()];
    let sized: Vec<&DisplayBounds> = match monitor {
        Some([w, h]) => displays.iter().filter(|d| size(d) == [w.round_ties_even(), h.round_ties_even()]).collect(),
        None => Vec::new(),
    };
    let candidates: Vec<&DisplayBounds> = if sized.is_empty() { displays.iter().collect() } else { sized };
    let overlap = |d: &DisplayBounds| {
        let w = window[2].min(d.x + d.width) - window[0].max(d.x);
        let h = window[3].min(d.y + d.height) - window[1].max(d.y);
        if w > 0.0 && h > 0.0 {
            w * h
        } else {
            0.0
        }
    };
    let mut best: Option<(&DisplayBounds, f64)> = None;
    for display in candidates {
        let area = overlap(display);
        if area > 0.0 && best.is_none_or(|(_, most)| area > most) {
            best = Some((display, area));
        }
    }
    best.map(|(d, _)| d.id)
}

/// The screen name the profile is asked for: `'Name (id)'`, as `DisplayTarget.screen_name` names a macOS screen
/// (Qt's screen name, then the display number). The toolkit gives no display name here, so the name says whether
/// it is the computer's own screen.
pub fn mac_screen_name(display: &DisplayBounds) -> String {
    let name = if display.builtin { "Built-in Display" } else { "Display" };
    format!("{name} ({})", display.id)
}

/// The display number of a screen named `'Name (id)'` (`re.search(r'\((\d+)\)$', screen)`; `$` also matches before
/// one final newline). The number is cut to 32 bits as ctypes' `c_uint32` argument cuts it.
pub fn display_number(screen: &str) -> Option<u32> {
    let text = screen.strip_suffix('\n').unwrap_or(screen);
    let inner = text.strip_suffix(')')?;
    let open = inner.rfind('(')?;
    let digits = &inner[open + 1..];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(digits.bytes().fold(0u32, |n, b| n.wrapping_mul(10).wrapping_add(u32::from(b - b'0'))))
}

/// The calls of `mac_profile_data` on one display: `CGDisplayCopyColorSpace`, `CGColorSpaceCopyICCData`,
/// `CFDataGetLength`, `CFDataGetBytePtr` and the two `CFRelease`. The real ones are the system's; tests script them.
pub trait ColorSync {
    /// `CGDisplayCopyColorSpace(display)`: whether the display gave a colour space (held until
    /// [`ColorSync::release_space`]).
    fn copy_color_space(&mut self, display: u32) -> bool;
    /// `CGColorSpaceCopyICCData` of the held colour space: whether it gave data (held until
    /// [`ColorSync::release_data`]).
    fn copy_icc_data(&mut self) -> bool;
    /// `CFDataGetLength` of the held data.
    fn data_length(&mut self) -> usize;
    /// `ctypes.string_at(CFDataGetBytePtr(data), length)`.
    fn data_bytes(&mut self, length: usize) -> Vec<u8>;
    /// `CFRelease(data)`.
    fn release_data(&mut self);
    /// `CFRelease(space)`.
    fn release_space(&mut self);
}

/// Port of luma/display_profiles.py `mac_profile_data` over the given calls: the ICC bytes of the display named
/// `'Name (id)'`, `None` without a number, a colour space or data (the system default), or the oracle's error for
/// a profile above [`MAX_PROFILE_BYTES`]. Both copies are released on every path.
pub fn profile_data_with(cs: &mut dyn ColorSync, screen: &str) -> Result<Option<Vec<u8>>, String> {
    let Some(display) = display_number(screen) else { return Ok(None) };
    if !cs.copy_color_space(display) {
        return Ok(None);
    }
    let result = if cs.copy_icc_data() {
        let size = cs.data_length();
        let result = if size > MAX_PROFILE_BYTES {
            Err("모니터 프로파일이 너무 큽니다.".to_owned())
        } else {
            Ok(Some(cs.data_bytes(size)))
        };
        cs.release_data();
        result
    } else {
        Ok(None)
    };
    cs.release_space();
    result
}

/// Port of luma/display_profiles.py `mac_display_id(x, y, width, height)`: the CoreGraphics display whose bounds
/// are a screen's geometry (points, origin top left); `None` without one and on other systems.
pub fn mac_display_id(x: i64, y: i64, width: i64, height: i64) -> Option<u32> {
    display_matching(&active_displays(), x, y, width, height)
}

/// Port of luma/display_profiles.py `mac_profile_data(screen)`: the ICC bytes macOS gives the display named
/// `'Name (id)'` (the profile itself, not a file), `None` for the system default and on other systems. Read-only;
/// call it from a background job, as the oracle does.
pub fn mac_profile_data(screen: &str) -> Result<Option<Vec<u8>>, String> {
    #[cfg(target_os = "macos")]
    {
        profile_data_with(&mut macos::SystemColorSync::default(), screen)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = screen;
        Ok(None)
    }
}

/// The active displays (`CGGetActiveDisplayList`, at most 32 as the oracle asks) with their bounds; empty on other
/// systems or when the system refuses. Cheap: fine whenever the window moved.
pub fn active_displays() -> Vec<DisplayBounds> {
    #[cfg(target_os = "macos")]
    {
        macos::active_displays()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

/// The macOS screen name (`'Name (id)'`, see [`mac_screen_name`]) of the display a window is on
/// ([`display_of_window`] over the active displays); `None` when no display holds the window and on other systems.
pub fn mac_screen_of_window(window: [f64; 4], monitor: Option<[f64; 2]>) -> Option<String> {
    let displays = active_displays();
    let id = display_of_window(&displays, window, monitor)?;
    displays.iter().find(|d| d.id == id).map(mac_screen_name)
}

/// The CoreGraphics and CoreFoundation calls: the macOS half of the only `unsafe` code of grainy-platform.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos {
    use std::ffi::c_void;

    use super::{ColorSync, DisplayBounds};

    /// `CGPoint` / `CGSize` (two `CGFloat`s, `double` on 64-bit macOS).
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Pair {
        a: f64,
        b: f64,
    }

    /// `CGRect`: origin and size.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CgRect {
        origin: Pair,
        size: Pair,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGGetActiveDisplayList(max_displays: u32, active_displays: *mut u32, display_count: *mut u32) -> i32;
        fn CGDisplayBounds(display: u32) -> CgRect;
        fn CGDisplayIsBuiltin(display: u32) -> u32;
        fn CGDisplayCopyColorSpace(display: u32) -> *mut c_void;
        fn CGColorSpaceCopyICCData(space: *mut c_void) -> *const c_void;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFDataGetLength(data: *const c_void) -> isize;
        fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
        fn CFRelease(object: *const c_void);
    }

    /// `CGGetActiveDisplayList(32, ids, &count)` and `CGDisplayBounds` of each.
    pub(super) fn active_displays() -> Vec<DisplayBounds> {
        let mut ids = [0u32; 32];
        let mut count = 0u32;
        // SAFETY: `ids` has room for the 32 displays the call is allowed to write and `count` is a valid `u32`;
        // both outlive the call. A non-zero `CGError` leaves them unused.
        let error = unsafe { CGGetActiveDisplayList(32, ids.as_mut_ptr(), &raw mut count) };
        if error != 0 {
            return Vec::new();
        }
        ids[..(count as usize).min(ids.len())]
            .iter()
            .map(|&id| {
                // SAFETY: plain values in and out: a display number (an unknown one gives an empty rectangle and
                // false), nothing is dereferenced or kept.
                let (bounds, builtin) = unsafe { (CGDisplayBounds(id), CGDisplayIsBuiltin(id) != 0) };
                DisplayBounds {
                    id,
                    x: bounds.origin.a,
                    y: bounds.origin.b,
                    width: bounds.size.a,
                    height: bounds.size.b,
                    builtin,
                }
            })
            .collect()
    }

    /// The colour space and its ICC data of one query, released at the latest when dropped.
    pub(super) struct SystemColorSync {
        space: *mut c_void,
        data: *const c_void,
    }

    impl Default for SystemColorSync {
        fn default() -> Self {
            Self { space: std::ptr::null_mut(), data: std::ptr::null() }
        }
    }

    impl ColorSync for SystemColorSync {
        fn copy_color_space(&mut self, display: u32) -> bool {
            self.release_data();
            self.release_space();
            // SAFETY: a display number by value (an unknown one gives null). The returned colour space follows the
            // Copy rule: it is owned by `self` and released once by `release_space`.
            self.space = unsafe { CGDisplayCopyColorSpace(display) };
            !self.space.is_null()
        }

        fn copy_icc_data(&mut self) -> bool {
            if self.space.is_null() {
                return false;
            }
            self.release_data();
            // SAFETY: `self.space` is a live colour space from `CGDisplayCopyColorSpace`. The returned data follows
            // the Copy rule: it is owned by `self` and released once by `release_data`.
            self.data = unsafe { CGColorSpaceCopyICCData(self.space) };
            !self.data.is_null()
        }

        fn data_length(&mut self) -> usize {
            if self.data.is_null() {
                return 0;
            }
            // SAFETY: `self.data` is a live `CFData` from `CGColorSpaceCopyICCData`.
            usize::try_from(unsafe { CFDataGetLength(self.data) }).unwrap_or(0)
        }

        fn data_bytes(&mut self, length: usize) -> Vec<u8> {
            if self.data.is_null() || length == 0 {
                return Vec::new();
            }
            // Never read past the data, whatever length is asked for.
            let length = length.min(self.data_length());
            // SAFETY: `self.data` is a live `CFData`; its byte pointer is valid for `CFDataGetLength` bytes while the
            // data is held (it is released only after this copy), and `length` is at most that.
            unsafe {
                let bytes = CFDataGetBytePtr(self.data);
                if bytes.is_null() {
                    return Vec::new();
                }
                std::slice::from_raw_parts(bytes, length).to_vec()
            }
        }

        fn release_data(&mut self) {
            if !self.data.is_null() {
                // SAFETY: `self.data` came from a Copy call and is released exactly once (set to null right after).
                unsafe { CFRelease(self.data) };
                self.data = std::ptr::null();
            }
        }

        fn release_space(&mut self) {
            if !self.space.is_null() {
                // SAFETY: `self.space` came from a Copy call and is released exactly once (set to null right after).
                unsafe { CFRelease(self.space.cast_const()) };
                self.space = std::ptr::null_mut();
            }
        }
    }

    impl Drop for SystemColorSync {
        fn drop(&mut self) {
            self.release_data();
            self.release_space();
        }
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

    // ---- macOS ------------------------------------------------------------------------------------------------------

    fn display(id: u32, x: f64, y: f64, width: f64, height: f64) -> DisplayBounds {
        DisplayBounds { id, x, y, width, height, builtin: id == 1 }
    }

    #[test]
    fn the_display_with_the_screens_geometry_is_found_as_the_oracle_rounds() {
        let displays = [display(1, 0.0, 0.0, 1512.0, 982.0), display(7, 1512.0, -200.4, 2560.0, 1440.0)];
        assert_eq!(display_matching(&displays, 0, 0, 1512, 982), Some(1));
        assert_eq!(display_matching(&displays, 1512, -200, 2560, 1440), Some(7));
        assert_eq!(display_matching(&displays, 1512, -201, 2560, 1440), None);
        // Python's round: halves to the even neighbour.
        let half = [display(3, 0.5, 1.5, 2.5, -0.5)];
        assert_eq!(display_matching(&half, 0, 2, 2, 0), Some(3));
        assert_eq!(display_matching(&half, 1, 2, 3, -1), None);
        assert_eq!(display_matching(&[], 0, 0, 1, 1), None);
    }

    #[test]
    fn the_window_belongs_to_the_display_holding_most_of_it() {
        // The built-in screen and a larger one to its right, raised a little (CoreGraphics' space: y downwards).
        let displays = [display(1, 0.0, 0.0, 1512.0, 982.0), display(7, 1512.0, -200.0, 2560.0, 1440.0)];
        let on_main = [100.0, 100.0, 900.0, 700.0];
        assert_eq!(display_of_window(&displays, on_main, Some([1512.0, 982.0])), Some(1));
        assert_eq!(display_of_window(&displays, on_main, None), Some(1));
        // Mostly on the second display: its size decides, else the overlap.
        let across = [1200.0, 0.0, 2600.0, 800.0];
        assert_eq!(display_of_window(&displays, across, Some([2560.0, 1440.0])), Some(7));
        assert_eq!(display_of_window(&displays, across, None), Some(7));
        // A monitor size that matches none (a scale change in between): the overlap alone.
        assert_eq!(display_of_window(&displays, across, Some([1920.0, 1080.0])), Some(7));
        // Two displays of the same size: the overlap tells them apart; a tie keeps the first.
        let twins = [display(4, 0.0, 0.0, 1920.0, 1080.0), display(5, 1920.0, 0.0, 1920.0, 1080.0)];
        assert_eq!(display_of_window(&twins, [1800.0, 0.0, 2400.0, 500.0], Some([1920.0, 1080.0])), Some(5));
        assert_eq!(display_of_window(&twins, [1820.0, 0.0, 2020.0, 500.0], Some([1920.0, 1080.0])), Some(4));
        // Off every display, or no display at all: none.
        assert_eq!(display_of_window(&displays, [-900.0, -900.0, -100.0, -100.0], None), None);
        assert_eq!(display_of_window(&[], on_main, Some([1512.0, 982.0])), None);
    }

    #[test]
    fn screen_names_carry_the_display_number() {
        assert_eq!(mac_screen_name(&display(1, 0.0, 0.0, 1.0, 1.0)), "Built-in Display (1)");
        assert_eq!(mac_screen_name(&display(69_733_248, 0.0, 0.0, 1.0, 1.0)), "Display (69733248)");
        for (screen, number) in [
            ("Built-in Display (1)", Some(1)),
            ("Studio Display (2)", Some(2)),
            (" (2)", Some(2)),
            ("A ((12)", Some(12)),
            ("A (1(2)", Some(2)),
            ("Display (3)\n", Some(3)),
            ("Display (4294967297)", Some(1)),
            ("Display", None),
            ("Display ()", None),
            ("Display (3) ", None),
            ("Display (3)\n\n", None),
            ("Display (x3)", None),
            ("Display (-3)", None),
            ("", None),
        ] {
            assert_eq!(display_number(screen), number, "{screen:?}");
        }
    }

    /// A scripted ColorSync: what each call answers, and what was called.
    #[derive(Default)]
    struct Sync {
        space: bool,
        data: Option<Vec<u8>>,
        length: Option<usize>,
        asked: Option<u32>,
        held_space: bool,
        held_data: bool,
        read: usize,
    }

    impl ColorSync for Sync {
        fn copy_color_space(&mut self, display: u32) -> bool {
            self.asked = Some(display);
            self.held_space = self.space;
            self.space
        }
        fn copy_icc_data(&mut self) -> bool {
            self.held_data = self.data.is_some();
            self.held_data
        }
        fn data_length(&mut self) -> usize {
            self.length.unwrap_or_else(|| self.data.as_ref().map_or(0, Vec::len))
        }
        fn data_bytes(&mut self, length: usize) -> Vec<u8> {
            self.read += 1;
            self.data.clone().unwrap_or_default()[..length].to_vec()
        }
        fn release_data(&mut self) {
            assert!(self.held_data, "only held data is released");
            self.held_data = false;
        }
        fn release_space(&mut self) {
            assert!(self.held_space, "only a held colour space is released");
            assert!(!self.held_data, "the data goes first");
            self.held_space = false;
        }
    }

    #[test]
    fn the_profile_data_follows_the_oracles_mac_profile_data() {
        let profile = b"icc profile bytes".to_vec();
        // A display with a profile: its bytes, both copies released.
        let mut sync = Sync { space: true, data: Some(profile.clone()), ..Sync::default() };
        assert_eq!(profile_data_with(&mut sync, "Studio Display (2)"), Ok(Some(profile.clone())));
        assert_eq!(sync.asked, Some(2));
        assert!(!sync.held_space && !sync.held_data);
        // No number in the name: nothing is asked.
        let mut sync = Sync { space: true, data: Some(profile.clone()), ..Sync::default() };
        assert_eq!(profile_data_with(&mut sync, "Studio Display"), Ok(None));
        assert_eq!(sync.asked, None);
        // No colour space, or one without data: the system default.
        let mut sync = Sync::default();
        assert_eq!(profile_data_with(&mut sync, "Display (5)"), Ok(None));
        let mut sync = Sync { space: true, ..Sync::default() };
        assert_eq!(profile_data_with(&mut sync, "Display (5)"), Ok(None));
        assert!(!sync.held_space);
        // Empty data comes back empty (the oracle's `b''`; `resolve` treats it as no profile).
        let mut sync = Sync { space: true, data: Some(Vec::new()), ..Sync::default() };
        assert_eq!(profile_data_with(&mut sync, "Display (5)"), Ok(Some(Vec::new())));
        // Too large: the oracle's error, nothing read, both released.
        let mut sync =
            Sync { space: true, data: Some(profile.clone()), length: Some(MAX_PROFILE_BYTES + 1), ..Sync::default() };
        assert_eq!(profile_data_with(&mut sync, "Display (5)"), Err("모니터 프로파일이 너무 큽니다.".to_owned()));
        assert_eq!(sync.read, 0);
        assert!(!sync.held_space && !sync.held_data);
        let mut sync = Sync { space: true, data: Some(vec![7; 4]), length: Some(4), ..Sync::default() };
        assert_eq!(profile_data_with(&mut sync, "Display (5)"), Ok(Some(vec![7; 4])));
    }

    /// The smoke test of the macOS calls: whatever this Mac's displays are, the calls return; elsewhere nothing is
    /// asked.
    #[test]
    fn the_display_query_answers_without_crashing() {
        let displays = active_displays();
        if cfg!(target_os = "macos") {
            for d in &displays {
                let name = mac_screen_name(d);
                assert_eq!(display_number(&name), Some(d.id));
                match mac_profile_data(&name) {
                    Ok(Some(data)) => assert!(data.len() <= MAX_PROFILE_BYTES),
                    Ok(None) => {}
                    Err(error) => eprintln!("the profile query answered with an error: {error}"),
                }
                let (x, y) = (d.x.round_ties_even() as i64, d.y.round_ties_even() as i64);
                let (w, h) = (d.width.round_ties_even() as i64, d.height.round_ties_even() as i64);
                assert!(mac_display_id(x, y, w, h).is_some());
            }
        } else {
            assert!(displays.is_empty());
            assert_eq!(mac_display_id(0, 0, 1512, 982), None);
            assert_eq!(mac_screen_of_window([0.0, 0.0, 100.0, 100.0], None), None);
        }
        let _ = mac_profile_data("Display (4294967295)");
        assert_eq!(mac_profile_data(""), Ok(None));
    }
}
