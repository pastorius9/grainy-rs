//! Port of `luma/native_gpu.py`: float32 compute stages and an explicitly owned resident stage cache.
//! `Ok(None)` requests CPU processing; errors describe invalid input, resource limits or a lost device.
//! Input is scene-linear RGB; tone returns encoded RGB. No photo library or application settings are read.

mod detail;
mod device;
mod film;
mod geometry;
mod local;
mod params;
mod pool;
mod present;
mod resident;

pub use device::{AdapterKind, Frame, Gpu};
pub use geometry::GpuGeometryParams;
pub use local::{can_gpu_local_layer, LocalMask};
pub use params::TileContext;
pub use present::{lut33_input_grid, pack_lut33, MonitorLut, PresentCrop, Presented, LUT_ENTRIES, LUT_SIZE};
pub use resident::{CpuStage, Resident, ResidentResult, StageRuns};
pub use wgpu;

/// Errors of the `luma/native_gpu.py` device/frame and parameter boundary.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Invalid RGB dimensions, non-finite pixels or a malformed setting.
    #[error("invalid GPU input: {0}")]
    Input(String),
    /// The adapter cannot hold or dispatch this frame.
    #[error("GPU resource limit: {0}")]
    Limit(String),
    /// The device was lost or a GPU operation failed; create a new context or use the CPU.
    #[error("GPU unavailable: {0}")]
    Device(String),
    /// A frame belongs to a different context.
    #[error("frame belongs to another GPU context")]
    ForeignFrame,
}

/// Result of a port of a `luma/native_gpu.py` operation.
pub type Result<T> = std::result::Result<T, Error>;

/// Whether the GPU parity tests are switched off on this machine: only when the environment variable
/// `GRAINY_CI_NO_GPU` is `1`. The CI workflow sets it for the Windows runner, which has no graphics card: its only
/// adapter, WARP of Windows Server 2022 (driver 10.0.20348), removes the device in the first stage of the noise
/// reduction (the Lab conversion); the same stages pass on WARP of Windows 11, on Metal and on hardware adapters. Everywhere else
/// the tests run. Says so loudly on standard error (not captured by the test harness) when it skips.
pub fn gpu_tests_switched_off(test: &str) -> bool {
    let off = std::env::var_os("GRAINY_CI_NO_GPU").is_some_and(|v| v == "1");
    if off {
        use std::io::Write;
        let _ = writeln!(
            std::io::stderr(),
            "!!! SKIPPED GPU TEST {test}: GRAINY_CI_NO_GPU=1 (CI runner without a usable GPU)"
        );
    }
    off
}
