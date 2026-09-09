//! The Tauri boundary.
//!
//! Nine commands, all the same shape: clone the `Database` handle, do
//! the work on the blocking pool, collapse the error to a `String`.
//! Every one of them stats or reads files, walks a revision list, or
//! touches sqlite, and the pane calls the first two on every debounced
//! watcher tick — exactly the work #171/#172 want off the main thread.
//!
//! The logic lives in the sibling modules; keeping it out of here is
//! what lets it be read and changed without a Tauri runtime in the way.

use std::sync::Arc;

use super::Scope;
use super::dto::{CommentDto, FileDetailDto, NewThread, ReviewScopeDto, ThreadDto};
use super::query::{file_impl, scope_impl};
use super::signoff::{self, SignoffStatus};
use super::write::add_thread_impl;
use crate::db::{Database, ReviewCommentRow};
use crate::review::now_ms;

/// The review header, commit list and file list for one scope.
#[tauri::command]
pub async fn review_scope(
    room_id: String,
    cwd: String,
    scope: Option<Scope>,
    commit_sha: Option<String>,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<ReviewScopeDto, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        scope_impl(
            &db,
            &room_id,
            &cwd,
            scope.unwrap_or_default(),
            commit_sha.as_deref(),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// One file's diff, plus every thread on it re-anchored to right now.
#[tauri::command]
pub async fn review_file(
    room_id: String,
    cwd: String,
    path: String,
    scope: Option<Scope>,
    commit_sha: Option<String>,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<FileDetailDto, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        file_impl(
            &db,
            &room_id,
            &cwd,
            &path,
            scope.unwrap_or_default(),
            commit_sha.as_deref(),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Open a thread with its first comment.
#[tauri::command]
pub async fn review_add_thread(
    room_id: String,
    cwd: String,
    thread: NewThread,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<ThreadDto, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || add_thread_impl(&db, &room_id, &cwd, &thread))
        .await
        .map_err(|e| e.to_string())?
}

/// Reply on an existing thread (D5: flat, with replies).
#[tauri::command]
pub async fn review_reply(
    room_id: String,
    thread_id: String,
    body: String,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<CommentDto, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        if body.trim().is_empty() {
            return Err("a comment needs a body".to_string());
        }
        if db.review_thread(&thread_id)?.is_none() {
            return Err("that thread no longer exists".to_string());
        }
        let now = now_ms();
        let row = ReviewCommentRow {
            id: uuid::Uuid::new_v4().to_string(),
            thread_id,
            room_id,
            author_kind: "user".to_string(),
            author_id: None,
            body,
            created_ms: now,
            updated_ms: now,
        };
        db.insert_review_comment(&row)?;
        Ok(CommentDto::from(row))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Edit a comment's text.
#[tauri::command]
pub async fn review_edit_comment(
    comment_id: String,
    body: String,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        if body.trim().is_empty() {
            return Err("a comment needs a body".to_string());
        }
        if db.update_review_comment(&comment_id, &body, now_ms())? {
            Ok(())
        } else {
            Err("that comment no longer exists".to_string())
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Delete a comment. Returns the thread id when that emptied the whole
/// thread, so the caller can drop it from the gutter.
#[tauri::command]
pub async fn review_delete_comment(
    comment_id: String,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<Option<String>, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || db.delete_review_comment(&comment_id))
        .await
        .map_err(|e| e.to_string())?
}

/// Delete a whole thread and every comment on it.
///
/// Separate from deleting the last comment one at a time: retracting a
/// review remark is one action in the user's head, and making them
/// delete four replies to undo one thread is the sort of friction that
/// leaves stale threads lying in the gutter instead.
#[tauri::command]
pub async fn review_delete_thread(
    thread_id: String,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || db.delete_review_thread(&thread_id).map(|_| ()))
        .await
        .map_err(|e| e.to_string())?
}

/// Resolve or reopen a thread.
///
/// Human-only, and deliberately so: D8 keeps resolve out of the agent's
/// verbs, because an agent that can resolve its own comments removes
/// the gate the loop exists to provide.
#[tauri::command]
pub async fn review_resolve_thread(
    thread_id: String,
    resolved: bool,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        let now = now_ms();
        let stamp = if resolved { Some(now) } else { None };
        if db.set_review_thread_resolved(&thread_id, stamp, now)? {
            Ok(())
        } else {
            Err("that thread no longer exists".to_string())
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Mark a file as looked at, or un-mark it.
#[tauri::command]
pub async fn review_mark_viewed(
    room_id: String,
    path: String,
    content_hash: String,
    viewed: bool,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        if viewed {
            db.set_review_viewed(&room_id, &path, &content_hash, now_ms())
        } else {
            db.clear_review_viewed(&room_id, &path)
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Point the room's review at a different base ref.
#[tauri::command]
pub async fn review_set_base(
    room_id: String,
    cwd: String,
    base_ref: String,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<ReviewScopeDto, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        db.set_review_base_ref(&room_id, &base_ref, now_ms())?;
        scope_impl(&db, &room_id, &cwd, Scope::Branch, None)
    })
    .await
    .map_err(|e| e.to_string())?
}

// ── the reviewer's sign-off (#214) ────────────────────────────────

/// Where the room's review stands: approved, stale, or neither.
#[tauri::command]
pub async fn review_signoff_status(
    room_id: String,
    cwd: String,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<SignoffStatus, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || signoff::status_impl(&db, &room_id, &cwd))
        .await
        .map_err(|e| e.to_string())?
}

/// Approve the room's work, or withdraw an approval.
///
/// Confirmed in the pane first. The approved sha is read here rather
/// than taken from the caller: the pane's HEAD is from its last
/// refresh, and an approval has to name the commit that exists when
/// the button is pressed.
#[tauri::command]
pub async fn review_set_signoff(
    room_id: String,
    cwd: String,
    approved: bool,
    note: Option<String>,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<SignoffStatus, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        signoff::set_impl(&db, &room_id, &cwd, approved, note.as_deref(), now_ms())
    })
    .await
    .map_err(|e| e.to_string())?
}
