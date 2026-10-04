//! The Tauri boundary for the design surface (#433): the preview base
//! URL, the list of previewable entries, and the reload watcher. The
//! folder always comes from the database by room id, never from the
//! frontend.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use tauri::State;
use tauri::ipc::Channel;

use super::PreviewState;
use crate::db::Database;
use crate::watcher::WatcherManager;

const MAX_DEPTH: usize = 12;
const MAX_ENTRIES: usize = 500;
const SKIPPED_DIRS: [&str; 2] = [".git", "node_modules"];

/// Where the preview server listens, or why it does not.
pub struct PreviewEndpoint(Result<u16, String>);

impl PreviewEndpoint {
    pub fn bound(port: u16) -> Self {
        Self(Ok(port))
    }

    pub fn failed(error: String) -> Self {
        Self(Err(error))
    }

    fn port(&self) -> Result<u16, String> {
        self.0
            .clone()
            .map_err(|e| format!("design preview server is not running: {e}"))
    }
}

pub(crate) fn room_root(db: &Database, room_id: &str) -> Result<PathBuf, String> {
    let room = db
        .room_by_id(room_id)?
        .ok_or_else(|| format!("unknown room {room_id}"))?;
    room.cwd
        .filter(|c| !c.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| format!("room {room_id} has no folder"))
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn design_preview_base(
    room_id: String,
    db: State<'_, Arc<Database>>,
    state: State<'_, Arc<PreviewState>>,
    endpoint: State<'_, PreviewEndpoint>,
) -> Result<String, String> {
    let port = endpoint.port()?;
    room_root(&db, &room_id)?;
    let token = state.mint(&room_id);
    Ok(format!("http://127.0.0.1:{port}/preview/{token}/"))
}

#[tauri::command]
pub async fn design_list_entries(
    room_id: String,
    db: State<'_, Arc<Database>>,
) -> Result<Vec<String>, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || list_entries_for_room(&db, &room_id))
        .await
        .map_err(|e| e.to_string())?
}

