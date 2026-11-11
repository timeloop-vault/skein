//! What the review verbs share: thread lookup scoped to the caller's room,
//! and the per-room reads a comment listing needs, done once.

use std::collections::{BTreeMap, BTreeSet};

use super::review::{AgentAddressed, AgentComment, AgentThread};
use super::{VerbError, VerbResult, internal};
use crate::agent_api::auth::Caller;
use crate::agent_api::element;
use crate::agent_api::element_source::SourceGuess;
use crate::db::{Database, ReviewAddressedRow, ReviewCommentRow, ReviewThreadRow};
use crate::review_surface::Scope;
use crate::review_surface::element::ElementDto;
use crate::review_surface::query::ScopeFiles;

/// A thread, but only if it belongs to the caller's room.
///
/// "Wrong room" and "no such thread" return the same error on purpose:
/// distinguishing them would turn a token into an oracle for other
/// rooms' thread ids.
pub(super) fn thread_in_room(
    db: &Database,
    caller: &Caller,
    thread_id: &str,
) -> VerbResult<ReviewThreadRow> {
    match db.review_thread(thread_id).map_err(internal)? {
        Some(t) if t.room_id == caller.room_id => Ok(t),
        _ => Err(VerbError::NotFound(format!(
            "no comment thread {thread_id} in this room"
        ))),
    }
}

pub(in crate::agent_api) fn scope_name(scope: Scope) -> &'static str {
    match scope {
        Scope::Branch => "branch",
        Scope::Commit => "commit",
        Scope::Pending => "pending",
    }
}

/// The per-room reads a listing needs, done once.
///
/// The anchoring pass is the expensive part and the reason this exists:
/// every file that carries threads is re-anchored through one
/// `ScopeFiles` snapshot (#171 slice (f)) rather than once per thread —
/// and, since that slice, rather than once per file either: the whole
/// batch costs one scope diff, not one per commented file. Reading the
/// stored coordinates instead would be free and sometimes wrong, which
/// is the one thing D6 does not allow.
pub(super) struct RoomCtx {
    comments: BTreeMap<String, Vec<ReviewCommentRow>>,
    addressed: BTreeMap<String, ReviewAddressedRow>,
    /// harness id → "claude · main", for bylines.
    labels: BTreeMap<String, String>,
    /// thread id → (start, end, outdated) as of right now.
    pub(super) placed: BTreeMap<String, (Option<usize>, Option<usize>, bool)>,
    /// Element threads' anchor and state (#434).
    elements: BTreeMap<String, ElementDto>,
    /// Element threads' proposals (#436), as stored.
    proposals: BTreeMap<String, serde_json::Value>,
    /// Element threads' guessed source, by thread id (#435).
    sources: BTreeMap<String, SourceGuess>,
}

impl RoomCtx {
    pub(super) fn load(
        db: &Database,
        caller: &Caller,
        threads: &[ReviewThreadRow],
    ) -> VerbResult<Self> {
        let mut comments: BTreeMap<String, Vec<ReviewCommentRow>> = BTreeMap::new();
        for c in db
            .review_comments_for_room(&caller.room_id)
            .map_err(internal)?
        {
            comments.entry(c.thread_id.clone()).or_default().push(c);
        }
        let addressed = db
            .addressed_for_room(&caller.room_id)
            .map_err(internal)?
            .into_iter()
            .map(|a| (a.thread_id.clone(), a))
            .collect();
        let labels = db
            .room_by_id(&caller.room_id)
            .map_err(internal)?
            .map(|r| {
                r.harnesses
                    .into_iter()
                    .map(|h| (h.id, format!("{} · {}", h.kind, h.name)))
                    .collect()
            })
            .unwrap_or_default();

        let mut placed = BTreeMap::new();
        if let Some(cwd) = caller.cwd.as_deref() {
            let paths: Vec<String> = files_of(threads).into_iter().collect();
            // A snapshot that cannot be built at all (not a repo, and so
            // on) is not a failure of the listing — every thread just
            // falls back to its stored coordinates below, the same as
            // when each file's own `file_impl` would have failed.
            if let Ok(snapshot) =
                ScopeFiles::load(db, &caller.room_id, cwd, Scope::Branch, None, &paths)
            {
                for path in &paths {
                    // A single file we cannot diff (deleted, unreadable)
                    // must not sink the rest of the batch — its threads
                    // fall back the same way.
                    let Ok(detail) = snapshot.file(db, path) else {
                        continue;
                    };
                    for t in detail.threads {
                        placed.insert(t.id, (t.line_start, t.line_end, t.outdated));
                    }
                }
            }
        }

        let elements = element::load(db, caller, threads)?;
        let proposals = element::load_proposals(db, caller, threads)?;
        let sources = element::locate_all(caller, threads, &elements);

        Ok(Self {
            comments,
            addressed,
            labels,
            placed,
            elements,
            proposals,
            sources,
        })
    }

    pub(super) fn to_agent_thread(&self, t: &ReviewThreadRow) -> AgentThread {
        let view = element::view(&self.elements, &self.sources, t);
        let (line_start, line_end, outdated) = match &view {
            Some(v) => (None, None, v.outdated),
            None => self.placed.get(&t.id).copied().unwrap_or_else(|| {
                (
                    t.line_start.and_then(|n| usize::try_from(n).ok()),
                    t.line_end.and_then(|n| usize::try_from(n).ok()),
                    false,
                )
            }),
        };
        AgentThread {
            thread_id: t.id.clone(),
            scope: t.scope.clone(),
            file: t.file_path.clone(),
            placement: view.as_ref().map(|v| v.placement),
            source: view.as_ref().and_then(|v| v.source.clone()),
            element: view.and_then(|v| v.json),
            proposal: self.proposals.get(&t.id).cloned(),
            commit_sha: t.commit_sha.clone(),
            line_start,
            line_end,
            outdated,
            resolved: t.resolved_ms.is_some(),
            addressed: self.addressed.get(&t.id).map(|a| AgentAddressed {
                commit_sha: a.commit_sha.clone(),
                by: self
                    .labels
                    .get(&a.harness_id)
                    .cloned()
                    .unwrap_or_else(|| "agent".to_owned()),
                note: a.note.clone(),
                addressed_ms: a.addressed_ms,
            }),
            comments: self
                .comments
                .get(&t.id)
                .map(|cs| {
                    cs.iter()
                        .map(|c| AgentComment {
                            author: self.author_of(c),
                            body: c.body.clone(),
                            created_ms: c.created_ms,
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// "reviewer" for the human, the harness label for an agent. The
    /// agent reading this needs to know which lines are its own.
    fn author_of(&self, c: &ReviewCommentRow) -> String {
        if c.author_kind != "agent" {
            return "reviewer".to_owned();
        }
        c.author_id
            .as_ref()
            .and_then(|id| self.labels.get(id).cloned())
            .unwrap_or_else(|| "agent".to_owned())
    }
}

/// The distinct files a set of threads touches, normalized.
fn files_of(threads: &[ReviewThreadRow]) -> BTreeSet<String> {
    threads
        .iter()
        .filter(|t| !element::is_element(t))
        .filter_map(|t| t.file_path.as_deref().map(normalize))
        .collect()
}

pub(super) fn normalize(path: &str) -> String {
    path.replace('\\', "/")
}

pub(super) fn parse_anchor(raw: Option<&str>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .unwrap_or_default()
}
