//! The review baseline, wired up — issue #211 (epic #52 D3, D4).
//!
//! [`skein_review`] owns the model (diff, accept, reject); this module
//! owns everything Tauri-shaped around it: where a baseline comes from,
//! when it is captured, and the three commands the Diff card calls.
//!
//! # When a baseline is captured
//!
//! Two knockers, both best-effort and both landing through
//! [`db::insert_review_baseline_if_absent`](crate::db::Database::insert_review_baseline_if_absent)
//! so neither can ever move an existing baseline:
//!
//! 1. A **live** `patch` row — see [`note_patch`], called from the
//!    Claude and opencode adapters. Not later, and deliberately not
//!    from the frontend: by the time the user looks at the room the
//!    agent may already have committed, and a baseline taken from a
//!    HEAD that has moved would show nothing pending, which is
//!    precisely the "changes silently auto-applied" failure #211
//!    exists to prevent. Backfill rows do not capture — replaying a
//!    harness's own history on attach is exactly the case where HEAD
//!    has already moved, so a room that predates this feature starts
//!    with a clean diff pane rather than a confidently wrong one.
//! 2. **Watcher discovery** (#221) — see [`discover`], fed every path a
//!    room's filesystem watcher reports changed (`review_discovery_start`
//!    in the commands section below), plus a one-shot [`catch_up`] run
//!    when that watcher starts, which walks `git status` to cover
//!    changes made while Skein itself was closed. This is what makes a
//!    shell write, a hand edit, or the `files` harness show up: none of
//!    them emit a `patch` row, only a filesystem event. A row captured
//!    this way carries an empty `harness_id` — the OS does not say
//!    *who* touched a file, so there is no chip until (if ever) a later
//!    live `patch` row claims it through `note_patch`'s touch path.
//!    Git only supplies the initial baseline *value* here (same as
//!    knocker 1) and the `catch_up` shortcut; a non-git room still
//!    discovers from live watcher ticks, it just cannot catch up on
//!    changes made before Skein was running to see them.
//!
//! # Where the content comes from
//!
//! The git HEAD blob, or nothing:
//!
//! 1. **Tracked at HEAD** — the committed content. The common case, and
//!    the one that makes committed work stay reviewable.
//! 2. **Anything else** (untracked, gitignored, a non-git room, an
//!    unborn HEAD) — [`FileState::Missing`], so the file reads as newly
//!    added. For a genuinely new file that is exactly right. For an
//!    untracked file that already existed it over-reports on first
//!    touch, and one accept settles it forever. Over-reporting is the
//!    safe direction: the invariant is that a pending change is never
//!    silently absorbed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;
use skein_review::{FileState, Hunk};
use tauri::Emitter;

use crate::agent_api::state::{REVIEW_CHANGED_EVENT, ReviewChanged};
use crate::db::Database;
use crate::watcher::WatcherManager;

// ── path keys ─────────────────────────────────────────────────────

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

/// Absolute on-disk path for a key. Only ever called with a key
/// [`relative_key`] produced, so it cannot escape the worktree.
pub(crate) fn abs_path(cwd: &str, key: &str) -> PathBuf {
    let mut p = PathBuf::from(cwd);
    for seg in key.split('/') {
        p.push(seg);
    }
    p
}

// ── capture ───────────────────────────────────────────────────────

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// The file(s) a `patch` row names. Mirrors `patchInfo` in
/// `app/src/liveContext/diff.ts`: a real single-file edit carries an
/// edit / write / multiedit tool, which is what excludes opencode's
/// multi-file commit snapshot (`{files, hash}`, no tool) and errored
/// patches.
fn patch_targets(payload: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<Value>(payload) else {
        return Vec::new();
    };
    if v.get("is_error") == Some(&Value::Bool(true)) {
        return Vec::new();
    }
    let tool = v
        .get("tool")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(tool.as_str(), "edit" | "write" | "multiedit") {
        return Vec::new();
    }
    let listed: Vec<String> = v
        .get("files")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if !listed.is_empty() {
        return listed;
    }
    let input = v.get("input");
    input
        .and_then(|i| i.get("filePath").or_else(|| i.get("file_path")))
        .and_then(Value::as_str)
        .map(|s| vec![s.to_string()])
        .unwrap_or_default()
}

/// The baseline content for a freshly touched path — see the module
/// docs for the two tiers. Opens its own repo handle; callers already
/// holding one (discovery, which processes a whole batch of paths)
/// should use [`capture_state_with_repo`] instead so each path doesn't
/// pay for a fresh `Repo::open`.
fn capture_state(cwd: &str, key: &str) -> FileState {
    let repo = skein_git::Repo::open(Path::new(cwd)).ok();
    capture_state_with_repo(repo.as_ref(), key)
}

/// Same as [`capture_state`], against an already-opened repo (or `None`
/// for a non-git room).
fn capture_state_with_repo(repo: Option<&skein_git::Repo>, key: &str) -> FileState {
    let Some(repo) = repo else {
        return FileState::Missing; // not a git room
    };
    match repo.head_blob(key) {
        Ok(Some(bytes)) => skein_review::classify_bytes(&bytes),
        Ok(None) => FileState::Missing,
        Err(e) => {
            tracing::warn!(path = key, error = %e, "review: head_blob failed; baselining as new");
            FileState::Missing
        }
    }
}

/// Record that a harness wrote something, capturing a baseline the
/// first time this room sees each path.
///
/// Best-effort and never propagates: a failure here must not take down
/// the adapter's tail loop. It is logged, and the worst case is one
/// file missing from the review surface until the next edit.
pub fn note_patch(db: &Database, room_id: &str, cwd: &str, harness_id: &str, payload: &str) {
    if cwd.is_empty() {
        return;
    }
    for reported in patch_targets(payload) {
        let Some(key) = relative_key(cwd, &reported) else {
            continue;
        };
        let ts = now_ms();
        match db.review_baseline(room_id, &key) {
            Ok(Some(_)) => {
                if let Err(e) = db.touch_review_baseline(room_id, &key, harness_id, ts) {
                    tracing::warn!(path = %key, error = %e, "review: touch failed");
                }
            }
            Ok(None) => {
                let (kind, content) = skein_review::encode(&capture_state(cwd, &key));
                if let Err(e) =
                    insert_or_attribute(db, room_id, &key, kind, content.as_deref(), harness_id, ts)
                {
                    tracing::warn!(path = %key, error = %e, "review: baseline capture failed");
                }
            }
            Err(e) => tracing::warn!(path = %key, error = %e, "review: baseline lookup failed"),
        }
    }
}

