//! Where a review thread sits right now.
//!
//! `skein_review::anchor` owns the matching itself — pure, and tested
//! without a repo. This module is the part that has to know about the
//! world: which document a thread should be matched against given the
//! scope it is being viewed in, and what to do with the answer.
//!
//! Two things here are deliberate and easy to undo by accident.
//!
//! **The new position is written back.** A thread that kept searching
//! from where it was first written would, three rounds of agent edits
//! later, be measuring distance from somewhere the code has not been in
//! an hour, and the nearest-occurrence tie-break would decay into noise.
//! What is never rewritten is the anchor *text* — that is the comment's
//! evidence, and D6 needs an unplaceable thread to render against it.
//!
//! **A guess is reported as a guess.** `Placement::Shifted` sets
//! `outdated` just as `Outdated` does, because a comment silently
//! sitting on code it was not written about is the failure the whole
//! model exists to prevent.

use std::collections::HashMap;

use skein_git::Repo;
use skein_review::{Anchor, FileState, Placement, Reanchorer, Side};

use super::dto::{AddressedDto, CommentDto, ThreadDto};
use super::{Scope, thread_scope};
use crate::db::{Database, ReviewThreadRow};
use crate::review::abs_path;

fn placement_str(p: Placement) -> &'static str {
    match p {
        Placement::Unmoved { .. } => "unmoved",
        Placement::Moved { .. } => "moved",
        Placement::Shifted { .. } => "shifted",
        Placement::Outdated => "outdated",
    }
}

pub(super) fn parse_anchor_lines(raw: Option<&str>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .unwrap_or_default()
}

pub(super) fn side_of(raw: Option<&str>) -> Side {
    match raw {
        Some("old") => Side::Old,
        _ => Side::New,
    }
}

/// Everything a placement pass needs to know about the view it runs in.
pub(super) struct PlaceCtx<'a> {
    repo: Option<&'a Repo>,
    cwd: &'a str,
    /// The review range's merge base — the old side of `Scope::Branch`.
    base_sha: Option<&'a str>,
    scope: Scope,
    commit_sha: Option<&'a str>,
    /// First parent of `commit_sha`, resolved once per pass.
    parent_sha: Option<String>,
}

impl<'a> PlaceCtx<'a> {
    pub(super) fn new(
        repo: Option<&'a Repo>,
        cwd: &'a str,
        base_sha: Option<&'a str>,
        scope: Scope,
        commit_sha: Option<&'a str>,
    ) -> Self {
        let parent_sha = commit_sha.and_then(|sha| {
            repo.and_then(|r| r.commit_info(sha).ok().flatten())
                .and_then(|c| c.parents.first().cloned())
        });
        Self {
            repo,
            cwd,
            base_sha,
            scope,
            commit_sha,
            parent_sha,
        }
    }

    /// The text a thread on `path` should be re-anchored against.
    ///
    /// The two sides are genuinely different documents: a comment on a
    /// deleted line lives in the old text and would be permanently
    /// outdated if searched for in the new. Keeping the side is what
    /// makes such a thread anchorable at all.
    fn anchor_text(&self, side: Side, path: &str) -> String {
        match (side, self.scope) {
            (Side::New, Scope::Commit) => self.blob_text(self.commit_sha, path),
            (Side::Old, Scope::Commit) => self.blob_text(self.parent_sha.as_deref(), path),
            (Side::Old, _) => self.blob_text(self.base_sha, path),
            // The new side of every non-commit scope is what is on disk.
            (Side::New, _) => match skein_review::read_state(&abs_path(self.cwd, path)) {
                FileState::Text(s) => s,
                _ => String::new(),
            },
        }
    }

    fn blob_text(&self, rev: Option<&str>, path: &str) -> String {
        let Some(repo) = self.repo else {
            return String::new();
        };
        repo.blob_at(rev.unwrap_or_default(), path)
            .ok()
            .flatten()
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_default()
    }
}

