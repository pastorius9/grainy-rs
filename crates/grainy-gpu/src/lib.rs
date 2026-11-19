//! Port of `luma/native_gpu.py`: float32 compute stages and an explicitly owned resident stage cache.
//! `Ok(None)` requests CPU processing; errors describe invalid input, resource limits or a lost device.
//! Input is scene-linear RGB; tone returns encoded RGB. No photo library or application settings are read.

mod detail;
mod device;
mod params;
mod pool;
mod resident;

pub use device::{AdapterKind, Frame, Gpu};
pub use resident::{CpuStage, Resident, ResidentResult, StageRuns};

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