/// Insert a fresh baseline, or — when a concurrent capture (watcher
/// discovery racing this same live `patch` row, or the reverse) already
/// won — fall through to attribution only.
///
/// `discover_batch` runs on the debouncer thread and can insert the
/// same row (with `harness_id == ""`) in the window between this
/// caller's own `review_baseline` read and its insert. Losing the
/// insert race must not lose the harness chip: content is never
/// re-captured once a row exists (the #211 invariant), but `harness_id`
/// and `touched_ms` still move, the same as the `Ok(Some(_))` arm above
/// does for an ordinary second edit.
fn insert_or_attribute(
    db: &Database,
    room_id: &str,
    key: &str,
    kind: &str,
    content: Option<&str>,
    harness_id: &str,
    ts: i64,
) -> Result<(), String> {
    if db.insert_review_baseline_if_absent(room_id, key, kind, content, harness_id, ts)? {
        return Ok(());
    }
    db.touch_review_baseline(room_id, key, harness_id, ts)
}

// ── discovery (#221) ─────────────────────────────────────────────

/// Feed a batch of watcher-reported paths (absolute or relative)
/// through the same capture as a live `patch` row, but with no
/// attribution: the filesystem doesn't say who wrote a file, so a row
/// discovered here starts with an empty `harness_id` — no chip until a
/// later `patch` row claims it via `note_patch`'s touch path.
///
/// Returns how many baselines were newly captured. Best-effort like
/// [`note_patch`]: logs and moves on rather than propagating, and never
/// touches a path that already has a baseline — recapturing would
/// silently move the baseline behind the user's back and absorb
/// whatever they haven't reviewed yet (the same #211 invariant
/// [`note_patch`] protects).
pub(crate) fn discover(
    db: &Database,
    room_id: &str,
    cwd: &str,
    reported: impl IntoIterator<Item = String>,
) -> usize {
    if cwd.is_empty() {
        return 0;
    }
    let repo = skein_git::Repo::open(Path::new(cwd)).ok();
    let (mut inserted, needs_catch_up) =
        discover_batch(db, room_id, cwd, repo.as_ref(), reported, true);
    if needs_catch_up {
        if let Some(repo) = &repo {
            inserted += catch_up_with(db, room_id, cwd, repo);
        }
        // A non-git room has nothing to catch up against — nothing
        // else to do.
    }
    inserted
}

/// One-shot `git status` sweep, run when a room's watcher (re)starts so
/// changes made while Skein itself was closed still surface. A non-git
/// room has no status to walk and returns 0 — see the module docs.
pub(crate) fn catch_up(db: &Database, room_id: &str, cwd: &str) -> usize {
    if cwd.is_empty() {
        return 0;
    }
    let Ok(repo) = skein_git::Repo::open(Path::new(cwd)) else {
        return 0;
    };
    catch_up_with(db, room_id, cwd, &repo)
}

fn catch_up_with(db: &Database, room_id: &str, cwd: &str, repo: &skein_git::Repo) -> usize {
    let entries = match repo.status() {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(cwd, error = %e, "review: discovery catch-up status failed");
            return 0;
        }
    };
    // `discover_batch`'s own needs-catch-up detection is moot here —
    // every path status() reports already exists or is a real tracked
    // deletion, never a directory collapsed into one OS event — so the
    // flag it returns is ignored. `status()` also already excludes
    // ignored files, so skip the redundant `is_path_ignored` re-check.
    let (inserted, _) = discover_batch(
        db,
        room_id,
        cwd,
        Some(repo),
        entries.into_iter().map(|e| e.path),
        false,
    );
    inserted
}

/// The shared walk behind [`discover`] and [`catch_up`]. Returns
/// `(inserted, needs_catch_up)` — the latter set when a reported path
/// no longer exists on disk, has no HEAD blob either, AND was a
/// **directory** at HEAD ([`skein_git::Repo::head_is_dir`]): the OS
/// folds a deleted directory into a single watcher event, so the caller
/// re-derives the individual deleted files via `git status`. An
/// ordinary file that is merely gone by the time discovery runs — a
/// lockfile, an editor swap file, anything create-then-delete — is
/// *not* a directory at HEAD and must not trigger that sweep.
///
/// `check_ignores` skips the `is_path_ignored` call for callers (the
/// `catch_up` path) whose `reported` already came from `git status`,
/// which excludes ignored files itself.
fn discover_batch(
    db: &Database,
    room_id: &str,
    cwd: &str,
    repo: Option<&skein_git::Repo>,
    reported: impl IntoIterator<Item = String>,
    check_ignores: bool,
) -> (usize, bool) {
    let mut inserted = 0;
    let mut needs_catch_up = false;
    for reported_path in reported {
        let Some(key) = relative_key(cwd, &reported_path) else {
            continue;
        };
        if key.split('/').any(|seg| seg == ".git") {
            continue;
        }
        match db.review_baseline(room_id, &key) {
            Ok(Some(_)) => continue, // never recapture — see module docs
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(path = %key, error = %e, "review: discovery baseline lookup failed");
                continue;
            }
        }
        if check_ignores {
            if let Some(repo) = repo {
                match repo.is_path_ignored(&key) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    // Can't prove it's ignored — the safe direction is
                    // to keep going rather than silently drop a real
                    // change.
                    Err(e) => {
                        tracing::warn!(path = %key, error = %e, "review: discovery ignore check failed");
                    }
                }
            }
        }
        let abs = abs_path(cwd, &key);
        // `symlink_metadata` so a symlink-to-directory doesn't get
        // followed into "yes, it's a directory".
        let meta = std::fs::symlink_metadata(&abs);
        if meta.as_ref().is_ok_and(std::fs::Metadata::is_dir) {
            continue; // not a file to baseline
        }
        let exists = meta.is_ok();
        let base = capture_state_with_repo(repo, &key);
        if !exists && base == FileState::Missing {
            let was_dir_at_head = repo.is_some_and(|r| match r.head_is_dir(&key) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(path = %key, error = %e, "review: discovery head_is_dir check failed");
                    false
                }
            });
            if was_dir_at_head {
                needs_catch_up = true;
            }
        }
        let disk = skein_review::read_state(&abs);
        if base == disk {
            continue; // nothing pending — a later write fires another event
        }
        let (kind, content) = skein_review::encode(&base);
        match db.insert_review_baseline_if_absent(
            room_id,
            &key,
            kind,
            content.as_deref(),
            "",
            now_ms(),
        ) {
            Ok(true) => inserted += 1,
            Ok(false) => {} // lost a race with another capture — fine
            Err(e) => {
                tracing::warn!(path = %key, error = %e, "review: discovery baseline capture failed");
            }
        }
    }
    (inserted, needs_catch_up)
}

