//! The Files overlay's filesystem commands (#7, #49, #185, #409). The
//! logic, room scoping and DTOs live in `fs`; this file only resolves the
//! database, moves the call onto the blocking pool and wraps the result
//! for the wire.

use std::sync::Arc;

use crate::db::Database;
use crate::fs::{
    DirEntryDto, FileTextDto, list_dir_impl, read_file_text_impl, read_image_bytes_impl,
    write_file_text_impl,
};

/// One-level directory listing. Hidden entries (`.foo`) are included —
/// the frontend filters by default but can opt-in. Common build /
/// dependency dirs are *not* skipped here either; that's a UX call,
/// not a filesystem call.
///
/// Async: `read_dir` + a `metadata()`/`symlink_metadata()` call per
/// entry is a filesystem walk, and #171 wants that off the main thread
/// so it can't queue behind an agent's own file churn.
#[tauri::command]
pub async fn list_dir(
    path: String,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<DirEntryDto>, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || list_dir_impl(&path, &db))
        .await
        .map_err(|e| e.to_string())?
}

/// Read up to `TEXT_MAX_BYTES` of `path` as text. Returns
/// `Err("binary")` when the leading sniff window contains a NUL byte
/// (cheap heuristic — robust enough for "is this a JPEG or a Rust
/// file" and matches `git diff`'s behaviour).
///
/// Async: reading up to `TEXT_MAX_BYTES` (256 KiB) off disk is real
/// blocking work — #171.
#[tauri::command]
pub async fn read_file_text(
    path: String,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<FileTextDto, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || read_file_text_impl(&path, &db))
        .await
        .map_err(|e| e.to_string())?
}

/// Read `path`'s raw bytes for the Files editor / review pane image
/// preview (#409). Returns a [`tauri::ipc::Response`] — a raw
/// `ArrayBuffer` on the JS side, not JSON — because #46 found base64
/// breaks down on large images; this is the reason the command exists
/// at all rather than reusing `read_file_text`.
///
/// Same room scope as the other commands. Refuses a path that is not
/// one of `skein_review::is_image_path`'s known extensions
/// (`Err("notimage")`) and a file over
/// [`skein_review::MAX_IMAGE_BYTES`] (`Err("toolarge:<len>")`), checked
/// from metadata before any bytes are read.
///
/// Async: reading up to 16 MiB off disk is real blocking work — #171.
#[tauri::command]
pub async fn read_image_bytes(
    path: String,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<tauri::ipc::Response, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        read_image_bytes_impl(&path, &db, skein_review::MAX_IMAGE_BYTES)
    })
    .await
    .map_err(|e| e.to_string())?
    .map(tauri::ipc::Response::new)
}

/// Write `content` to `path` — the Files editor's save (#185). Same
/// room scope as the reads. The write goes through a sibling temp
/// file (unique per call) + rename so a crash mid-write can't leave
/// a truncated file and concurrent saves can't eat each other's
/// temp. The target must already exist (the editor only saves files
/// it opened); creating new files is a later stage.
///
/// `expected_mtime_ms` is the staleness token from `read_file_text`
/// (or a previous save): if the file on disk was modified since, the
/// write is refused — in a shared worktree an agent may have
/// rewritten the file under the buffer, and silently reverting its
/// work is the one thing a save must never do. Returns the new mtime
/// for the frontend to store.
///
/// Async: the metadata check, the write-then-rename and the
/// permissions carry-over are all blocking filesystem calls — #171.
/// `WRITE_LOCK` serializes the whole check-through-rename (acquired
/// inside `write_file_text_impl`, held for its full body): the
/// metadata check and the rename that settles it are not atomic with
/// each other, so async scheduling could otherwise let two overlapping
/// saves of the same path both pass the staleness guard before either
/// has written.
#[tauri::command]
pub async fn write_file_text(
    path: String,
    content: String,
    expected_mtime_ms: i64,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<i64, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        write_file_text_impl(&path, &content, expected_mtime_ms, &db)
    })
    .await
    .map_err(|e| e.to_string())?
}
