//! Element-thread handling for the agent verbs (#434), split out of
//! `verbs.rs` (#455). An element thread has no line, so the agent sees the
//! design pane's last report instead of a diff placement.

use std::collections::BTreeMap;
use std::path::Path;

use crate::db::ReviewThreadRow;
use crate::review_surface::element::{DigestCache, ElementDto, element_dto};
use crate::review_surface::thread_scope;

use super::auth::Caller;
use super::element_source::{SourceGuess, SourceIndex};
use super::verbs::VerbError;

/// The `placement` an element thread reports in place of line numbers.
pub(super) const PLACEMENT: &str = "element";
/// The `placement` when its source could be guessed.
pub(super) const PLACEMENT_GUESS: &str = "source_guess";

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
    pub placement: &'static str,
    pub source: Option<SourceGuess>,
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

/// A guess at the source of each element thread in `threads`, by thread
/// id. One [`SourceIndex`] serves the whole call, so the tree is walked
/// and each file read at most once however many threads there are.
pub(super) fn locate_all(
    caller: &Caller,
    threads: &[ReviewThreadRow],
    elements: &BTreeMap<String, ElementDto>,
) -> BTreeMap<String, SourceGuess> {
    let mut sources = BTreeMap::new();
    let Some(root) = caller.cwd.as_deref().map(Path::new) else {
        return sources;
    };
    let mut index = SourceIndex::new(root);
    for t in threads.iter().filter(|t| is_element(t)) {
        if let Some(guess) = elements.get(&t.id).and_then(|e| index.locate(&e.anchor)) {
            sources.insert(t.id.clone(), guess);
        }
    }
    sources
}

/// `None` for a non-element thread. Only an `anchored` report counts as
/// current for the agent; `reanchored` is still a guess to it.
pub(super) fn view(
    elements: &BTreeMap<String, ElementDto>,
    sources: &BTreeMap<String, SourceGuess>,
    t: &ReviewThreadRow,
) -> Option<View> {
    if !is_element(t) {
        return None;
    }
    let dto = elements.get(&t.id);
    let source = sources.get(&t.id).cloned();
    Some(View {
        outdated: dto.is_none_or(|e| e.state != "anchored"),
        json: dto.and_then(agent_json),
        placement: if source.is_some() {
            PLACEMENT_GUESS
        } else {
            PLACEMENT
        },
        source,
    })
}

/// The element as the agent sees it. When the stamp no longer holds
/// (`unknown`) the old selector and rect describe a page that is gone, so
/// they are dropped; state, stamp and time stay.
fn agent_json(dto: &ElementDto) -> Option<serde_json::Value> {
    let mut dto = dto.clone();
    if dto.state == "unknown"
        && let Some(seen) = dto.last_seen.as_mut()
    {
        seen.selector = None;
        seen.rect = None;
    }
    serde_json::to_value(&dto).ok()
}