// ── DTOs ──────────────────────────────────────────────────────────

/// One file with unreviewed changes. `hunks` reuses `skein-review`'s
/// shape, which matches the Diff card's existing renderer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingFileDto {
    /// Worktree-relative, forward slashes — the identity for every
    /// command in this module.
    pub path: String,
    /// Last segment, for the tab label.
    pub name: String,
    /// Harness that most recently wrote this file. A chip and a filter,
    /// never a partition (D4).
    pub harness_id: String,
    /// `added` / `deleted` / `modified`, relative to the baseline.
    pub change: &'static str,
    /// Why this file has no line diff, when it has none: `binary`,
    /// `toolarge`, `symlink` or `unreadable`. The pane says so rather
    /// than dropping the file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<&'static str>,
    pub additions: usize,
    pub deletions: usize,
    /// The working tree's content hash as a decimal string — a JS
    /// number cannot hold 64 bits exactly. Echoed back on a whole-file
    /// accept or reject as the staleness guard.
    pub content_hash: String,
    pub hunks: Vec<Hunk>,
    /// Epoch ms of the last harness write, for tab ordering.
    pub touched_ms: i64,
}

fn blocked_reason(state: &FileState) -> Option<&'static str> {
    match state {
        FileState::Binary { .. } => Some("binary"),
        FileState::TooLarge { .. } => Some("toolarge"),
        FileState::Symlink => Some("symlink"),
        FileState::Unreadable => Some("unreadable"),
        FileState::Text(_) | FileState::Missing => None,
    }
}

pub(crate) fn pending_impl(
    db: &Database,
    room_id: &str,
    cwd: &str,
) -> Result<Vec<PendingFileDto>, String> {
    let mut out = Vec::new();
    for b in db.review_baselines_for_room(room_id)? {
        let base = skein_review::decode(&b.kind, b.content.as_deref());
        let disk = skein_review::read_state(&abs_path(cwd, &b.path));
        if base == disk {
            continue; // reviewed — this is the only thing that clears a tab
        }
        let (change, blocked, hunks) = if base.is_diffable() && disk.is_diffable() {
            let hunks = skein_review::diff_lines(base.as_diff_text(), disk.as_diff_text());
            if hunks.is_empty() {
                continue;
            }
            let change = match (&base, &disk) {
                (FileState::Missing, _) => "added",
                (_, FileState::Missing) => "deleted",
                _ => "modified",
            };
            (change, None, hunks)
        } else {
            // One side cannot be diffed. Report the file and why.
            let reason = blocked_reason(&disk).or_else(|| blocked_reason(&base));
            ("modified", reason, Vec::new())
        };
        out.push(PendingFileDto {
            name: b.path.rsplit('/').next().unwrap_or(&b.path).to_string(),
            additions: hunks.iter().map(Hunk::additions).sum(),
            deletions: hunks.iter().map(Hunk::deletions).sum(),
            content_hash: skein_review::content_hash(&disk).to_string(),
            path: b.path,
            harness_id: b.harness_id,
            change,
            blocked,
            hunks,
            touched_ms: b.touched_ms,
        });
    }
    // Oldest touch first, so the tab strip reads left-to-right in the
    // order the agent worked — the ordering the card already had.
    out.sort_by(|a, b| a.touched_ms.cmp(&b.touched_ms).then(a.path.cmp(&b.path)));
    Ok(out)
}

// ── accept / reject ───────────────────────────────────────────────

const STALE: &str = "the file changed since it was rendered — refresh and try again";

fn guard_hash(disk: &FileState, expected: Option<&str>) -> Result<(), String> {
    match expected {
        Some(want) if want != skein_review::content_hash(disk).to_string() => {
            Err(STALE.to_string())
        }
        _ => Ok(()),
    }
}

/// Advance the baseline for one file, choosing the state to store.
///
/// When the accepted text equals what is on disk we store the disk
/// *state*, not `Text(...)`: accepting the deletion of a file must
/// leave the baseline `Missing`, or the file would immediately read as
/// newly added again.
fn store_advance(
    db: &Database,
    room_id: &str,
    path: &str,
    next_text: &str,
    disk: &FileState,
) -> Result<(), String> {
    let state = if next_text == disk.as_diff_text() {
        disk.clone()
    } else {
        FileState::Text(next_text.to_string())
    };
    let (kind, content) = skein_review::encode(&state);
    db.advance_review_baseline(room_id, path, kind, content.as_deref(), now_ms())
}

fn accept_impl(
    db: &Database,
    room_id: &str,
    cwd: &str,
    path: Option<&str>,
    hunks: &[Hunk],
    content_hash: Option<&str>,
) -> Result<usize, String> {
    // Accept-all: every pending file moves to whatever is on disk.
    let Some(path) = path else {
        let pending = pending_impl(db, room_id, cwd)?;
        for f in &pending {
            let disk = skein_review::read_state(&abs_path(cwd, &f.path));
            let (kind, content) = skein_review::encode(&disk);
            db.advance_review_baseline(room_id, &f.path, kind, content.as_deref(), now_ms())?;
        }
        return Ok(pending.len());
    };

    let Some(row) = db.review_baseline(room_id, path)? else {
        return Err(format!("no review baseline for {path}"));
    };
    let base = skein_review::decode(&row.kind, row.content.as_deref());
    let disk = skein_review::read_state(&abs_path(cwd, path));

    if hunks.is_empty() {
        // Whole file.
        guard_hash(&disk, content_hash)?;
        let (kind, content) = skein_review::encode(&disk);
        db.advance_review_baseline(room_id, path, kind, content.as_deref(), now_ms())?;
        return Ok(1);
    }

    if !base.is_diffable() || !disk.is_diffable() {
        return Err(format!("{path} has no line diff to accept hunk-by-hunk"));
    }
    let next = skein_review::accept(base.as_diff_text(), disk.as_diff_text(), hunks)
        .map_err(|e| e.to_string())?;
    store_advance(db, room_id, path, &next, &disk)?;
    Ok(1)
}

