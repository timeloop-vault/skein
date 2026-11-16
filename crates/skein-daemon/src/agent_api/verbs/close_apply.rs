//! What `close_room` and `close_harness` do in the daemon once their guards
//! pass (#734): the unsaved-buffer question, and the store mutation. Both used
//! to be a round trip to the webview.

use std::time::Duration;

use serde::Deserialize;
use skein_proto::room_mutations::RoomMutation;
use skein_proto::rooms::Room;

use super::shared::frontend_error;
use super::{VerbError, VerbResult};
use crate::agent_api::state::AgentApiState;
use crate::db::Harness;
use crate::room_store::RoomStore;

/// Origin the store stamps on a change an agent verb made.
const ORIGIN: &str = "daemon-agent";

/// The one question a close still asks a client: do any of these harnesses
/// hold buffers that were never saved? Only a client can know, and only a
/// `files` harness has buffers.
const FILES_DIRTY: &str = "files.dirty";
const FILES_DIRTY_TIMEOUT: Duration = Duration::from_secs(5);

/// The client's answer to `files.dirty`: only harnesses with dirty buffers.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilesDirtyAnswer {
    dirty: Vec<DirtyHarness>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DirtyHarness {
    #[allow(dead_code)] // part of the contract; the refusal names files, not harnesses
    harness_id: String,
    names: Vec<String>,
}

/// Refuse with `unsaved_files:` when a client reports dirty buffers in any of
/// `harnesses`.
///
/// Nothing is asked when none of them is a `files` harness, or when no client
/// is attached (nothing can be unsaved then). A client that is attached but
/// does not answer refuses the close: an unanswered question is never a yes.
pub(super) async fn refuse_unsaved_files(
    state: &AgentApiState,
    harnesses: &[Harness],
) -> VerbResult<()> {
    let ids: Vec<&str> = harnesses
        .iter()
        .filter(|h| h.kind == "files")
        .map(|h| h.id.as_str())
        .collect();
    if ids.is_empty() || !state.has_client() {
        return Ok(());
    }
    let reply = state
        .request_frontend(
            FILES_DIRTY,
            serde_json::json!({ "harnessIds": ids }),
            FILES_DIRTY_TIMEOUT,
        )
        .await
        .map_err(|e| frontend_error(&e))?;
    let answer: FilesDirtyAnswer = serde_json::from_value(reply).map_err(|e| {
        VerbError::Unavailable(format!(
            "the app returned an unexpected answer for files.dirty: {e}"
        ))
    })?;
    let names: Vec<String> = answer.dirty.into_iter().flat_map(|d| d.names).collect();
    if names.is_empty() {
        Ok(())
    } else {
        Err(VerbError::Refused(format!(
            "unsaved_files: {}",
            names.join(", ")
        )))
    }
}

/// The attached room store, or an `Unavailable` when the daemon has none.
pub(super) fn store_of(state: &AgentApiState) -> VerbResult<&std::sync::Arc<RoomStore>> {
    state.room_store().ok_or_else(|| {
        VerbError::Unavailable("the room store is not attached to the agent API".to_owned())
    })
}

/// The store and the held room-verb lock (`DaemonHandles::verb_lock`). Take
/// it after every slow await and hold it across store calls only: the guard
/// must be dropped before any other await.
pub(super) async fn lock_verbs(
    state: &AgentApiState,
) -> VerbResult<(tokio::sync::MutexGuard<'_, ()>, &std::sync::Arc<RoomStore>)> {
    let store = store_of(state)?;
    let lock = state.verb_lock().ok_or_else(|| {
        VerbError::Unavailable("the room store is not attached to the agent API".to_owned())
    })?;
    Ok((lock.lock().await, store))
}

/// Apply `mutations` as one batch; any entry the store skips is a refusal
/// (`not_found`), never a success. `what` names the thing for the message.
pub(super) fn apply_all(
    store: &RoomStore,
    mutations: Vec<RoomMutation>,
    what: &str,
) -> VerbResult<()> {
    let outcome = store.apply(ORIGIN, mutations).map_err(super::internal)?;
    if let Some(skipped) = outcome.skipped.first() {
        return Err(VerbError::NotFound(format!(
            "not_found: {what} could not be changed ({})",
            skipped.reason
        )));
    }
    Ok(())
}

/// The room as the store holds it right now.
pub(super) fn current_room(store: &RoomStore, room_id: &str) -> Option<Room> {
    store.rooms().into_iter().find(|r| r.id == room_id)
}
