//! Filesystem helpers used by the file-browser pane (issue #7).
//!
//! Two commands: `list_dir` (one level deep, just enough to render a
//! directory listing) and `read_file_text` (read a small text file
//! for inline preview). Symlinks are reported as `kind: "symlink"`
//! and not followed — the consumer can choose to navigate into them
//! by listing the link's target separately.

use std::path::Path;

use serde::Serialize;

const PREVIEW_MAX_BYTES: u64 = 256 * 1024;
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
pub struct FilePreviewDto {
    pub content: String,
    /// True when the file was longer than `PREVIEW_MAX_BYTES` and we
    /// truncated. UI shows a banner.
    pub truncated: bool,
}

/// One-level directory listing. Hidden entries (`.foo`) are included —
/// the frontend filters by default but can opt-in. Common build /
/// dependency dirs are *not* skipped here either; that's a UX call,
/// not a filesystem call.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn list_dir(path: String) -> Result<Vec<DirEntryDto>, String> {
    let read = std::fs::read_dir(Path::new(&path)).map_err(|e| format!("read_dir: {e}"))?;
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

/// Read up to `PREVIEW_MAX_BYTES` of `path` as text. Returns
/// `Err("binary")` when the leading sniff window contains a NUL byte
/// (cheap heuristic — robust enough for "is this a JPEG or a Rust
/// file" and matches `git diff`'s behaviour).
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn read_file_text(path: String) -> Result<FilePreviewDto, String> {
    use std::io::Read;
    let file = std::fs::File::open(Path::new(&path)).map_err(|e| format!("open: {e}"))?;
    let meta = file.metadata().map_err(|e| format!("metadata: {e}"))?;
    let total_size = meta.len();
    let truncated = total_size > PREVIEW_MAX_BYTES;
    let read_size = total_size.min(PREVIEW_MAX_BYTES);
    let mut buf = Vec::with_capacity(usize::try_from(read_size).unwrap_or(0));
    file.take(read_size)
        .read_to_end(&mut buf)
        .map_err(|e| format!("read: {e}"))?;
    if buf.iter().take(BINARY_SNIFF_BYTES).any(|&b| b == 0) {
        return Err("binary".into());
    }
    Ok(FilePreviewDto {
        content: String::from_utf8_lossy(&buf).into_owned(),
        truncated,
    })
}
