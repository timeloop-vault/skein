//! Element-thread handling for the agent verbs (#434), split out of
//! `verbs.rs` (#455). An element thread has no line, so the agent sees the
//! design pane's last report instead of a diff placement.

use std::collections::BTreeMap;
use std::path::Path;

use crate::db::ReviewThreadRow;
use crate::review_surface::element::{DigestCache, ElementDto, element_dto};
use crate::review_surface::thread_scope;

use super::auth::Caller;
use super::verbs::VerbError;

/// The `placement` an element thread reports in place of line numbers.
pub(super) const PLACEMENT: &str = "element";

pub(super) fn is_element(t: &ReviewThreadRow) -> bool {
    t.scope == thread_scope::ELEMENT
}

/// The threads `RoomCtx::load` should hold element anchors for: just
/// this one if it is an element thread. The empty slice for any other
/// skips the anchoring pass `get_comment` does itself.
pub(super) fn seed(t: &ReviewThreadRow) -> &[ReviewThreadRow] {
    if is_element(t) {
        std::slice::from_ref(t)
    } else {
        &[]
    }
}

/// How one thread presents to the agent, when it is an element thread.
pub(super) struct View {
    /// Never a line position, and a guess unless the pane's report holds
    /// — also when the anchor could not be read.
    pub outdated: bool,
    pub json: Option<serde_json::Value>,
}

/// Element threads' anchor and state for the room, if `threads` holds any.
/// The agent has no DOM, so this is the pane's last report, believed only
/// while its content stamp still holds — never a line position.
pub(super) fn load(
    db: &crate::db::Database,
    caller: &Caller,
    threads: &[ReviewThreadRow],
) -> Result<BTreeMap<String, ElementDto>, VerbError> {
    let mut elements = BTreeMap::new();
    if threads.iter().any(|t| t.scope == thread_scope::ELEMENT) {
        let rows = db
            .review_element_anchors_for_room(&caller.room_id)
            .map_err(super::verbs::internal)?;
        let root = caller.cwd.as_deref().map(Path::new);
        let mut cache = DigestCache::default();
        for row in rows {
            if let Some(dto) = element_dto(&row, root, &mut cache) {
                elements.insert(row.thread_id, dto);
            }
        }
    }
    Ok(elements)
}

/// `None` for a non-element thread.
pub(super) fn view(elements: &BTreeMap<String, ElementDto>, t: &ReviewThreadRow) -> Option<View> {
    if !is_element(t) {
        return None;
    }
    let dto = elements.get(&t.id);
    Some(View {
        outdated: dto.is_none_or(ElementDto::is_outdated),
        json: dto.and_then(|e| serde_json::to_value(e).ok()),
    })
}
