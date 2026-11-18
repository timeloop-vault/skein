//! Ends a harness PTY child's whole process tree, not just the child (#190).
//!
//! A harness spawns grandchildren (build tools, MCP servers, shells); killing
//! only the direct child leaks them. `skein_procprio::ProcessTree` is attached
//! right after the spawn and used by [`kill`].
//!
//! Coverage per OS:
//! - Windows: a job object with kill-on-close. Everything the child started
//!   after the attach is in the job, unless it explicitly asked to leave it
//!   (see Limits). The job closes when the `Pty` entry is dropped or the
//!   Skein process exits, which also ends the tree.
//! - Unix: SIGHUP to the child's process group, then a delayed SIGKILL. A
//!   descendant that left the process group or session is not reached.
//! - Other OSes: no tree; behaviour is the plain child kill.
//!
//! Limits:
//! - Windows window: it is attached after the spawn, because portable-pty
//!   exposes no creation flags and no way to spawn suspended. Anything the
//!   child starts between spawn and attach is outside the job. The attach is
//!   done before the priority change to keep that window small.
//! - Windows breakaway: the job allows breakaway, so a descendant started
//!   with `CREATE_BREAKAWAY_FROM_JOB` leaves the job and survives the kill.
//!   Refusing it would make such a tool fail to start with access denied.
//! - Unix: a leader that has exited is skipped, see [`kill`].
//! - The waiter sets `exited` up to 250 ms after it reaped the child, so a
//!   kill in that window still signals the group. That is accepted: a pid is
//!   not reused that fast.

use std::io::ErrorKind;

use portable_pty::ChildKiller;
use skein_procprio::ProcessTree;

/// Attach a tree to the child `pid`. Never fails the spawn: errors are logged
/// and `None` is returned, which makes [`kill`] fall back to the plain kill.
pub(super) fn attach(id: &str, pid: Option<u32>) -> Option<ProcessTree> {
    let Some(pid) = pid else {
        tracing::debug!(id = %id, "pty_spawn process tree skipped: no child pid");
        return None;
    };
    match ProcessTree::attach(pid) {
        Ok(tree) => {
            tracing::debug!(id = %id, pid, "pty_spawn attached process tree");
            Some(tree)
        }
        Err(e) if e.kind() == ErrorKind::Unsupported => {
            tracing::debug!(id = %id, pid, "pty_spawn process tree unsupported here");
            None
        }
        Err(e) => {
            tracing::warn!(id = %id, pid, error = %e, "pty_spawn could not attach process tree");
            None
        }
    }
}

/// End the child and, when a tree is attached, everything under it.
///
/// `exited` is whether the waiter has seen the child exit. On Windows the job
/// handle stays valid after that and holds any leftovers, so the tree is
/// killed regardless. On Unix the tree is killed only for a child that has
/// not exited: once the waiter reaped the leader its pid, and so the process
/// group id, can be reused, and signalling the group could hit a stranger.
/// Leftovers of a harness that exited on its own are deliberately not chased
/// there.
///
/// The plain `killer` runs whenever the tree kill did not happen or failed.
pub(super) fn kill(
    id: &str,
    tree: Option<&ProcessTree>,
    killer: &mut (dyn ChildKiller + Send + Sync),
    exited: bool,
) {
    if let Some(tree) = tree
        && (cfg!(windows) || !exited)
    {
        match tree.kill() {
            Ok(()) => return,
            Err(e) => {
                tracing::warn!(id = %id, error = %e, "pty kill: process tree kill failed");
            }
        }
    }
    let _ = killer.kill();
}
