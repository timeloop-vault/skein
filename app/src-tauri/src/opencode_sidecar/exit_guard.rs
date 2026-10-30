//! Exit cleanup for the sidecars, reachable from both ways Skein can end.
//!
//! A normal quit runs `RunEvent::Exit`. The Windows in-app updater does not:
//! it launches the installer and calls `process::exit(0)` from
//! `Update::install`, after only `AppHandle::cleanup_before_exit`, which
//! clears the app's resources table (#698). The plugin hard-wires that hook
//! on the JS `check()` path, so it cannot be replaced. Dropping a resource in
//! that table is the one thing it does run, hence [`SidecarExitGuard`].
//! `stop_all` drains the map, so running it again after a normal exit is a
//! no-op.

use tauri::Resource;

use super::OpencodeSidecarManager;

/// Stop every sidecar. Shared by `RunEvent::Exit` and [`SidecarExitGuard`].
pub fn exit_cleanup(sidecars: &OpencodeSidecarManager) {
    sidecars.stop_all();
}

/// Registered in the app's resources table (see `setup/state.rs`); its `Drop`
/// is the updater-exit path described in the module docs.
pub struct SidecarExitGuard(OpencodeSidecarManager);

impl SidecarExitGuard {
    pub fn new(sidecars: OpencodeSidecarManager) -> Self {
        Self(sidecars)
    }
}

impl Resource for SidecarExitGuard {}

impl Drop for SidecarExitGuard {
    fn drop(&mut self) {
        tracing::info!("sidecar exit guard dropped: stopping opencode sidecars");
        exit_cleanup(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_the_guard_over_an_empty_manager_is_harmless() {
        let manager = OpencodeSidecarManager::default();
        drop(SidecarExitGuard::new(manager.clone()));
        exit_cleanup(&manager);
    }
}
