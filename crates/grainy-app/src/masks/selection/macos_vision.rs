//! Native macOS Apple Vision subject & portrait segmentation (Neural Engine / Metal).
//! Zero model downloads, 100% offline, with graceful fallback on macOS 11-13 or headless scenes.
use grainy_local::selection::SIDE;

unsafe extern "C" {
    fn grainy_macos_vision_matte(rgb320: *const u8, mode: u32, out_matte: *mut u8) -> i32;
}

fn run(rgb: &[[u8; 3]], mode: u32) -> Option<Vec<u8>> {
    if rgb.len() != SIDE * SIDE {
        return None;
    }
    let mut out = vec![0u8; SIDE * SIDE];
    let rc = unsafe { grainy_macos_vision_matte(rgb.as_ptr().cast::<u8>(), mode, out.as_mut_ptr()) };
    (rc == 1).then_some(out)
}

/// Subject/object foreground matte via macOS 14+ `VNGenerateForegroundInstanceMaskRequest`.
pub(super) fn foreground(rgb: &[[u8; 3]]) -> Option<Vec<u8>> {
    run(rgb, 0)
}

/// Person segmentation matte via macOS 12+ `VNGeneratePersonSegmentationRequest`.
pub(super) fn person(rgb: &[[u8; 3]]) -> Option<Vec<u8>> {
    run(rgb, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vision_bridge_handles_synthetic_frame_cleanly() {
        let blank = vec![[30u8, 60, 90]; SIDE * SIDE];
        // A uniform solid frame has no salient subject or person; bridge must not crash and must return valid len if any.
        if let Some(matte) = foreground(&blank) {
            assert_eq!(matte.len(), SIDE * SIDE);
        }
        if let Some(matte) = person(&blank) {
            assert_eq!(matte.len(), SIDE * SIDE);
        }
    }
}