fn reject_impl(
    db: &Database,
    room_id: &str,
    cwd: &str,
    path: &str,
    hunks: &[Hunk],
    content_hash: Option<&str>,
) -> Result<(), String> {
    let Some(row) = db.review_baseline(room_id, path)? else {
        return Err(format!("no review baseline for {path}"));
    };
    let base = skein_review::decode(&row.kind, row.content.as_deref());
    let disk = skein_review::read_state(&abs_path(cwd, path));
    let target = abs_path(cwd, path);

    // Whole file: put the baseline back verbatim.
    if hunks.is_empty() {
        guard_hash(&disk, content_hash)?;
        return match &base {
            FileState::Missing => skein_review::remove_file(&target).map_err(|e| e.to_string()),
            FileState::Text(t) => skein_review::write_text(&target, t).map_err(|e| e.to_string()),
            _ => Err(format!(
                "{path}'s baseline is not stored as text, so it cannot be restored"
            )),
        };
    }

    if !base.is_diffable() || !disk.is_diffable() {
        return Err(format!("{path} has no line diff to reject hunk-by-hunk"));
    }
    let next = skein_review::reject(base.as_diff_text(), disk.as_diff_text(), hunks)
        .map_err(|e| e.to_string())?;
    // Rejecting every hunk of a file the agent created means the file
    // should be gone, not empty.
    if next.is_empty() && base == FileState::Missing {
        return skein_review::remove_file(&target).map_err(|e| e.to_string());
    }
    skein_review::write_text(&target, &next).map_err(|e| e.to_string())
}

// ── commands ──────────────────────────────────────────────────────

/// Every file in the room with unreviewed changes.
///
/// Runs on the blocking pool: it stats and reads one file per baseline
/// and the Diff card calls it on every debounced watcher tick, which is
/// exactly the shape of work #171/#172 want off the main thread.
#[tauri::command]
pub async fn review_pending(
    room_id: String,
    cwd: String,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<PendingFileDto>, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || pending_impl(&db, &room_id, &cwd))
        .await
        .map_err(|e| e.to_string())?
}