/// Re-anchor every thread on one file and write the new coordinates
/// back.
///
/// The two sides are indexed at most once each, and only when a thread
/// actually asks for them: a file with twenty threads costs two index
/// builds, not twenty, and the common file with only new-side threads
/// never reads its old blob at all.
pub(super) fn place_threads(
    db: &Database,
    ctx: &PlaceCtx<'_>,
    path: &str,
    threads: Vec<ReviewThreadRow>,
    comments: &HashMap<String, Vec<CommentDto>>,
) -> Vec<ThreadDto> {
    let wants = |side: Side| {
        threads
            .iter()
            .any(|t| t.scope == thread_scope::LINE && side_of(t.side.as_deref()) == side)
    };
    let old_text = wants(Side::Old).then(|| ctx.anchor_text(Side::Old, path));
    let new_text = wants(Side::New).then(|| ctx.anchor_text(Side::New, path));
    let old_index = old_text.as_deref().map(Reanchorer::new);
    let new_index = new_text.as_deref().map(Reanchorer::new);

    let mut out = Vec::with_capacity(threads.len());
    for t in threads {
        let anchor_lines = parse_anchor_lines(t.anchor_lines.as_deref());
        let mut placement = Placement::Outdated;

        if t.scope == thread_scope::LINE && !anchor_lines.is_empty() {
            let side = side_of(t.side.as_deref());
            let index = match side {
                Side::Old => old_index.as_ref(),
                Side::New => new_index.as_ref(),
            };
            if let Some(index) = index {
                let start = usize::try_from(t.line_start.unwrap_or(1))
                    .unwrap_or(1)
                    .max(1);
                let anchor = Anchor::new(side, start, anchor_lines.clone());
                placement = index.place(&anchor);
                // Write the new position back so the next round searches
                // from where the thread was last seen.
                if let Some(new_start) = placement.start() {
                    if new_start != start {
                        let end = new_start + anchor_lines.len().saturating_sub(1);
                        if let Err(e) = db.update_review_thread_anchor(
                            &t.id,
                            i64::try_from(new_start).unwrap_or(i64::MAX),
                            i64::try_from(end).unwrap_or(i64::MAX),
                        ) {
                            tracing::warn!(thread = %t.id, error = %e, "review: re-anchor write failed");
                        }
                    }
                }
            }
        }

        out.push(to_thread_dto(t, anchor_lines, placement, comments));
    }
    out
}

pub(super) fn to_thread_dto(
    t: ReviewThreadRow,
    anchor_lines: Vec<String>,
    placement: Placement,
    comments: &HashMap<String, Vec<CommentDto>>,
) -> ThreadDto {
    // A thread that is not line-scoped has no anchor to go stale, so it
    // must never be reported as outdated — that flag means "this may no
    // longer be about what you think", which is meaningless for a
    // review-level remark.
    let line_scoped = t.scope == thread_scope::LINE;
    let (line_start, line_end, outdated, confidence) = if line_scoped {
        let start = placement.start();
        let end = start.map(|s| s + anchor_lines.len().saturating_sub(1));
        let confidence = match placement {
            Placement::Shifted { confidence, .. } => Some(confidence),
            _ => None,
        };
        (start, end, placement.is_outdated(), confidence)
    } else {
        (None, None, false, None)
    };

    ThreadDto {
        comments: comments.get(&t.id).cloned().unwrap_or_default(),
        placement: if line_scoped {
            placement_str(placement)
        } else {
            "unmoved"
        },
        id: t.id,
        scope: t.scope,
        file_path: t.file_path,
        commit_sha: t.commit_sha,
        side: t.side,
        line_start,
        line_end,
        anchor_lines,
        outdated,
        confidence,
        resolved_ms: t.resolved_ms,
        // Stamped afterwards by `apply_addressed` — see there for why
        // it is not this function's business.
        addressed: None,
        created_ms: t.created_ms,
        updated_ms: t.updated_ms,
    }
}

/// Every comment in the room, grouped by thread and already in DTO form.
pub(super) fn comments_by_thread(
    db: &Database,
    room_id: &str,
) -> Result<HashMap<String, Vec<CommentDto>>, String> {
    let labels = harness_labels(db, room_id)?;
    let mut map: HashMap<String, Vec<CommentDto>> = HashMap::new();
    for c in db.review_comments_for_room(room_id)? {
        let mut dto = CommentDto::from(c);
        if dto.author_kind == "agent" {
            dto.author_label = dto
                .author_id
                .as_ref()
                .and_then(|id| labels.get(id).cloned());
        }
        map.entry(dto.thread_id.clone()).or_default().push(dto);
    }
    Ok(map)
}

/// Harness id → the byline the pane prints for it (#213).
///
/// Resolved at read time rather than stored on the comment, so renaming
/// a harness renames its past replies too — the alternative is a row
/// full of ids nobody recognises once the harness is gone.
fn harness_labels(db: &Database, room_id: &str) -> Result<HashMap<String, String>, String> {
    Ok(db
        .room_by_id(room_id)?
        .map(|r| {
            r.harnesses
                .into_iter()
                .map(|h| (h.id, format!("{} · {}", h.kind, h.name)))
                .collect()
        })
        .unwrap_or_default())
}

