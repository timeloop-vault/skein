//! The launch mode of a stored harness (#752): its tool, or the post-exit
//! shell the user chose with Enter. `shell_argv` reads the explicit field;
//! `infer_launch`/`migrate_launch` are the one-time migration that fills it
//! in for records written before it existed, using the old test (is
//! `cmd[0]` the kind's own program?) exactly as `resume_harness` does.

use skein_proto::harness_kind::HarnessKind;
use skein_proto::rooms::{Harness, LaunchMode, Room};

/// The shell argv the harness runs, only when its launch mode is `Shell`
/// (whatever `cmd` holds).
#[must_use]
pub fn shell_argv(h: &Harness) -> Option<&[String]> {
    match h.launch.as_ref()? {
        LaunchMode::Shell { argv } => Some(argv),
        LaunchMode::Tool => None,
    }
}

/// The launch mode an old record implies, or `None` to leave it alone.
/// `Shell` only for a harness with no explicit mode, of a resuming kind with
/// a managed program, whose stored `cmd[0]` is something else.
#[must_use]
pub fn infer_launch(h: &Harness) -> Option<LaunchMode> {
    if h.launch.is_some() {
        return None;
    }
    let kind = HarnessKind::from_name(&h.kind)?;
    if !kind.resumes() {
        return None;
    }
    let program = kind.program()?;
    let cmd = h.cmd.as_ref()?;
    let first = cmd.first().filter(|s| !s.is_empty())?;
    // Same exact comparison as `resume_harness`.
    if first.as_str() == program {
        return None;
    }
    Some(LaunchMode::Shell { argv: cmd.clone() })
}

/// The shell argv the harness is in, owned: the explicit `Shell` launch mode,
/// or (the legacy, un-migrated arm, kept as the migration fallback for a
/// record `migrate_launch` has not reached yet) the argv `infer_launch`
/// derives from `cmd[0]` not being the kind's own program.
#[must_use]
pub fn effective_shell_argv(h: &Harness) -> Option<Vec<String>> {
    if let Some(argv) = shell_argv(h) {
        return Some(argv.to_vec());
    }
    match infer_launch(h)? {
        LaunchMode::Shell { argv } => Some(argv),
        LaunchMode::Tool => None,
    }
}

/// Fill in `launch` on every harness that needs it. Touches nothing else;
/// returns whether anything changed. Idempotent.
pub fn migrate_launch(room: &mut Room) -> bool {
    let mut changed = false;
    for h in &mut room.harnesses {
        if let Some(mode) = infer_launch(h) {
            h.launch = Some(mode);
            changed = true;
        }
    }
    changed
}
