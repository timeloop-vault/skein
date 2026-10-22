//! Lowers a freshly spawned harness child's CPU priority (#596).
//!
//! Builds started by a harness (cargo -> rustc -> linker) inherit its
//! priority class, so a lowered harness stops those builds starving
//! Skein's own `WebView2` and the rest of the desktop.
//!
//! It is done after the spawn because portable-pty exposes no creation
//! flags. That leaves a race: anything the child itself starts before
//! this call keeps normal priority. In practice that window is tiny, as
//! a shell or agent CLI takes far longer to start up than this call
//! takes.

use crate::spawn_settings::ChildPriority;

/// Apply `priority` to the child `pid`. Never fails the spawn: errors are
/// logged and swallowed.
pub(super) fn apply(id: &str, pid: Option<u32>, priority: ChildPriority) {
    if priority == ChildPriority::Normal {
        return;
    }
    let Some(pid) = pid else {
        tracing::debug!(id = %id, "pty_spawn priority skipped: no child pid");
        return;
    };
    match skein_procprio::lower_priority(pid) {
        Ok(outcome) => {
            tracing::info!(id = %id, pid, outcome = ?outcome, "pty_spawn lowered child priority");
        }
        Err(e) => {
            tracing::warn!(id = %id, pid, error = %e, "pty_spawn could not lower child priority");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_priority_and_a_missing_pid_are_no_ops() {
        apply("t", Some(u32::MAX), ChildPriority::Normal);
        apply("t", None, ChildPriority::Lowered);
    }

    #[cfg(windows)]
    mod windows {
        use std::path::Path;

        use crate::harness_kind::HarnessKind;
        use crate::pty::{PtyManager, SpawnRequest};
        use crate::spawn_settings::SpawnSettings;

        fn spawn_sleeper(manager: &PtyManager, id: &str, settings: &SpawnSettings) -> u32 {
            let cmd: Vec<String> = ["ping", "-n", "30", "127.0.0.1"]
                .map(str::to_owned)
                .to_vec();
            manager
                .spawn(
                    SpawnRequest {
                        id: id.to_owned(),
                        cmd: &cmd,
                        cwd: Path::new("C:\\"),
                        rows: 24,
                        cols: 80,
                        settings,
                        env_profile: None,
                        agent: None,
                        kind: HarnessKind::Byoh,
                        harness_config: None,
                    },
                    |_| {},
                )
                .expect("spawn");
            manager.pid(id).expect("child pid")
        }

        #[test]
        fn the_setting_decides_whether_a_real_child_is_lowered() {
            let manager = PtyManager::new();

            let on = SpawnSettings::default();
            let lowered = spawn_sleeper(&manager, "prio-on", &on);
            let off = SpawnSettings {
                lower_harness_priority: false,
                ..SpawnSettings::default()
            };
            let normal = spawn_sleeper(&manager, "prio-off", &off);

            let lowered_state = skein_procprio::is_lowered(lowered);
            let normal_state = skein_procprio::is_lowered(normal);
            manager.kill("prio-on");
            manager.kill("prio-off");

            assert!(lowered_state.expect("query lowered child"));
            let normal_state = normal_state.expect("query normal child");
            // A runner that is itself low priority hands its children the same class.
            if !skein_procprio::is_lowered(std::process::id()).expect("query self") {
                assert!(!normal_state);
            }
        }
    }
}
