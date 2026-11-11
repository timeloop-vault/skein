//! `show_changes` (#547): outline the rendered elements whose JSX opening
//! tag sits on a line a diff scope changed.
//!
//! Rust works out which new-side lines changed (the review's own scopes,
//! so it agrees with `get_diff`); the webview owns the page and does the
//! mapping. It maps only changed lines inside an element's opening tag;
//! everything else comes back as `unmapped` with a reason, and `limits`
//! says so in words. Never a claim about what visibly changed.
//!
//! Frontend kind `design.show_changes`, args
//! `{roomId, harnessId, scope, files: [{path, lines, deleted}],
//! truncated?: true, reveal?: true}`; answer `{highlighted, capped?,
//! mapped[], unmapped[], noSourceInfo?, truncated?, revealed?, limits}`.
//! A scope with no changed files never reaches the frontend.

use serde::Deserialize;
use serde_json::{Value, json};

use super::design::{
    SHOW_ELEMENT_TIMEOUT, ask, bad_arguments, caller_room, check_reveal, guard_write, pick_harness,
    with_harness_id,
};
use super::review::parse_scope;
use super::{VerbError, VerbResult, internal};
use crate::agent_api::auth::Caller;
use crate::agent_api::state::AgentApiState;
use crate::review_surface::Scope;
use crate::review_surface::changes::{
    ChangedFile, MAX_CHANGED_FILES, MAX_CHANGED_LINES, cap, scope_changes,
};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ShowChangesArgs {
    #[serde(default)]
    pub harness: Option<String>,
    /// `branch` (default), `pending` or `commit`, as for `get_diff`.
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub commit_sha: Option<String>,
    /// Boolean; kept as JSON (null included) so a non-boolean is
    /// `bad_arguments`.
    #[serde(default, deserialize_with = "present")]
    pub reveal: Option<Value>,
}

fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

fn scope_label(scope: Scope) -> &'static str {
    match scope {
        Scope::Branch => "branch",
        Scope::Commit => "commit",
        Scope::Pending => "pending",
    }
}

fn file_json(f: &ChangedFile) -> Value {
    json!({ "path": f.path, "lines": f.lines, "deleted": f.deleted })
}

/// Outline, in the design pane, the rendered elements a diff scope's
/// changed lines belong to. Transient; creates no thread.
pub async fn show_changes(
    state: &AgentApiState,
    caller: &Caller,
    args: &ShowChangesArgs,
    harness_control_enabled: bool,
) -> VerbResult<Value> {
    let scope = parse_scope(args.scope.as_deref(), args.commit_sha.as_deref())
        .map_err(|m| bad_arguments(&m))?;
    if scope != Scope::Commit && args.commit_sha.is_some() {
        return Err(bad_arguments(
            "`commit_sha` is only for `scope: \"commit\"`",
        ));
    }
    let reveal = check_reveal(args.reveal.as_ref())?;
    guard_write(state, caller, "show_changes", harness_control_enabled)?;
    let room = caller_room(state, caller)?;
    let harness = pick_harness(&room, args.harness.as_deref())?;
    let cwd = room
        .cwd
        .clone()
        .ok_or_else(|| VerbError::Unavailable("this room has no folder to diff".into()))?;

    let db = state.db.clone();
    let room_id = room.id.clone();
    let sha = args.commit_sha.clone();
    let files = tokio::task::spawn_blocking(move || {
        scope_changes(&db, &room_id, &cwd, scope, sha.as_deref())
    })
    .await
    .map_err(internal)?
    .map_err(VerbError::Unavailable)?;

    let mut tag = json!({ "scope": scope_label(scope), "harnessId": harness.id });
    if let (Some(obj), Some(sha)) = (tag.as_object_mut(), &args.commit_sha) {
        obj.insert("commit_sha".into(), json!(sha));
    }

    if files.is_empty() {
        let mut out = json!({
            "files": 0, "highlighted": 0, "mapped": [], "unmapped": [],
            "note": "no changes in this scope",
        });
        merge(&mut out, &tag);
        return Ok(out);
    }

    let (files, truncated) = cap(files, MAX_CHANGED_FILES, MAX_CHANGED_LINES);
    let mut request = json!({
        "roomId": room.id,
        "harnessId": harness.id,
        "scope": scope_label(scope),
        "files": files.iter().map(file_json).collect::<Vec<_>>(),
    });
    if let Some(obj) = request.as_object_mut() {
        if truncated {
            obj.insert("truncated".into(), json!(true));
        }
        if reveal {
            obj.insert("reveal".into(), json!(true));
        }
    }
    let answer = ask(
        state,
        "show_changes",
        caller,
        &harness.id,
        "design.show_changes",
        request,
        SHOW_ELEMENT_TIMEOUT,
    )
    .await?;
    let mut answer = with_harness_id(answer, &harness.id)?;
    merge(&mut answer, &tag);
    Ok(answer)
}

/// Copy `extra`'s keys into `target`, overwriting.
fn merge(target: &mut Value, extra: &Value) {
    if let (Some(t), Some(e)) = (target.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            t.insert(k.clone(), v.clone());
        }
    }
}
