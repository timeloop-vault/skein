//! Which edits are structural (#594, #729): a change to which rooms or
//! harnesses exist, their order, or their scope. Structural edits are
//! written to sqlite at once because other code reads the persisted rows;
//! everything else is coalesced by the debounce.

use skein_proto::rooms::{Harness, Room};

/// Room-level structural key: id, name, archived, retired, cwd, repoRoot,
/// branch, createdBy, closedBy. `harnesses` is deliberately not compared.
pub(super) fn room_key_eq(a: &Room, b: &Room) -> bool {
    a.id == b.id
        && a.name == b.name
        && a.archived == b.archived
        && a.retired == b.retired
        && a.cwd == b.cwd
        && a.repo_root == b.repo_root
        && a.branch == b.branch
        && a.created_by == b.created_by
        && a.closed_by == b.closed_by
}

/// Harness structural key: id, kind, name, agent, cwd, remote, sessionId,
/// launch (#752: the spawn reads it), createdBy.
pub(super) fn harness_key_eq(a: &Harness, b: &Harness) -> bool {
    a.id == b.id
        && a.kind == b.kind
        && a.name == b.name
        && a.agent == b.agent
        && a.cwd == b.cwd
        && a.remote == b.remote
        && a.session_id == b.session_id
        && a.launch == b.launch
        && a.created_by == b.created_by
}
