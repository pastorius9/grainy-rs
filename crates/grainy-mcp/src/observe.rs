//! A test hook: the pixels handed to `photo_images.jpeg`, in call order (the oracle's goldens record the same).
//! Off unless a test switches it on; process-wide, so one test at a time may use it.

use std::sync::Mutex;

use ndarray::Array3;

use crate::util::lock;

static SEEN: Mutex<Option<Vec<Array3<u8>>>> = Mutex::new(None);

/// Start (or stop) recording.
pub fn record(on: bool) {
    *lock(&SEEN) = on.then(Vec::new);
}

/// The images recorded since the last call, emptying the record.
pub fn take() -> Vec<Array3<u8>> {
    lock(&SEEN).as_mut().map(std::mem::take).unwrap_or_default()
}

pub(crate) fn seen(image: &Array3<u8>) {
    if let Some(list) = lock(&SEEN).as_mut() {
        list.push(image.clone());
    }
}