/// Every standing "addressed" claim in the room, keyed by thread.
pub(super) fn addressed_by_thread(
    db: &Database,
    room_id: &str,
) -> Result<HashMap<String, AddressedDto>, String> {
    let labels = harness_labels(db, room_id)?;
    Ok(db
        .addressed_for_room(room_id)?
        .into_iter()
        .map(|a| {
            (
                a.thread_id,
                AddressedDto {
                    commit_sha: a.commit_sha,
                    by: labels
                        .get(&a.harness_id)
                        .cloned()
                        .unwrap_or_else(|| "agent".to_owned()),
                    note: a.note,
                    addressed_ms: a.addressed_ms,
                },
            )
        })
        .collect())
}

/// Stamp the claims onto threads that have one.
///
/// Applied after the fact rather than threaded through
/// [`to_thread_dto`]: whether a thread is addressed has nothing to do
/// with where it sits, and mixing the two would put a database read in
/// the middle of the anchoring path.
pub(super) fn apply_addressed(
    threads: &mut [ThreadDto],
    addressed: &HashMap<String, AddressedDto>,
) {
    for t in threads.iter_mut() {
        t.addressed = addressed.get(&t.id).cloned();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_thread_side_defaults_to_new_when_unset_or_unknown() {
        // An old row, or a file/review thread that never had a side.
        assert_eq!(side_of(Some("old")), Side::Old);
        assert_eq!(side_of(Some("new")), Side::New);
        assert_eq!(side_of(None), Side::New);
        assert_eq!(side_of(Some("nonsense")), Side::New);
    }

    #[test]
    fn anchor_lines_survive_a_json_round_trip_and_tolerate_junk() {
        let lines = vec!["let x = 1;".to_owned(), "  // a comment".to_owned()];
        let json = serde_json::to_string(&lines).unwrap();
        assert_eq!(parse_anchor_lines(Some(&json)), lines);
        // A row written by something else, or corrupted, must not panic
        // — it degrades to an unanchorable thread, which renders as
        // outdated rather than crashing the pane.
        assert!(parse_anchor_lines(Some("not json")).is_empty());
        assert!(parse_anchor_lines(None).is_empty());
    }

    #[test]
    fn a_non_line_thread_is_never_reported_as_outdated() {
        // "Outdated" means "this may no longer be about what you think",
        // which is meaningless for a review-level remark.
        let row = ReviewThreadRow {
            id: "t1".into(),
            room_id: "r1".into(),
            scope: thread_scope::REVIEW.into(),
            file_path: None,
            commit_sha: None,
            side: None,
            line_start: None,
            line_end: None,
            anchor_hash: None,
            anchor_lines: None,
            resolved_ms: None,
            created_ms: 1,
            updated_ms: 1,
        };
        let dto = to_thread_dto(row, Vec::new(), Placement::Outdated, &HashMap::new());
        assert!(!dto.outdated);
        assert_eq!(dto.placement, "unmoved");
        assert_eq!(dto.line_start, None);
    }

    #[test]
    fn a_shifted_line_thread_is_placed_and_flagged_at_once() {
        let row = ReviewThreadRow {
            id: "t1".into(),
            room_id: "r1".into(),
            scope: thread_scope::LINE.into(),
            file_path: Some("a.rs".into()),
            commit_sha: None,
            side: Some("new".into()),
            line_start: Some(10),
            line_end: Some(11),
            anchor_hash: None,
            anchor_lines: None,
            resolved_ms: None,
            created_ms: 1,
            updated_ms: 1,
        };
        let lines = vec!["one".to_owned(), "two".to_owned()];
        let dto = to_thread_dto(
            row,
            lines,
            Placement::Shifted {
                start: 20,
                confidence: 0.75,
            },
            &HashMap::new(),
        );
        assert_eq!((dto.line_start, dto.line_end), (Some(20), Some(21)));
        assert!(dto.outdated, "a guess renders as a guess");
        assert_eq!(dto.placement, "shifted");
        assert_eq!(dto.confidence, Some(0.75));
    }

    #[test]
    fn an_outdated_line_thread_keeps_its_evidence_but_loses_its_position() {
        let row = ReviewThreadRow {
            id: "t1".into(),
            room_id: "r1".into(),
            scope: thread_scope::LINE.into(),
            file_path: Some("a.rs".into()),
            commit_sha: None,
            side: Some("new".into()),
            line_start: Some(10),
            line_end: Some(10),
            anchor_hash: None,
            anchor_lines: None,
            resolved_ms: None,
            created_ms: 1,
            updated_ms: 1,
        };
        let lines = vec!["the code as it was".to_owned()];
        let dto = to_thread_dto(row, lines.clone(), Placement::Outdated, &HashMap::new());
        assert_eq!(dto.line_start, None, "it is not placed anywhere");
        assert_eq!(
            dto.anchor_lines, lines,
            "but it still carries what it was about"
        );
        assert!(dto.outdated);
    }
}
