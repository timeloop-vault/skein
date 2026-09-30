//! Opening a thread.
//!
//! The only write path that has to think, because it is where an anchor
//! is captured. Everything else about a comment — replies, edits,
//! resolve — is a row change the database does on its own.
//!
//! The caller sends the lines it rendered rather than having them
//! re-read here. An anchor has to be the text the user was actually
//! looking at, and re-reading from disk could capture an agent edit
//! that landed between the click and the submit — the one moment where
//! an anchor could be wrong from birth.

use skein_review::{FileState, Placement, Side, capture_lines};

use super::anchoring::{comments_by_thread, side_of, to_thread_dto};
use super::dto::{NewThread, ThreadDto};
use super::element::{apply_element, element_rows_by_thread, validate_anchor};
use super::git::norm;
use super::thread_scope;
use crate::db::{Database, ReviewCommentRow, ReviewElementAnchorRow, ReviewThreadRow};
use crate::review::{abs_path, now_ms, relative_key};

pub(super) fn add_thread_impl(
    db: &Database,
    room_id: &str,
    cwd: &str,
    input: &NewThread,
) -> Result<ThreadDto, String> {
    if input.body.trim().is_empty() {
        return Err("a comment needs a body".into());
    }
    let scope = match input.scope.as_str() {
        thread_scope::LINE
        | thread_scope::FILE
        | thread_scope::COMMIT
        | thread_scope::REVIEW
        | thread_scope::ELEMENT => input.scope.clone(),
        other => return Err(format!("unknown comment scope: {other}")),
    };
    let is_element = scope == thread_scope::ELEMENT;
    if is_element != input.element.is_some() {
        return Err(if is_element {
            "an element comment needs the element it is about".into()
        } else {
            "only an element comment carries an element".into()
        });
    }
    let file_path = input
        .file_path
        .as_deref()
        .map(|p| relative_key(cwd, p).unwrap_or_else(|| norm(p)));
    if scope == thread_scope::LINE && file_path.is_none() {
        return Err("a line comment needs a file".into());
    }
    // Validated before anything is written, and what is stored is the
    // validated struct, not the caller's JSON.
    let element = match (&input.element, &file_path) {
        (Some(a), Some(path)) => Some(validate_anchor(a.clone(), path)?),
        (Some(_), None) => return Err("an element comment needs a file".into()),
        (None, _) => None,
    };

    // Fall back to reading the anchor from disk only when the caller
    // sent none — a line comment with no anchor text could never be
    // re-matched, and would be outdated from birth.
    let anchor_lines = if is_element {
        // An element has no text lines to be matched against.
        Vec::new()
    } else if input.anchor_lines.is_empty() && scope == thread_scope::LINE {
        match (&file_path, input.line_start, input.line_end) {
            (Some(path), Some(start), Some(end)) => {
                let text = match skein_review::read_state(&abs_path(cwd, path)) {
                    FileState::Text(s) => s,
                    _ => String::new(),
                };
                capture_lines(&text, start, end)
            }
            _ => Vec::new(),
        }
    } else {
        input.anchor_lines.clone()
    };

    let now = now_ms();
    let thread_id = uuid::Uuid::new_v4().to_string();
    let row = ReviewThreadRow {
        id: thread_id.clone(),
        room_id: room_id.to_owned(),
        scope: scope.clone(),
        file_path,
        commit_sha: if is_element {
            None
        } else {
            input.commit_sha.clone()
        },
        side: if scope == thread_scope::LINE {
            Some(match side_of(input.side.as_deref()) {
                Side::Old => "old".to_owned(),
                Side::New => "new".to_owned(),
            })
        } else {
            None
        },
        line_start: input
            .line_start
            .filter(|_| !is_element)
            .and_then(|n| i64::try_from(n).ok()),
        line_end: input
            .line_end
            .filter(|_| !is_element)
            .and_then(|n| i64::try_from(n).ok()),
        anchor_hash: if anchor_lines.is_empty() {
            None
        } else {
            Some(skein_review::hash_lines(&anchor_lines))
        },
        anchor_lines: if anchor_lines.is_empty() {
            None
        } else {
            serde_json::to_string(&anchor_lines).ok()
        },
        resolved_ms: None,
        created_ms: now,
        updated_ms: now,
    };
    match &element {
        Some(anchor) => db.insert_review_element_thread(
            &row,
            &ReviewElementAnchorRow {
                thread_id: thread_id.clone(),
                room_id: room_id.to_owned(),
                file_path: anchor.entry.clone(),
                anchor_json: serde_json::to_string(anchor).map_err(|e| e.to_string())?,
                last_seen_json: None,
                updated_ms: now,
            },
        )?,
        None => db.insert_review_thread(&row)?,
    }
    db.insert_review_comment(&ReviewCommentRow {
        id: uuid::Uuid::new_v4().to_string(),
        thread_id: thread_id.clone(),
        room_id: room_id.to_owned(),
        // v1 is human-only (D7). #213 is what starts writing `agent`.
        author_kind: "user".to_owned(),
        author_id: None,
        body: input.body.clone(),
        created_ms: now,
        updated_ms: now,
    })?;

    let comments = comments_by_thread(db, room_id)?;
    let stored = db
        .review_thread(&thread_id)?
        .ok_or("the thread vanished immediately after being written")?;
    // A brand-new thread is by definition where it was just placed.
    let placement = Placement::Unmoved {
        start: input.line_start.unwrap_or(1),
    };
    let mut dto = to_thread_dto(stored, anchor_lines, placement, &comments);
    if is_element {
        apply_element(
            std::slice::from_mut(&mut dto),
            &element_rows_by_thread(db, room_id)?,
            Some(cwd),
        );
    }
    Ok(dto)
}
