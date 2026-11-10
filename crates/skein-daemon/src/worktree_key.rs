use std::path::Path;

/// The worktree-relative, forward-slash path that identifies a file
/// everywhere in the review model — in the table, in the DTOs, and in
/// the commands.
///
/// `None` when the path is not inside the room's worktree. Harnesses do
/// touch files elsewhere (a global config, a temp file) and those are
/// not this room's review surface; refusing them here is also what
/// keeps a reject from ever writing outside the worktree.
pub fn relative_key(cwd: &str, reported: &str) -> Option<String> {
    let root = cwd.replace('\\', "/");
    let root = root.trim_end_matches('/');
    let raw = reported.replace('\\', "/");
    let abs = if Path::new(&raw).is_absolute() {
        raw
    } else {
        format!("{root}/{raw}")
    };

    let rel = strip_root(&abs, root)?;
    let rel = rel.trim_start_matches('/');
    if rel.is_empty() || rel.split('/').any(|s| s == ".." || s == ".") {
        return None;
    }
    Some(rel.to_string())
}

/// Strip `root` from the front of `abs`, case-insensitively on Windows
/// (where `C:/Git/...` and `c:/git/...` are the same directory and the
/// harness picks whichever spelling it likes).
fn strip_root<'a>(abs: &'a str, root: &str) -> Option<&'a str> {
    if abs.len() < root.len() {
        return None;
    }
    let (head, tail) = abs.split_at(root.len());
    let same = if cfg!(windows) {
        head.eq_ignore_ascii_case(root)
    } else {
        head == root
    };
    if !same {
        return None;
    }
    // Guard against `<root>-other/file` matching `<root>`.
    if !tail.is_empty() && !tail.starts_with('/') {
        return None;
    }
    Some(tail)
}
