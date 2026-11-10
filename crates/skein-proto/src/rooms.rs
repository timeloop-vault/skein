//! Room and harness records: the persisted/wire shape of a room.

use serde::{Deserialize, Serialize};

/// Mirrors the TS Harness interface. Field renames keep the wire format
/// camelCase to match what the frontend serializes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Harness {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub status: String,
    pub model: String,
    pub tokens: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub live: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cmd: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cwd: Option<String>,
    /// Conversation id from the underlying tool. See chapter-5-plan.md
    /// for how it gets populated; Skein only round-trips it.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub session_id: Option<String>,
    /// Which agent this harness was spawned with (`--agent <name>`),
    /// or `None` for "let the tool's own default decide" — a real
    /// choice, and distinct from any named agent (#247).
    ///
    /// Round-tripped, not interpreted: the frontend owns argv. It has
    /// to exist here all the same, because this struct *is* the
    /// persisted shape — a field the blob carries but the struct does
    /// not is dropped on the next save.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent: Option<String>,
    /// A `design` harness's chosen preview entry (#433): a worktree-
    /// relative, `/`-separated path. Round-tripped, not interpreted.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub design_entry: Option<String>,
    /// A `design` harness's device setting (#528): preset/custom size,
    /// landscape, DPR override. Round-tripped, not interpreted, opaque
    /// JSON on purpose, since the frontend owns the shape
    /// (`app/src/designDevice.ts`) and #529/#530 can add flags without a
    /// Rust change.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub design_device: Option<serde_json::Value>,
    /// Count of attention-worthy transitions accumulated for this
    /// harness while the user wasn't viewing it. Cleared when the
    /// harness becomes the active harness in the active room.
    /// Persisted so the badge survives Skein restarts. Epic #50 L5a.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pending_notifications: Option<i64>,
    /// Who opened this harness through the agent API's `open_harness`
    /// verb (#411) — `None` for every harness added by hand through
    /// "+ harness". Round-tripped only, the same as `Room.createdBy`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub created_by: Option<HarnessCreatedBy>,
    /// The session a post-exit shell's re-bound CLI is running (#520).
    /// `None` = no live claim. Round-tripped only: the frontend decides
    /// at resume whether the claim still holds.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shell_claim: Option<ShellClaim>,
    /// #568: where a `remote` harness runs.
    /// Round-tripped only; the frontend builds the ssh argv from it.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub remote: Option<RemoteSpec>,
    /// #615: per-harness env profile override; `None` = follow the global active profile.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub env_profile: Option<String>,
    /// #627: the Hermes profile a `hermes` harness runs in (`-p <name>`);
    /// `None` = no flag (root/active profile). Round-tripped only.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub hermes_profile: Option<String>,
    /// #675: the opencode version seen at the last spawn. `None` =
    /// unknown / never probed, which means v1. Round-tripped only.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub opencode_version: Option<OpencodeVersion>,
}

/// The opencode CLI version a harness last spawned under (#675, epic #674).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OpencodeVersion {
    pub major: u32,
    pub version: String,
}

/// A `remote` harness's target (#568): ssh host, the tool run
/// there, and the tmux session name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSpec {
    pub host: String,
    pub tool: String,
    pub session: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub dir: Option<String>,
}

/// A CLI the user re-launched from a harness's post-exit shell and
/// that Skein re-bound to the pane (#520). Kind-agnostic; `port` is
/// reserved for opencode's embedded server (#517).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShellClaim {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub port: Option<u16>,
}

/// Who asked for a harness `open_harness` (#411) added, so the UI can
/// say so. `harness_id` is `Option`, unlike [`CreatedBy::harness_id`]
/// on a room: a room's own `create_room` call always has a caller
/// harness attribution slot the frontend contract requires as a
/// string (round-tripped as `""` when absent, see `verbs::create_room`),
/// while this is a fresh field with no such wire constraint to match.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HarnessCreatedBy {
    pub room_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub harness_id: Option<String>,
}