/// Advance the baseline. **Never writes to the working tree.**
///
/// `path` `None` accepts every pending file in the room; `hunks` empty
/// accepts the whole file. Returns how many files moved.
#[tauri::command]
pub async fn review_accept(
    room_id: String,
    cwd: String,
    path: Option<String>,
    hunks: Vec<Hunk>,
    content_hash: Option<String>,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<usize, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        accept_impl(
            &db,
            &room_id,
            &cwd,
            path.as_deref(),
            &hunks,
            content_hash.as_deref(),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Put the working tree back to the baseline. **Never moves the
/// baseline.** `hunks` empty rejects the whole file.
///
/// There is deliberately no reject-all: it is a destructive verb and
/// wants to be chosen per file.
#[tauri::command]
pub async fn review_reject(
    room_id: String,
    cwd: String,
    path: String,
    hunks: Vec<Hunk>,
    content_hash: Option<String>,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        reject_impl(&db, &room_id, &cwd, &path, &hunks, content_hash.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Start watching `cwd` for review discovery (#221): every debounced
/// change re-runs [`discover`] over just the paths that moved, and a
/// notify error (dropped events) falls back to [`catch_up`]'s full
/// `git status` sweep. Returns an opaque id — stop it with the existing
/// `git_watch_stop` (same [`WatcherManager`], no new stop command).
///
/// Also runs one [`catch_up`] immediately, off the async runtime, so a
/// room reopened after Skein was closed for a while sees what changed
/// meanwhile — the reason this command exists rather than callers just
/// reusing `git_watch_start` with a `review_pending`-shaped callback.
///
/// A capture emits [`REVIEW_CHANGED_EVENT`] so the review pane's
/// `useAgentWrites` re-fetches without racing its own worktree watcher
/// tick (both are debounced independently).
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn review_discovery_start(
    room_id: String,
    cwd: String,
    app: tauri::AppHandle,
    db: tauri::State<'_, Arc<Database>>,
    manager: tauri::State<'_, WatcherManager>,
) -> Result<String, String> {
    if cwd.is_empty() {
        return Err("cwd must not be empty".to_string());
    }
    let id = uuid::Uuid::new_v4().to_string();
    let db_for_watch = Arc::clone(&db);
    let app_for_watch = app.clone();
    let room_for_watch = room_id.clone();
    let cwd_for_watch = cwd.clone();
    manager
        .start_with_paths(id.clone(), Path::new(&cwd), move |paths| {
            let inserted = match paths {
                Some(paths) => discover(
                    &db_for_watch,
                    &room_for_watch,
                    &cwd_for_watch,
                    paths_to_reported(paths),
                ),
                None => catch_up(&db_for_watch, &room_for_watch, &cwd_for_watch),
            };
            if inserted > 0 {
                emit_review_changed(&app_for_watch, &room_for_watch);
            }
        })
        .map_err(|e| e.to_string())?;

    // The initial sweep: catches changes made while this room had no
    // watcher running (Skein closed, or the tab was never opened).
    let db_for_catchup = Arc::clone(&db);
    let room_for_catchup = room_id;
    let cwd_for_catchup = cwd;
    tauri::async_runtime::spawn_blocking(move || {
        let inserted = catch_up(&db_for_catchup, &room_for_catchup, &cwd_for_catchup);
        if inserted > 0 {
            emit_review_changed(&app, &room_for_catchup);
        }
    });

    Ok(id)
}

fn paths_to_reported(paths: Vec<PathBuf>) -> Vec<String> {
    paths
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

fn emit_review_changed(app: &tauri::AppHandle, room_id: &str) {
    if let Err(e) = app.emit(
        REVIEW_CHANGED_EVENT,
        ReviewChanged {
            room_id: room_id.to_owned(),
        },
    ) {
        tracing::warn!(room_id, error = %e, "review: discovery-changed emit failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // ── path keys ─────────────────────────────────────────────────

    /// An absolute-path prefix for this platform. `C:/…` is not absolute
    /// on Unix, so a Windows-only literal would be joined under the
    /// worktree and accepted there (#281).
    const ABS: &str = if cfg!(windows) { "C:" } else { "" };

    #[test]
    fn relative_key_normalizes_separators_and_scopes_to_the_worktree() {
        let cwd = format!("{ABS}/git/skein-wt/room");
        let cwd = cwd.as_str();
        #[cfg(windows)]
        assert_eq!(
            relative_key(cwd, r"C:\git\skein-wt\room\src\a.rs").as_deref(),
            Some("src/a.rs")
        );
        assert_eq!(
            relative_key(cwd, &format!("{ABS}/git/skein-wt/room/src/a.rs")).as_deref(),
            Some("src/a.rs")
        );
        // Already relative — interpreted against the worktree.
        assert_eq!(relative_key(cwd, "src/a.rs").as_deref(), Some("src/a.rs"));
    }

    #[test]
    fn relative_key_refuses_anything_outside_the_worktree() {
        let cwd = format!("{ABS}/git/skein-wt/room");
        let cwd = cwd.as_str();
        // A sibling directory that merely shares a prefix.
        assert_eq!(
            relative_key(cwd, &format!("{ABS}/git/skein-wt/room-other/a.rs")),
            None
        );
        // Somewhere else entirely — harnesses do edit global config.
        assert_eq!(
            relative_key(cwd, &format!("{ABS}/Users/x/.claude/settings.json")),
            None
        );
        // Traversal, however it is spelled.
        assert_eq!(relative_key(cwd, "../escape.rs"), None);
        assert_eq!(
            relative_key(cwd, &format!("{ABS}/git/skein-wt/room/../escape.rs")),
            None
        );
        // The worktree root itself is not a file.
        assert_eq!(relative_key(cwd, &format!("{ABS}/git/skein-wt/room")), None);
    }

    #[cfg(windows)]
    #[test]
    fn relative_key_is_case_insensitive_on_windows() {
        assert_eq!(
            relative_key("C:/Git/Room", "c:/git/room/src/a.rs").as_deref(),
            Some("src/a.rs")
        );
    }

    // ── payload parsing ───────────────────────────────────────────

    #[test]
    fn patch_targets_reads_the_shapes_both_adapters_emit() {
        assert_eq!(
            patch_targets(r#"{"tool":"Edit","files":["/w/a.rs"]}"#),
            ["/w/a.rs"]
        );
        assert_eq!(
            patch_targets(r#"{"tool":"write","input":{"filePath":"/w/b.rs"}}"#),
            ["/w/b.rs"]
        );
        assert_eq!(
            patch_targets(r#"{"tool":"MultiEdit","input":{"file_path":"/w/c.rs"}}"#),
            ["/w/c.rs"]
        );
    }

    #[test]
    fn patch_targets_ignores_errors_and_the_opencode_commit_snapshot() {
        // Errored patch — nothing landed on disk.
        assert!(patch_targets(r#"{"tool":"Edit","files":["/w/a.rs"],"is_error":true}"#).is_empty());
        // opencode's multi-file commit snapshot: no tool, so it must
        // not own a baseline (same exclusion as diff.ts's patchInfo).
        assert!(patch_targets(r#"{"files":["/w/a.rs","/w/b.rs"],"hash":"abc"}"#).is_empty());
        assert!(patch_targets(r#"{"tool":"Read","files":["/w/a.rs"]}"#).is_empty());
        assert!(patch_targets("not json").is_empty());
    }

    // ── fixtures ──────────────────────────────────────────────────

    struct Room {
        _dir: TempDir,
        cwd: String,
        db: Database,
    }

    impl Room {
        /// A git room with `src/a.rs` committed.
        fn new() -> Self {
            let dir = TempDir::new().unwrap();
            let cwd = dir.path().to_string_lossy().to_string();
            let repo = git2::Repository::init(dir.path()).unwrap();
            fs::create_dir_all(dir.path().join("src")).unwrap();
            fs::write(dir.path().join("src/a.rs"), "one\ntwo\nthree\n").unwrap();
            commit(&repo, "init");
            let db = Database::open(&dir.path().join("skein.db")).unwrap();
            Self { _dir: dir, cwd, db }
        }

        /// A room that is not a git repo at all.
        fn bare() -> Self {
            let dir = TempDir::new().unwrap();
            let cwd = dir.path().to_string_lossy().to_string();
            let db = Database::open(&dir.path().join("skein.db")).unwrap();
            Self { _dir: dir, cwd, db }
        }

        fn path(&self, rel: &str) -> PathBuf {
            abs_path(&self.cwd, rel)
        }

        fn write(&self, rel: &str, text: &str) {
            let p = self.path(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }

        fn read(&self, rel: &str) -> String {
            fs::read_to_string(self.path(rel)).unwrap()
        }

        /// What the adapters do: a live patch row naming `rel`.
        fn touched(&self, harness: &str, rel: &str) {
            let quoted =
                serde_json::to_string(&self.path(rel).to_string_lossy().to_string()).unwrap();
            let payload = format!("{{\"tool\":\"Edit\",\"files\":[{quoted}]}}");
            note_patch(&self.db, "r1", &self.cwd, harness, &payload);
        }

        fn pending(&self) -> Vec<PendingFileDto> {
            pending_impl(&self.db, "r1", &self.cwd).unwrap()
        }

        fn commit_all(&self, msg: &str) {
            let repo = git2::Repository::open(&self.cwd).unwrap();
            commit(&repo, msg);
        }

        fn numbered(&self, rel: &str, replacements: &[(usize, &str)]) -> String {
            let mut lines: Vec<String> = (1..=20).map(|i| format!("{i}\n")).collect();
            for (i, text) in replacements {
                lines[*i] = (*text).to_string();
            }
            let joined = lines.concat();
            self.write(rel, &joined);
            joined
        }

        /// What the watcher reports: absolute paths, the shape
        /// `event.path` comes in as.
        fn discover(&self, rels: &[&str]) -> usize {
            discover(
                &self.db,
                "r1",
                &self.cwd,
                rels.iter()
                    .map(|r| self.path(r).to_string_lossy().to_string()),
            )
        }

        fn catch_up(&self) -> usize {
            catch_up(&self.db, "r1", &self.cwd)
        }
    }

    fn commit(repo: &git2::Repository, msg: &str) {
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@example.com").unwrap();
        let parents = repo
            .head()
            .ok()
            .and_then(|h| h.peel_to_commit().ok())
            .map(|c| vec![c])
            .unwrap_or_default();
        let refs: Vec<&git2::Commit> = parents.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &refs)
            .unwrap();
    }

    // ── capture ───────────────────────────────────────────────────

    #[test]
    fn first_touch_baselines_the_committed_content_not_what_is_on_disk() {
        let room = Room::new();
        // The agent edits, and only then does the patch row reach us.
        room.write("src/a.rs", "one\nTWO\nthree\n");
        room.touched("h1", "src/a.rs");

        let pending = room.pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].path, "src/a.rs");
        assert_eq!(pending[0].change, "modified");
        assert_eq!((pending[0].additions, pending[0].deletions), (1, 1));
        assert_eq!(pending[0].harness_id, "h1");
    }

    #[test]
    fn work_stays_pending_after_the_agent_commits_it() {
        // The property the whole issue turns on: the review unit is
        // committed work, so a commit must not clear the diff.
        let room = Room::new();
        room.write("src/a.rs", "one\nTWO\nthree\n");
        room.touched("h1", "src/a.rs");
        room.commit_all("agent commit");

        let pending = room.pending();
        assert_eq!(pending.len(), 1, "a commit must not clear the review");
        assert_eq!((pending[0].additions, pending[0].deletions), (1, 1));
    }

    #[test]
    fn a_second_edit_still_diffs_against_the_original_baseline() {
        let room = Room::new();
        room.write("src/a.rs", "one\nTWO\nthree\n");
        room.touched("h1", "src/a.rs");
        room.write("src/a.rs", "one\nTWO\nTHREE\n");
        room.touched("h2", "src/a.rs");

        let pending = room.pending();
        assert_eq!((pending[0].additions, pending[0].deletions), (2, 2));
        assert_eq!(
            pending[0].harness_id, "h2",
            "the chip follows the latest write"
        );
    }

    #[test]
    fn a_file_outside_the_worktree_is_not_baselined() {
        let room = Room::new();
        note_patch(
            &room.db,
            "r1",
            &room.cwd,
            "h1",
            r#"{"tool":"Edit","files":["/somewhere/else/global.json"]}"#,
        );
        assert!(room.pending().is_empty());
    }

    #[test]
    fn a_new_file_in_a_non_git_room_reads_as_added() {
        let room = Room::bare();
        room.write("notes.md", "hello\n");
        room.touched("h1", "notes.md");

        let pending = room.pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].change, "added");
        assert_eq!(pending[0].deletions, 0);
    }

    // ── accept ────────────────────────────────────────────────────

    #[test]
    fn accepting_a_whole_file_clears_it_and_leaves_the_worktree_untouched() {
        let room = Room::new();
        room.write("src/a.rs", "one\nTWO\nthree\n");
        room.touched("h1", "src/a.rs");
        let before = room.read("src/a.rs");
        let hash = room.pending()[0].content_hash.clone();

        assert_eq!(
            accept_impl(
                &room.db,
                "r1",
                &room.cwd,
                Some("src/a.rs"),
                &[],
                Some(&hash)
            )
            .unwrap(),
            1
        );
        assert!(room.pending().is_empty(), "accept must clear the diff");
        assert_eq!(room.read("src/a.rs"), before, "accept must not touch disk");
    }

    #[test]
    fn accepting_one_hunk_leaves_the_other_pending() {
        let room = Room::new();
        room.numbered("src/wide.rs", &[]);
        room.commit_all("wide");
        room.touched("h1", "src/wide.rs");
        // Two changes, far enough apart to be separate hunks.
        room.numbered("src/wide.rs", &[(1, "TWO\n"), (17, "EIGHTEEN\n")]);

        let file = room.pending().into_iter().next().unwrap();
        assert_eq!(file.hunks.len(), 2);
        accept_impl(
            &room.db,
            "r1",
            &room.cwd,
            Some("src/wide.rs"),
            &file.hunks[..1],
            None,
        )
        .unwrap();

        let after = room.pending();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].hunks.len(), 1);
        assert!(
            after[0].hunks[0]
                .lines
                .iter()
                .any(|l| l.content == "EIGHTEEN")
        );
    }

    #[test]
    fn accepting_a_deletion_leaves_the_baseline_missing_not_empty() {
        let room = Room::new();
        room.touched("h1", "src/a.rs");
        fs::remove_file(room.path("src/a.rs")).unwrap();
        assert_eq!(room.pending()[0].change, "deleted");

        let file = room.pending().into_iter().next().unwrap();
        accept_impl(
            &room.db,
            "r1",
            &room.cwd,
            Some("src/a.rs"),
            &file.hunks,
            None,
        )
        .unwrap();
        assert!(room.pending().is_empty());

        // Re-creating it must read as added, not as "unchanged".
        room.write("src/a.rs", "back\n");
        assert_eq!(room.pending()[0].change, "added");
    }

    #[test]
    fn accept_all_clears_every_pending_file() {
        let room = Room::new();
        room.write("src/a.rs", "one\nTWO\nthree\n");
        room.touched("h1", "src/a.rs");
        room.write("src/b.rs", "new\n");
        room.touched("h2", "src/b.rs");
        assert_eq!(room.pending().len(), 2);

        assert_eq!(
            accept_impl(&room.db, "r1", &room.cwd, None, &[], None).unwrap(),
            2
        );
        assert!(room.pending().is_empty());
    }

    #[test]
    fn a_stale_hash_refuses_the_accept() {
        let room = Room::new();
        room.write("src/a.rs", "one\nTWO\nthree\n");
        room.touched("h1", "src/a.rs");
        let hash = room.pending()[0].content_hash.clone();
        // The agent writes again between render and click.
        room.write("src/a.rs", "one\nTWO AGAIN\nthree\n");

        let err = accept_impl(
            &room.db,
            "r1",
            &room.cwd,
            Some("src/a.rs"),
            &[],
            Some(&hash),
        )
        .unwrap_err();
        assert!(err.contains("refresh"), "unexpected error: {err}");
        assert_eq!(room.pending().len(), 1, "nothing may have been absorbed");
    }

    // ── reject ────────────────────────────────────────────────────

    #[test]
    fn rejecting_a_whole_file_restores_the_baseline_on_disk() {
        let room = Room::new();
        room.write("src/a.rs", "one\nTWO\nthree\n");
        room.touched("h1", "src/a.rs");
        let hash = room.pending()[0].content_hash.clone();

        reject_impl(&room.db, "r1", &room.cwd, "src/a.rs", &[], Some(&hash)).unwrap();
        assert_eq!(room.read("src/a.rs"), "one\ntwo\nthree\n");
        assert!(room.pending().is_empty());
    }

    #[test]
    fn rejecting_a_file_the_agent_created_deletes_it() {
        let room = Room::new();
        room.write("src/new.rs", "unwanted\n");
        room.touched("h1", "src/new.rs");
        assert_eq!(room.pending()[0].change, "added");

        reject_impl(&room.db, "r1", &room.cwd, "src/new.rs", &[], None).unwrap();
        assert!(!room.path("src/new.rs").exists());
        assert!(room.pending().is_empty());
    }

    #[test]
    fn rejecting_one_hunk_leaves_the_other_on_disk() {
        let room = Room::new();
        room.numbered("src/wide.rs", &[]);
        room.commit_all("wide");
        room.touched("h1", "src/wide.rs");
        room.numbered("src/wide.rs", &[(1, "TWO\n"), (17, "EIGHTEEN\n")]);

        let file = room.pending().into_iter().next().unwrap();
        reject_impl(
            &room.db,
            "r1",
            &room.cwd,
            "src/wide.rs",
            &file.hunks[1..],
            None,
        )
        .unwrap();

        let disk = room.read("src/wide.rs");
        assert!(disk.contains("TWO\n"), "the unrejected hunk must survive");
        assert!(!disk.contains("EIGHTEEN"));
    }

    #[test]
    fn rejecting_a_deletion_puts_the_file_back() {
        let room = Room::new();
        room.touched("h1", "src/a.rs");
        fs::remove_file(room.path("src/a.rs")).unwrap();

        reject_impl(&room.db, "r1", &room.cwd, "src/a.rs", &[], None).unwrap();
        assert_eq!(room.read("src/a.rs"), "one\ntwo\nthree\n");
        assert!(room.pending().is_empty());
    }

    #[test]
    fn reject_never_moves_the_baseline() {
        let room = Room::new();
        room.write("src/a.rs", "one\nTWO\nthree\n");
        room.touched("h1", "src/a.rs");
        reject_impl(&room.db, "r1", &room.cwd, "src/a.rs", &[], None).unwrap();

        let row = room.db.review_baseline("r1", "src/a.rs").unwrap().unwrap();
        assert_eq!(row.content.as_deref(), Some("one\ntwo\nthree\n"));
        // So the next edit is pending again, against the same baseline.
        room.write("src/a.rs", "one\nTHREE\nthree\n");
        assert_eq!(room.pending().len(), 1);
    }

    // ── non-text ──────────────────────────────────────────────────

    #[test]
    fn a_binary_file_is_reported_as_blocked_and_still_clears_on_accept() {
        let room = Room::new();
        fs::write(room.path("logo.png"), [0x89, 0x50, 0x00, 0x01]).unwrap();
        room.touched("h1", "logo.png");

        let pending = room.pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].blocked, Some("binary"));
        assert!(pending[0].hunks.is_empty());

        accept_impl(&room.db, "r1", &room.cwd, Some("logo.png"), &[], None).unwrap();
        assert!(room.pending().is_empty(), "an accepted binary must clear");

        // A different image of the same length is pending again.
        fs::write(room.path("logo.png"), [0x89, 0x50, 0x00, 0x02]).unwrap();
        assert_eq!(room.pending().len(), 1);
    }

    #[test]
    fn hunk_operations_are_refused_on_a_file_with_no_line_diff() {
        let room = Room::new();
        fs::write(room.path("logo.png"), [0x00, 0x01]).unwrap();
        room.touched("h1", "logo.png");
        let fake = skein_review::diff_lines("a\n", "b\n");

        assert!(
            accept_impl(&room.db, "r1", &room.cwd, Some("logo.png"), &fake, None)
                .unwrap_err()
                .contains("no line diff")
        );
        assert!(
            reject_impl(&room.db, "r1", &room.cwd, "logo.png", &fake, None)
                .unwrap_err()
                .contains("no line diff")
        );
    }

    // ── persistence ───────────────────────────────────────────────

    #[test]
    fn the_review_survives_a_restart() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let db_path = dir.path().join("skein.db");
        let repo = git2::Repository::init(dir.path()).unwrap();
        fs::write(dir.path().join("a.rs"), "one\n").unwrap();
        commit(&repo, "init");

        let quoted =
            serde_json::to_string(&dir.path().join("a.rs").to_string_lossy().to_string()).unwrap();
        let payload = format!("{{\"tool\":\"Edit\",\"files\":[{quoted}]}}");

        {
            let db = Database::open(&db_path).unwrap();
            fs::write(dir.path().join("a.rs"), "ONE\n").unwrap();
            note_patch(&db, "r1", &cwd, "h1", &payload);
            assert_eq!(pending_impl(&db, "r1", &cwd).unwrap().len(), 1);
        }

        // Skein restarts. An empty tracker here would make the pending
        // change disappear — the user would see it as auto-applied.
        let db = Database::open(&db_path).unwrap();
        let pending = pending_impl(&db, "r1", &cwd).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].path, "a.rs");
        assert_eq!((pending[0].additions, pending[0].deletions), (1, 1));
    }

    // ── discovery (#221) ─────────────────────────────────────────

    #[test]
    fn discover_baselines_a_shell_write_in_a_non_git_room() {
        let room = Room::bare();
        room.write("notes.md", "hello\n");
        assert_eq!(room.discover(&["notes.md"]), 1);

        let pending = room.pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].change, "added");
        assert_eq!(
            pending[0].harness_id, "",
            "no chip — discovery has no attribution"
        );
    }

    #[test]
    fn discover_baselines_a_modified_tracked_file_from_head() {
        let room = Room::new();
        room.write("src/a.rs", "one\nTWO\nthree\n");
        assert_eq!(room.discover(&["src/a.rs"]), 1);

        let pending = room.pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].change, "modified");
        assert_eq!(pending[0].harness_id, "");
        let row = room.db.review_baseline("r1", "src/a.rs").unwrap().unwrap();
        assert_eq!(
            row.content.as_deref(),
            Some("one\ntwo\nthree\n"),
            "baseline is HEAD content, not what's on disk"
        );
    }

    #[test]
    fn discover_never_touches_an_existing_baseline() {
        let room = Room::new();
        room.write("src/a.rs", "one\nTWO\nthree\n");
        room.touched("h1", "src/a.rs");
        room.write("src/a.rs", "one\nTWO\nTHREE\n");

        assert_eq!(room.discover(&["src/a.rs"]), 0);

        let row = room.db.review_baseline("r1", "src/a.rs").unwrap().unwrap();
        assert_eq!(row.content.as_deref(), Some("one\ntwo\nthree\n"));
        assert_eq!(row.harness_id, "h1", "discovery must not steal the chip");
    }

    #[test]
    fn discover_skips_git_internals_ignored_directory_and_unchanged_paths() {
        let room = Room::new();
        room.write(".gitignore", "ignored.log\n");
        room.commit_all("add gitignore");
        room.write("ignored.log", "noise\n");

        // `.git/`, a gitignored file, and a directory (src already holds
        // a.rs from Room::new).
        assert_eq!(room.discover(&[".git/HEAD", "ignored.log", "src"]), 0);
        assert!(
            room.db
                .review_baseline("r1", "ignored.log")
                .unwrap()
                .is_none()
        );
        assert!(room.db.review_baseline("r1", "src").unwrap().is_none());

        // Outside the worktree entirely.
        assert_eq!(
            discover(
                &room.db,
                "r1",
                &room.cwd,
                vec![format!("{ABS}/elsewhere/x.rs")]
            ),
            0
        );

        // An unchanged tracked file — nothing pending yet.
        assert_eq!(room.discover(&["src/a.rs"]), 0);
        assert!(room.db.review_baseline("r1", "src/a.rs").unwrap().is_none());
    }

    #[test]
    fn discover_baselines_a_deleted_tracked_file() {
        let room = Room::new();
        fs::remove_file(room.path("src/a.rs")).unwrap();
        assert_eq!(room.discover(&["src/a.rs"]), 1);

        let pending = room.pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].change, "deleted");
    }

    #[test]
    fn discover_falls_back_to_catch_up_for_a_deleted_tracked_directory() {
        let room = Room::new();
        room.write("src/inner/b.rs", "b\n");
        room.write("src/inner/c.rs", "c\n");
        room.commit_all("add inner dir");
        fs::remove_dir_all(room.path("src/inner")).unwrap();

        // The watcher only ever reports the directory itself vanishing —
        // one OS event, not one per file inside it.
        let inserted = room.discover(&["src/inner"]);
        assert_eq!(
            inserted, 2,
            "both files must surface via the catch-up fallback"
        );

        let mut paths: Vec<String> = room.pending().into_iter().map(|f| f.path).collect();
        paths.sort();
        assert_eq!(paths, ["src/inner/b.rs", "src/inner/c.rs"]);
        assert!(
            room.db
                .review_baseline("r1", "src/inner")
                .unwrap()
                .is_none(),
            "the directory path itself is never baselined"
        );
    }

    #[test]
    fn discover_ignores_untracked_temp_file_churn_and_does_not_sweep() {
        // A never-tracked path that is gone by the time discovery runs
        // (an editor swap file, a lockfile, any create-then-delete temp
        // file) must not latch the catch-up fallback — only a
        // directory that existed at HEAD may. Observed through the
        // sweep's own effect: a dirty tracked file that was never
        // reported in the batch would only surface if a full
        // `git status` sweep ran.
        let room = Room::new();
        room.write("src/other.rs", "hello\n");
        room.commit_all("add other");
        room.write("src/other.rs", "HELLO\n"); // dirty, but not reported below

        room.write("tmp.x", "noise");
        fs::remove_file(room.path("tmp.x")).unwrap();

        assert_eq!(
            room.discover(&["tmp.x"]),
            0,
            "temp churn must not trigger a sweep"
        );
        assert!(
            room.db
                .review_baseline("r1", "src/other.rs")
                .unwrap()
                .is_none(),
            "a full sweep would have picked up the unreported dirty file"
        );
    }

    #[test]
    fn catch_up_discovers_dirty_and_untracked_files() {
        let room = Room::new();
        // The fixture's own sqlite file lives inside this same worktree
        // (real rooms never do — the db is in APP_DATA), so keep it out
        // of `git status` or it would show up as untracked noise.
        room.write(".gitignore", "skein.db*\n");
        room.commit_all("ignore fixture db");

        room.write("src/a.rs", "one\nTWO\nthree\n"); // dirty
        room.write("new.rs", "brand new\n"); // untracked

        assert_eq!(room.catch_up(), 2);
        let mut paths: Vec<String> = room.pending().into_iter().map(|f| f.path).collect();
        paths.sort();
        assert_eq!(paths, ["new.rs", "src/a.rs"]);
    }

    #[test]
    fn catch_up_in_a_non_git_room_does_nothing() {
        let room = Room::bare();
        room.write("notes.md", "hello\n");
        assert_eq!(room.catch_up(), 0);
        assert!(room.pending().is_empty(), "non-git rooms cannot catch up");
    }

    #[test]
    fn accepting_a_discovered_file_clears_it() {
        let room = Room::new();
        room.write("src/a.rs", "one\nTWO\nthree\n");
        room.discover(&["src/a.rs"]);
        let hash = room.pending()[0].content_hash.clone();

        accept_impl(
            &room.db,
            "r1",
            &room.cwd,
            Some("src/a.rs"),
            &[],
            Some(&hash),
        )
        .unwrap();
        assert!(room.pending().is_empty());
    }

    #[test]
    fn rejecting_a_discovered_created_file_deletes_it_in_a_non_git_room() {
        let room = Room::bare();
        room.write("notes.md", "unwanted\n");
        room.discover(&["notes.md"]);
        assert_eq!(room.pending()[0].change, "added");

        reject_impl(&room.db, "r1", &room.cwd, "notes.md", &[], None).unwrap();
        assert!(!room.path("notes.md").exists());
        assert!(room.pending().is_empty());
    }

    #[test]
    fn note_patch_after_discovery_attributes_the_row() {
        let room = Room::new();
        room.write("src/a.rs", "one\nTWO\nthree\n");
        room.discover(&["src/a.rs"]);
        assert_eq!(room.pending()[0].harness_id, "");

        room.touched("h1", "src/a.rs");
        assert_eq!(room.pending()[0].harness_id, "h1");

        let row = room.db.review_baseline("r1", "src/a.rs").unwrap().unwrap();
        assert_eq!(
            row.content.as_deref(),
            Some("one\ntwo\nthree\n"),
            "note_patch must attribute, not recapture"
        );
    }

    // ── insert_or_attribute (lost-insert race, #221) ────────────────

    #[test]
    fn insert_or_attribute_claims_a_row_a_concurrent_capture_already_won() {
        // Simulates the race the module docs describe: watcher
        // discovery's own insert lands first, with no attribution,
        // in the window between `note_patch`'s baseline read and its
        // own insert.
        let room = Room::new();
        assert!(
            room.db
                .insert_review_baseline_if_absent(
                    "r1",
                    "src/a.rs",
                    "text",
                    Some("one\ntwo\nthree\n"),
                    "",
                    100,
                )
                .unwrap()
        );

        insert_or_attribute(
            &room.db,
            "r1",
            "src/a.rs",
            "text",
            Some("a different snapshot"),
            "h1",
            200,
        )
        .unwrap();

        let row = room.db.review_baseline("r1", "src/a.rs").unwrap().unwrap();
        assert_eq!(row.harness_id, "h1", "the chip must not be lost");
        assert_eq!(
            row.content.as_deref(),
            Some("one\ntwo\nthree\n"),
            "content must never be re-captured, only attribution moves"
        );
    }
}
