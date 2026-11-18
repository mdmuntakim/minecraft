//! The loader's error type is the format crate's, so a failure keeps one code
//! and one sentence from the container reader to the launcher.

pub use fabric_mod_format::{ErrorCode, FabricError};

/// A loader failure.
pub type LoaderError = FabricError;

/// A loader result.
pub type LoaderResult<T> = fabric_mod_format::Result<T>;

/// Builds a loader error with a stable code.
pub fn error(code: ErrorCode, message: impl Into<String>) -> LoaderError {
    FabricError::new(code, message)
}

/// Builds a loader error with a developer detail line.
pub fn error_with(
    code: ErrorCode,
    message: impl Into<String>,
    detail: impl Into<String>,
) -> LoaderError {
    FabricError::with_detail(code, message, detail)
}