/// Mirrors the TS Room interface.
///
/// Field policy (#167): every field added after v0.2.5 MUST carry
/// `#[serde(default)]` (or live inside `Option`). A required field
/// makes every previously-persisted blob unparseable, and an
/// unparseable blob gets quarantined out of the live table on the
/// next boot. Existing required fields stay required — a room
/// missing `name` or `id` is corrupt, not old.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Room {
    pub id: String,
    pub name: String,
    pub task: String,
    pub status: String,
    pub badge: i64,
    pub harnesses: Vec<Harness>,
    pub active_harness_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cwd: Option<String>,
    /// `None` for non-git rooms (chapter 6 phase 3). Present together
    /// with `branch` when the room was created from a git repo.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repo: Option<String>,
    /// Close timestamp (epoch ms). `None` = active; `Some` = archived
    /// (chapter 6 phase 2). Skein round-trips this; the frontend reads
    /// it for tab-strip filtering and the reopen modal.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub archived: Option<i64>,
    /// Canonical main-checkout path of the git repo this room belongs
    /// to (resolved from a worktree via skein-git `main_repo_root()`).
    /// `None` for non-git rooms and rooms not yet resolved. This is
    /// the room-group key (#76), and is kept even if the folder later
    /// disappears (#164).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repo_root: Option<String>,
    /// #328: set on a room created in the background
    /// (`createRoom(args, { activate: false })`), cleared the moment it
    /// becomes the active room — an unread mark on the room's own
    /// existence, not on any harness inside it.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub attention: Option<bool>,
    /// Who asked an agent to make this room (#330's `create_room` verb)
    /// — `None` for every room made through the New Room dialog.
    /// Round-tripped only: nothing on the Rust side reads it back, the
    /// same as `Harness.agent` round-trips argv it never interprets.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub created_by: Option<CreatedBy>,
    /// Who archived this room through the agent API's `close_room` verb
    /// (#411) — `None` for a room the user closed by hand, and cleared
    /// on unarchive (the same "not evidence of anything once undone" call
    /// `review_signoff` makes about its own record). Round-tripped only.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub closed_by: Option<ClosedBy>,
    /// Retirement timestamp (epoch ms, #417) — set by the frontend on an
    /// archived room that is history. A retired room owns no path:
    /// open-from-outside skips it. Round-tripped; must survive save.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub retired: Option<i64>,
    /// The repository this room was made in (#418): root commit ids, which
    /// survive clones and worktrees. Lets open-from-outside tell "same
    /// repo" from "a different repo now sits at this path".
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repo_identity: Option<RepoIdentity>,
    /// #335: the room's manual todo list. Opaque JSON, like
    /// `design_device`: the model lives in `app/src/todos/model.ts`, so
    /// unknown keys survive a round trip and Rust never needs a schema.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub todos: Option<Vec<serde_json::Value>>,
}

/// What stays the same about a repository across clones and worktrees
/// (#418). `origin_url` is display-only; matching uses `root_commits`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RepoIdentity {
    #[serde(default)]
    pub root_commits: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_url: Option<String>,
}

/// Who asked for a room `create_room` (#330) made, so the UI can say so.
/// `harness_id` is the specific harness that called `create_room`, not
/// merely the room — attribution the same way `X-Skein-Harness` is
/// everywhere else in the agent API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreatedBy {
    pub room_id: String,
    pub harness_id: String,
    /// First non-empty line of the `create_room` prompt, trimmed and
    /// capped (#356) — a title a director can show for a room it opened
    /// without re-reading the mail that made it. `None` for a room made
    /// before this field existed, and for the rare `create_room` call
    /// this couldn't be derived from.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_first_line: Option<String>,
    /// The commit the new worktree was cut from (#356) — known only to
    /// whoever actually created the worktree, so this is filled in
    /// after the fact rather than at the point `CreatedBy` is first
    /// built. `None` for a room made before this field existed, a
    /// `branchMode: "current"` room (no new worktree), or one Skein
    /// could not resolve a base commit for.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub base_sha: Option<String>,
}

/// Who closed a room through the agent API's `close_room` verb (#411).
/// `harness_id` is `Option`, unlike [`CreatedBy::harness_id`] — see
/// [`HarnessCreatedBy`]'s doc for why the two attribution shapes differ.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClosedBy {
    pub room_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub harness_id: Option<String>,
    /// Epoch milliseconds, mirroring `Room.archived`.
    pub at: i64,
}

/// A `sessions` row whose JSON blob failed to parse at load time.
/// The blob itself is preserved in `sessions_quarantine`; only the
/// id + parse error travel to the frontend (issue #167).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedRoom {
    pub id: String,
    pub error: String,
}
