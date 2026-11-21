//! A separate Windows scRGB surface for the HDR photo. All HWND and Direct3D objects stay on the UI thread.

use std::{ffi::c_void, marker::PhantomData, ptr::NonNull, rc::Rc, time::Duration};

use eframe::egui::{Ui, Vec2};

use crate::{
    context::AppContext,
    hdr,
    text::{tr, tr_with},
    widgets,
};

unsafe extern "C" {
    fn grainy_hdr_view_open(owner: isize, viewer: *mut *mut c_void) -> i32;
    fn grainy_hdr_view_drop(viewer: *mut c_void);
    fn grainy_hdr_view_state(viewer: *mut c_void, w: *mut u32, h: *mut u32, stops: *mut f32) -> i32;
    fn grainy_hdr_view_frame(viewer: *mut c_void, pixels: *const f32, w: u32, h: u32) -> i32;
}

fn error(hr: i32) -> String {
    if hr as u32 == 0x887a0004 {
        tr("Windows에서 HDR을 켜고 HDR 모니터의 밝기 정보를 확인해 주세요.")
    } else {
        tr_with("HDR 미리보기를 열 수 없습니다 ({0}).", &[&format!("0x{:08X}", hr as u32)])
    }
}

struct Window(NonNull<c_void>, PhantomData<Rc<()>>);

impl Window {
    fn open(owner: isize) -> Result<Self, String> {
        let mut pointer = std::ptr::null_mut();
        // The handle comes from eframe. Native code checks it and creates an owned viewer on this thread.
        let hr = unsafe { grainy_hdr_view_open(owner, &mut pointer) };
        NonNull::new(pointer).map(|p| Self(p, PhantomData)).ok_or_else(|| error(hr))
    }

    fn state(&self) -> Result<Option<(Vec2, f32)>, String> {
        let (mut w, mut h, mut stops) = (0, 0, 0.);
        // The window pointer remains owned until Drop; all out-parameters live through this call.
        match unsafe { grainy_hdr_view_state(self.0.as_ptr(), &mut w, &mut h, &mut stops) } {
            0 => Ok(Some((Vec2::new(w as f32, h as f32), stops))),
            1 => Ok(None),
            hr => Err(error(hr)),
        }
    }

    fn frame(&self, pixels: Option<&[f32]>, width: usize, height: usize) -> Result<(), String> {
        let ptr = match pixels {
            None => std::ptr::null(),
            Some(p)
                if width > 0 && width <= 16384 && height > 0 && height <= 16384 && p.len() == width * height * 4 =>
            {
                p.as_ptr()
            }
            Some(_) => return Err(error(0x80070057u32 as i32)),
        };
        // Direct3D copies the validated slice in this call. A null pointer hides the preceding photo.
        let hr = unsafe { grainy_hdr_view_frame(self.0.as_ptr(), ptr, width as u32, height as u32) };
        if hr < 0 {
            Err(error(hr))
        } else {
            Ok(())
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        // Non-Send, unique ownership ties destruction to the creating UI thread.
        unsafe { grainy_hdr_view_drop(self.0.as_ptr()) };
    }
}

#[derive(Default)]
struct State {
    owner: isize,
    window: Option<Window>,
    size: Vec2,
    stops: f32,
    // Photo IDs are scoped to a document. Keep both keys with the rendered frame ID.
    uploaded: Option<(u64, u64, u64)>,
    message: Option<String>,
}

pub(crate) fn bind(cx: &mut AppContext, owner: isize) {
    cx.state::<State>().owner = owner;
}

fn close(cx: &mut AppContext, message: Option<String>) {
    let state = cx.state::<State>();
    state.window = None;
    state.uploaded = None;
    state.message = message;
    let view = hdr::View { native_preview: false, ..cx.state::<hdr::HdrState>().view };
    hdr::set_view(cx, view);
}

pub(crate) fn button(ui: &mut Ui, cx: &mut AppContext) {
    let state = cx.state::<State>();
    if state.owner == 0 {
        return;
    }
    let open = state.window.is_some();
    let label = if open { "HDR 모니터 창 닫기" } else { "HDR 모니터 창 열기" };
    if widgets::button(ui, &tr(label)).clicked() {
        if open {
            close(cx, None);
        } else {
            match Window::open(state.owner) {
                Ok(window) => {
                    state.window = Some(window);
                    state.message = None;
                    let view = hdr::View { native_preview: true, ..cx.state::<hdr::HdrState>().view };
                    hdr::set_view(cx, view);
                }
                Err(message) => state.message = Some(message),
            }
        }
    }
    let state = cx.state::<State>();
    if let Some(message) = &state.message {
        ui.label(message);
    } else if state.window.is_some() && state.stops > 0. {
        widgets::muted(ui, &tr_with("HDR 모니터 · +{0} EV", &[&format!("{:.1}", state.stops)]));
    }
}

pub(crate) fn fitted_side(cx: &AppContext, photo: Vec2) -> usize {
    let Some(s) = cx.state_ref::<State>().filter(|s| s.window.is_some()) else { return 0 };
    let scale = (s.size.x / photo.x.max(1.)).min(s.size.y / photo.y.max(1.));
    (photo.x.max(photo.y) * scale).ceil() as usize
}

pub(crate) fn poll(cx: &mut AppContext) {
    if !cx.state_ref::<State>().is_some_and(|s| s.window.is_some()) {
        return;
    }
    if cx.doc.current().is_none() || !cx.settings().bool("hdr") {
        close(cx, None);
        return;
    }
    let photo = cx.doc.current_id();
    let document = cx.document_generation;
    let frame = cx.frame.clone();
    let state = cx.state::<State>();
    let window = state.window.as_ref().unwrap();
    match window.state() {
        Ok(Some((size, stops))) => {
            state.size = size;
            state.stops = stops;
        }
        Ok(None) => {
            close(cx, None);
            return;
        }
        Err(message) => {
            close(cx, Some(message));
            return;
        }
    }
    if state.uploaded.is_some_and(|(doc, id, _)| doc != document || id != photo) {
        if let Err(message) = window.frame(None, 0, 0) {
            close(cx, Some(message));
            return;
        }
        state.uploaded = None;
    }
    if let Some(f) = frame.filter(|f| f.photo == photo && state.uploaded != Some((document, photo, f.id))) {
        if let Some(pixels) = &f.native_hdr {
            if let Err(message) = window.frame(Some(pixels), f.preview.width, f.preview.height) {
                close(cx, Some(message));
                return;
            }
            state.uploaded = Some((document, photo, f.id));
        }
    }
    cx.egui.request_repaint_after(Duration::from_millis(250));
}

#[cfg(test)]
#[test]
fn fp16_swap_chain_keeps_values_above_sdr_white() {
    unsafe extern "C" {
        fn grainy_hdr_test_readback(samples: *mut u16) -> i32;
    }
    let mut samples = [0u16; 16];
    // The hidden diagnostic writes exactly four RGBA16F pixels, without presenting to a physical monitor.
    let hr = unsafe { grainy_hdr_test_readback(samples.as_mut_ptr()) };
    assert_eq!(hr, 0, "offscreen HDR HRESULT {hr:#x}");
    assert_eq!([samples[0], samples[4], samples[8], samples[12]], [0x3800, 0x3c00, 0x4000, 0x4400]);
}