/// The previewable entries of a room's folder (blocking walk). The
/// non-Tauri core of `design_list_entries`, shared with the agent API's
/// design verbs (#512).
pub fn list_entries_for_room(db: &Database, room_id: &str) -> Result<Vec<String>, String> {
    let root = room_root(db, room_id)?;
    Ok(list_html_entries(&root))
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn design_watch_start(
    room_id: String,
    on_change: Channel<()>,
    db: State<'_, Arc<Database>>,
    manager: State<'_, WatcherManager>,
) -> Result<String, String> {
    let root = room_root(&db, &room_id)?;
    let canonical = dunce::canonicalize(&root).map_err(|e| e.to_string())?;
    let id = uuid::Uuid::new_v4().to_string();
    tracing::info!(room_id = %room_id, root = %canonical.display(), "design watch start");
    start_design_watch(&manager, id.clone(), &canonical, move || {
        // Fails only if the frontend dropped its half.
        let _ = on_change.send(());
    })
    .map_err(|e| e.to_string())?;
    Ok(id)
}

/// Wires `ChangeFilter` to a real watcher; `tick` fires per reload-worthy
/// batch. Split out so tests drive the same code `design_watch_start` runs.
fn start_design_watch<F>(
    manager: &WatcherManager,
    id: String,
    canonical: &Path,
    tick: F,
) -> Result<(), crate::watcher::WatcherError>
where
    F: Fn() + Send + 'static,
{
    let filter = Mutex::new(ChangeFilter::new(
        canonical.to_path_buf(),
        SystemTime::now(),
    ));
    manager.start_with_paths(id, canonical, move |paths| {
        let (count, changed) = match paths {
            None => (0, true),
            Some(p) => (
                p.len(),
                filter
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .observe(&p),
            ),
        };
        tracing::debug!(paths = count, ticked = changed, "design watch batch");
        if changed {
            tick();
        }
    })
}

/// Whether a debounced batch should reload the preview: anything outside
/// `.git` and `node_modules` does. Paths that are not under `root` count
/// as changes, the safe direction.
pub fn should_reload(root: &Path, paths: &[PathBuf]) -> bool {
    paths.iter().any(|p| {
        let rel = p.strip_prefix(root).unwrap_or(p);
        !rel.components().any(|c| {
            matches!(c, Component::Normal(n) if SKIPPED_DIRS.iter().any(|s| n == OsStr::new(s)))
        })
    })
}

/// Decides from filesystem state, not event kinds, whether a batch is a
/// real change. On Linux the inotify backend reports opens and reads, so
/// the preview server's own reads (and the entry listing each tick
/// triggers) would otherwise queue the next reload forever.
pub struct ChangeFilter {
    root: PathBuf,
    start: SystemTime,
    seen: HashMap<PathBuf, (Option<SystemTime>, u64)>,
}

impl ChangeFilter {
    pub fn new(root: PathBuf, start: SystemTime) -> Self {
        Self {
            root,
            start,
            seen: HashMap::new(),
        }
    }

    /// Whether the batch changed anything; every relevant path is
    /// recorded even once a change is found.
    pub fn observe(&mut self, paths: &[PathBuf]) -> bool {
        let mut changed = false;
        for p in paths {
            if !should_reload(&self.root, std::slice::from_ref(p)) {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(p) else {
                self.seen.remove(p);
                changed = true;
                continue;
            };
            if meta.is_dir() {
                // A file added or removed inside reports its own path.
                continue;
            }
            let stamp = (meta.modified().ok(), meta.len());
            match self.seen.insert(p.clone(), stamp) {
                Some(prev) => changed |= prev != stamp,
                None => changed |= stamp.0.is_none_or(|m| m >= self.start),
            }
        }
        changed
    }
}

/// Relative, `/`-separated, sorted paths of the `.html`/`.htm` files
/// under `root`. Capped in depth and count; symlinked directories are
/// not followed.
pub fn list_html_entries(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, "", 0, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, prefix: &str, depth: usize, out: &mut Vec<String>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        if out.len() >= MAX_ENTRIES {
            return;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if kind.is_dir() {
            if depth < MAX_DEPTH && !SKIPPED_DIRS.contains(&name.as_str()) {
                walk(&entry.path(), &rel, depth + 1, out);
            }
        } else if kind.is_file() {
            let is_html = Path::new(&name)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"));
            if is_html {
                out.push(rel);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn touch(root: &Path, rel: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "x").unwrap();
    }

    #[test]
    fn lists_html_sorted_and_skips_noise() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        touch(r, "b.html");
        touch(r, "A.HTM");
        touch(r, "sub/deep/c.html");
        touch(r, "notes.txt");
        touch(r, ".git/x.html");
        touch(r, "node_modules/pkg/y.html");
        touch(r, "sub/node_modules/z.html");
        assert_eq!(
            list_html_entries(r),
            vec!["A.HTM", "b.html", "sub/deep/c.html"]
        );
    }

    #[test]
    fn caps_depth_and_count() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        let deep = ["d"; MAX_DEPTH + 2].join("/");
        touch(r, &format!("{deep}/too-deep.html"));
        let ok = ["d"; MAX_DEPTH].join("/");
        touch(r, &format!("{ok}/fine.html"));
        assert_eq!(list_html_entries(r).len(), 1);

        for i in 0..MAX_ENTRIES + 20 {
            touch(r, &format!("many/{i}.html"));
        }
        assert_eq!(list_html_entries(r).len(), MAX_ENTRIES);
    }

    #[test]
    fn missing_root_is_empty() {
        assert_eq!(
            list_html_entries(Path::new("/definitely/not/here")),
            Vec::<String>::new()
        );
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_symlinked_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        touch(outside.path(), "out.html");
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        assert_eq!(list_html_entries(dir.path()), Vec::<String>::new());
    }

    fn reload(root: &str, paths: &[&str]) -> bool {
        let p: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        should_reload(Path::new(root), &p)
    }

    #[test]
    fn reload_ignores_git_and_node_modules() {
        assert!(!reload("/r", &["/r/.git"]));
        assert!(!reload("/r", &["/r/.git/index.lock"]));
        assert!(!reload("/r", &["/r/a/b/node_modules/x/y.js"]));
        assert!(!reload("/r", &["/r/.git/HEAD", "/r/node_modules"]));
    }

    #[test]
    fn reload_counts_real_changes() {
        assert!(reload("/r", &["/r/index.html"]));
        assert!(reload("/r", &["/r/.github/workflows/ci.yml"]));
        assert!(reload("/r", &["/r/.git/HEAD", "/r/a.css"]));
        assert!(reload("/r", &["/r/.gitignore"]));
        assert!(reload("/r", &["/r/my_node_modules/a.js"]));
    }

    fn filter(root: &Path, start_offset_secs: i64) -> ChangeFilter {
        let start = if start_offset_secs >= 0 {
            SystemTime::now() + Duration::from_secs(start_offset_secs.unsigned_abs())
        } else {
            SystemTime::now() - Duration::from_secs(start_offset_secs.unsigned_abs())
        };
        ChangeFilter::new(root.to_path_buf(), start)
    }

    #[test]
    fn filter_ignores_a_reread_of_an_unchanged_file() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "a.html");
        let p = dir.path().join("a.html");
        let mut f = filter(dir.path(), -5);
        assert!(f.observe(std::slice::from_ref(&p)), "new file after start");
        assert!(!f.observe(std::slice::from_ref(&p)));
        assert!(!f.observe(std::slice::from_ref(&p)));
    }

    #[test]
    fn filter_ticks_on_rewrite_and_removal() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "a.html");
        let p = dir.path().join("a.html");
        let mut f = filter(dir.path(), 5);
        assert!(!f.observe(std::slice::from_ref(&p)), "old unseen file");
        std::fs::write(&p, "longer body").unwrap();
        assert!(f.observe(std::slice::from_ref(&p)), "len changed");
        assert!(!f.observe(std::slice::from_ref(&p)));
        std::fs::remove_file(&p).unwrap();
        assert!(f.observe(std::slice::from_ref(&p)), "removed");
    }

    #[test]
    fn filter_ignores_directories_and_skipped_paths() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "sub/a.html");
        touch(dir.path(), ".git/x");
        let mut f = filter(dir.path(), -5);
        assert!(!f.observe(&[dir.path().join("sub")]));
        assert!(!f.observe(&[dir.path().join(".git/x")]));
    }

    fn rewrite_ticks(watch_root: &Path, file_dir: &Path) -> bool {
        let p = file_dir.join("proto/x.jsx");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "one").unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let manager = WatcherManager::new();
        let canonical = dunce::canonicalize(watch_root).unwrap();
        start_design_watch(&manager, "t".into(), &canonical, move || {
            let _ = tx.send(());
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(&p, "two, longer").unwrap();
        rx.recv_timeout(Duration::from_secs(3)).is_ok()
    }

    #[test]
    fn real_watcher_ticks_on_jsx_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        assert!(rewrite_ticks(dir.path(), dir.path()));
    }

    #[test]
    fn real_watcher_ticks_with_mixed_separator_root() {
        let dir = tempfile::tempdir().unwrap();
        let s = dir.path().to_string_lossy().into_owned();
        let mixed = PathBuf::from(if cfg!(windows) {
            s.replacen('\\', "/", 1)
        } else {
            format!("{s}/./")
        });
        assert!(rewrite_ticks(&mixed, dir.path()));
    }

    #[test]
    fn reload_handles_windows_paths() {
        if cfg!(windows) {
            assert!(!reload(r"C:\r", &[r"C:\r\.git\index"]));
            assert!(!reload(r"C:\r", &[r"C:\r\a\node_modules\b\c.js"]));
            assert!(reload(r"C:\r", &[r"C:\r\.github\x.yml"]));
            assert!(reload(r"C:\r", &[r"C:\r\page.html"]));
        }
    }
}
