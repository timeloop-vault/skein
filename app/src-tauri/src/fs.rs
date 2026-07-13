//! Filesystem helpers used by the Files overlay (issue #7, revived
//! for #49 stage 1).
//!
//! `list_dir` lists one level deep — just enough to render a
//! directory listing; `read_file_text` feeds the raw file view.
//! Symlinks are reported as `kind: "symlink"` and not followed — the
//! consumer can choose to navigate into them by listing the link's
//! target separately.
//!
//! Every command is scoped (#174): the requested path is
//! canonicalized (so symlinks can't smuggle a target out) and must
//! sit under one of the persisted rooms' cwds. Files outside every
//! room are a later stage of #49 (explicit OS-picker grants).

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;

use crate::db::Database;

const TEXT_MAX_BYTES: u64 = 256 * 1024;
const BINARY_SNIFF_BYTES: usize = 2048;

#[derive(Debug, Serialize)]
pub struct DirEntryDto {
    pub name: String,
    /// "file" | "dir" | "symlink"
    pub kind: &'static str,
    pub size: u64,
    /// Modification time as Unix epoch seconds. `None` if the platform
    /// doesn't expose it (rare).
    #[serde(rename = "mtimeSecs")]
    pub mtime_secs: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct FileTextDto {
    pub content: String,
    /// True when the file was longer than `TEXT_MAX_BYTES` and we
    /// truncated. UI shows a banner.
    pub truncated: bool,
}

/// Canonicalize `path` and require it to live under one of the
/// persisted rooms' cwds. Canonicalizing both sides resolves
/// symlinks before the prefix check, so a link inside a room can't
/// escape it. Room cwds that no longer exist are skipped.
pub(crate) fn ensure_room_scope(db: &Database, path: &str) -> Result<PathBuf, String> {
    let canon = std::fs::canonicalize(path).map_err(|e| format!("{path}: {e}"))?;
    for cwd in db.room_cwds()? {
        if let Ok(root) = std::fs::canonicalize(&cwd) {
            if canon.starts_with(&root) {
                return Ok(canon);
            }
        }
    }
    Err("path is outside every room's folder".into())
}

/// One-level directory listing. Hidden entries (`.foo`) are included —
/// the frontend filters by default but can opt-in. Common build /
/// dependency dirs are *not* skipped here either; that's a UX call,
/// not a filesystem call.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn list_dir(
    path: String,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<DirEntryDto>, String> {
    let canon = ensure_room_scope(&db, &path)?;
    let read = std::fs::read_dir(&canon).map_err(|e| format!("read_dir: {e}"))?;
    let mut out = Vec::new();
    for entry in read {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        // `metadata()` follows symlinks; `symlink_metadata()` doesn't.
        // We use the latter so a broken symlink shows up as a link
        // rather than disappearing. The reported size for a symlink
        // is the size of the link itself, which is fine.
        let Ok(meta) = entry.metadata() else { continue };
        let file_type = meta.file_type();
        let kind = if file_type.is_symlink() {
            "symlink"
        } else if file_type.is_dir() {
            "dir"
        } else {
            "file"
        };
        let mtime_secs = meta.modified().ok().and_then(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        });
        out.push(DirEntryDto {
            name,
            kind,
            size: meta.len(),
            mtime_secs,
        });
    }
    Ok(out)
}

/// Read up to `TEXT_MAX_BYTES` of `path` as text. Returns
/// `Err("binary")` when the leading sniff window contains a NUL byte
/// (cheap heuristic — robust enough for "is this a JPEG or a Rust
/// file" and matches `git diff`'s behaviour).
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn read_file_text(
    path: String,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<FileTextDto, String> {
    use std::io::Read;
    let canon = ensure_room_scope(&db, &path)?;
    let file = std::fs::File::open(&canon).map_err(|e| format!("open: {e}"))?;
    let meta = file.metadata().map_err(|e| format!("metadata: {e}"))?;
    let total_size = meta.len();
    let truncated = total_size > TEXT_MAX_BYTES;
    let read_size = total_size.min(TEXT_MAX_BYTES);
    let mut buf = Vec::with_capacity(usize::try_from(read_size).unwrap_or(0));
    file.take(read_size)
        .read_to_end(&mut buf)
        .map_err(|e| format!("read: {e}"))?;
    if buf.iter().take(BINARY_SNIFF_BYTES).any(|&b| b == 0) {
        return Err("binary".into());
    }
    Ok(FileTextDto {
        content: String::from_utf8_lossy(&buf).into_owned(),
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Harness, Room};
    use tempfile::TempDir;

    fn db_with_room_at(cwd: &std::path::Path) -> (TempDir, Database) {
        let db_dir = TempDir::new().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();
        let room = Room {
            id: "r1".into(),
            name: "room".into(),
            task: String::new(),
            status: "idle".into(),
            badge: 0,
            harnesses: Vec::<Harness>::new(),
            active_harness_id: String::new(),
            cwd: Some(cwd.to_string_lossy().into_owned()),
            branch: None,
            repo: None,
            archived: None,
        };
        db.save_all(&[room]).unwrap();
        (db_dir, db)
    }

    #[test]
    fn scope_allows_room_cwd_and_children() {
        let room_dir = TempDir::new().unwrap();
        let child = room_dir.path().join("sub");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(child.join("f.txt"), "hi").unwrap();
        let (_db_dir, db) = db_with_room_at(room_dir.path());
        assert!(ensure_room_scope(&db, room_dir.path().to_str().unwrap()).is_ok());
        assert!(ensure_room_scope(&db, child.join("f.txt").to_str().unwrap()).is_ok());
    }

    #[test]
    fn scope_rejects_paths_outside_every_room() {
        let room_dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "no").unwrap();
        let (_db_dir, db) = db_with_room_at(room_dir.path());
        let err = ensure_room_scope(&db, outside.path().join("secret.txt").to_str().unwrap())
            .unwrap_err();
        assert!(err.contains("outside"), "unexpected error: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn scope_rejects_symlink_escaping_the_room() {
        let room_dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "no").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            room_dir.path().join("link.txt"),
        )
        .unwrap();
        let (_db_dir, db) = db_with_room_at(room_dir.path());
        // The link lives inside the room, but canonicalize resolves it
        // to the outside target — must be rejected.
        let err =
            ensure_room_scope(&db, room_dir.path().join("link.txt").to_str().unwrap()).unwrap_err();
        assert!(err.contains("outside"), "unexpected error: {err}");
    }
}
