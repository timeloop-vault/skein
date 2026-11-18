//! `context_status` (#689): the calling harness's own context size next
//! to the limit the user configured.

use serde::Serialize;

use super::{VerbError, VerbResult, internal};
use crate::agent_api::auth::Caller;
use crate::db::Database;

/// What `context_status` reports. `snake_case` like `StatusOut`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct ContextStatusOut {
    pub harness_id: String,
    /// The latest turn's context tokens; `None` when the harness has no
    /// turn cost row yet or its kind reports no context size.
    pub context_tokens: Option<u64>,
    /// `SpawnSettings.context_threshold`; `None` when the user set none.
    pub limit: Option<u64>,
    /// True only when both values are known and tokens reach the limit.
    pub over_limit: bool,
}

/// The calling harness's own latest-turn context size and the user's
/// limit. Takes no arguments and never answers for another harness:
/// the harness id is an unverified header hint, so it is checked
/// against the harnesses of the caller's own room first, and a miss is
/// [`VerbError::NotFound`] — indistinguishable from an id that exists
/// elsewhere, so the verb cannot probe other rooms.
///
/// `limit` is read by the caller from the live spawn settings.
pub fn context_status(
    db: &Database,
    caller: &Caller,
    limit: Option<u64>,
) -> VerbResult<ContextStatusOut> {
    let Some(harness_id) = caller.harness_id.as_deref() else {
        return Err(VerbError::Refused(
            "context_status needs X-Skein-Harness to say which harness is asking".into(),
        ));
    };
    let room = db
        .room_by_id(&caller.room_id)
        .map_err(internal)?
        .ok_or_else(|| VerbError::NotFound("no such room".into()))?;
    if !room.harnesses.iter().any(|h| h.id == harness_id) {
        return Err(VerbError::NotFound("no such harness in this room".into()));
    }
    let context_tokens = crate::harness_log::context_sizes(db, &[harness_id.to_owned()])
        .map_err(internal)?
        .into_iter()
        .find(|s| s.harness_id == harness_id)
        .map(|s| s.tokens);
    let over_limit = matches!((context_tokens, limit), (Some(t), Some(l)) if t >= l);
    Ok(ContextStatusOut {
        harness_id: harness_id.to_owned(),
        context_tokens,
        limit,
        over_limit,
    })
}
