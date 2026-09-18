//! Windows-only OS toast notifications for an unpackaged Skein build
//! (#155): a registered `AppUserModelId` + `CustomActivator` COM
//! class so a click reaches Skein even from Action Center or a cold
//! `-Embedding` launch, and a foreground-window recovery helper for
//! the case where `SetForegroundWindow` alone is refused.
//!
//! Tauri-free and sync, like the other workspace crates — but unlike
//! them, this one is the workspace's one deliberate exception to
//! `unsafe_code = "forbid"` (see `Cargo.toml`): registry writes, COM
//! class registration and the `WinRT` toast APIs all require raw FFI
//! calls the `windows` crate cannot make safe. `app/src-tauri` keeps
//! the workspace's `forbid` and only ever calls the safe functions
//! below.
//!
//! Everything genuinely Windows-specific — registry, COM, `WinRT` — is
//! `#[cfg(windows)]`; on macOS/Linux this crate compiles down to just
//! [`clsid_for`]/[`clsid_braced`]/[`parse_launch`], which are pure
//! and OS-agnostic on purpose so they stay unit-tested on every CI
//! platform, not only Windows.

mod clsid;
mod launch;

pub use clsid::{clsid_braced, clsid_for};
pub use launch::{LaunchTarget, parse_launch};

#[cfg(windows)]
mod activator;
#[cfg(windows)]
mod error;
#[cfg(windows)]
mod foreground;
#[cfg(windows)]
mod registry;
#[cfg(windows)]
mod toast;

#[cfg(windows)]
pub use activator::{ActivatorHandle, start_activator};
#[cfg(windows)]
pub use error::{Error, Result};
#[cfg(windows)]
pub use foreground::bring_to_front;
#[cfg(windows)]
pub use registry::register_app;
#[cfg(windows)]
pub use toast::show_toast;
