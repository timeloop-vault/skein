//! Error type for the Windows-only calls in this crate. Wraps
//! `windows::core::Error` so a caller in `app/src-tauri` never has to
//! bring the `windows` crate into scope itself — it just needs
//! `.to_string()` (or `?` into its own `String`-flattening command
//! boundary, same as every other Tauri command in this codebase).

/// Errors from registry writes, COM class registration, or `WinRT`
/// toast calls.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Windows(#[from] windows::core::Error),
    /// `start_activator`'s dedicated registration thread panicked or
    /// exited before it could report success or failure.
    #[error("activator thread ended without registering")]
    ActivatorThreadDied,
}

pub type Result<T> = std::result::Result<T, Error>;
