//! Reading a review: the header and file list, and one file's diff.
//!
//! The two commands the pane leans on hardest, and the only place the
//! three scopes visibly differ. `pending` is #211's model — baseline to
//! disk, git-independent, and the only scope where accept and reject
//! mean anything. The other two are git ranges, and both come back in
//! the same hunk shape so the pane needs one renderer rather than three.

use std::collections::HashMap;
use std::path::Path;

use skein_git::Repo;
use skein_review::Placement;

use super::anchoring::{
    PlaceCtx, addressed_by_thread, apply_addressed, comments_by_thread, parse_anchor_lines,
    place_threads, to_thread_dto,
};
use super::dto::{CommitDto, FileDetailDto, ReviewFileDto, ReviewScopeDto, ThreadDto};
use super::git::{
    additions, deletions, file_hash, norm, resolve_range, scope_diffs, status_str, to_review_hunk,
};
use super::{Scope, thread_scope};
use crate::db::{Database, ReviewThreadRow};
use crate::review::{pending_impl, relative_key};

#[allow(clippy::too_many_lines)]
pub(crate) fn scope_impl(
    db: &Database,
    room_id: &str,
    cwd: &str,
    scope: Scope,
    commit_sha: Option<&str>,
) -> Result<ReviewScopeDto, String> {
    let threads = db.review_threads_for_room(room_id)?;
    let comments = comments_by_thread(db, room_id)?;
    let viewed: HashMap<String, String> = db.review_viewed_for_room(room_id)?.into_iter().collect();
    let pending = pending_impl(db, room_id, cwd)?;
    let pending_paths: Vec<String> = pending.iter().map(|p| p.path.clone()).collect();
    // Attribution comes from the baselines rather than from the pending
    // set, so a file the agent wrote and then committed keeps its chip
    // instead of losing it at the moment it stops being pending.
    let harness_of: HashMap<String, String> = db
        .review_baselines_for_room(room_id)?
        .into_iter()
        .filter(|b| !b.harness_id.is_empty())
        .map(|b| (b.path, b.harness_id))
        .collect();

    let unresolved_count = threads.iter().filter(|t| t.resolved_ms.is_none()).count();
    let addressed = addressed_by_thread(db, room_id)?;
    let mut review_threads: Vec<ThreadDto> = threads
        .iter()
        .filter(|t| t.scope == thread_scope::REVIEW)
        .cloned()
        .map(|t| {
            let lines = parse_anchor_lines(t.anchor_lines.as_deref());
            to_thread_dto(t, lines, Placement::Outdated, &comments)
        })
        .collect();
    apply_addressed(&mut review_threads, &addressed);

    // Per-file thread tallies, keyed the same way the file list is.
    let mut per_file: HashMap<String, (usize, usize)> = HashMap::new();
    let mut per_commit: HashMap<String, usize> = HashMap::new();
    for t in &threads {
        if let Some(path) = t.file_path.as_deref() {
            let entry = per_file.entry(norm(path)).or_default();
            entry.0 += 1;
            if t.resolved_ms.is_none() {
                entry.1 += 1;
            }
        }
        if t.scope == thread_scope::COMMIT {
            if let Some(sha) = t.commit_sha.as_deref() {
                *per_commit.entry(sha.to_owned()).or_default() += 1;
            }
        }
    }

    let Ok(repo) = Repo::open(Path::new(cwd)) else {
        // A non-git room still has a review: #211's pending set is not
        // git-dependent, and saying "not a repo" beats an empty pane.
        return Ok(ReviewScopeDto {
            is_repo: false,
            base_ref: None,
            base_resolved: false,
            base_sha: None,
            head_branch: None,
            head_sha: None,
            commits: Vec::new(),
            truncated: false,
            files: pending_files(&pending, &viewed, &per_file),
            additions: pending.iter().map(|p| p.additions).sum(),
            deletions: pending.iter().map(|p| p.deletions).sum(),
            pending_count: pending.len(),
            unresolved_count,
            branches: Vec::new(),
            threads: review_threads,
            error: None,
        });
    };

    let range = resolve_range(db, room_id, &repo)?;
    let branches: Vec<String> = repo
        .branches()
        .map(|bs| bs.into_iter().map(|b| b.name).collect())
        .unwrap_or_default();

    let commits = if range.head_sha.is_some() {
        repo.commits_between(range.base_sha.as_deref(), "HEAD")
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let truncated = commits.len() >= skein_git::MAX_RANGE_COMMITS;
    let commit_dtos: Vec<CommitDto> = commits
        .into_iter()
        .map(|c| CommitDto {
            is_merge: c.parents.len() > 1,
            thread_count: per_commit.get(&c.sha).copied().unwrap_or(0),
            sha: c.sha,
            short_sha: c.short_sha,
            summary: c.summary,
            body: c.body,
            author_name: c.author_name,
            time_ms: c.time_ms,
        })
        .collect();

    // Pending is its own file list and needs no git diff.
    let files = if scope == Scope::Pending {
        pending_files(&pending, &viewed, &per_file)
    } else {
        let diffs = scope_diffs(&repo, &range, scope, commit_sha)?;
        diffs
            .iter()
            .map(|f| {
                let path = norm(&f.path);
                let hash = file_hash(cwd, &path, scope, commit_sha, &repo);
                let (threads, unresolved) = per_file.get(&path).copied().unwrap_or((0, 0));
                let marked = viewed.get(&path);
                ReviewFileDto {
                    name: path.rsplit('/').next().unwrap_or(&path).to_owned(),
                    change: status_str(f.kind),
                    additions: additions(f),
                    deletions: deletions(f),
                    binary: f.binary,
                    viewed: marked.is_some_and(|h| *h == hash),
                    changed_since_viewed: marked.is_some_and(|h| *h != hash),
                    content_hash: hash,
                    thread_count: threads,
                    unresolved_count: unresolved,
                    has_pending: pending_paths.contains(&path),
                    harness_id: harness_of.get(&path).cloned(),
                    path,
                }
            })
            .collect()
    };

    let error = if range.base_ref.is_some() && !range.base_resolved {
        Some(format!(
            "base branch {} no longer exists — pick another",
            range.base_ref.clone().unwrap_or_default()
        ))
    } else {
        None
    };

    Ok(ReviewScopeDto {
        is_repo: true,
        base_ref: range.base_ref,
        base_resolved: range.base_resolved,
        base_sha: range.base_sha,
        head_branch: range.head_branch,
        head_sha: range.head_sha,
        commits: commit_dtos,
        truncated,
        additions: files.iter().map(|f| f.additions).sum(),
        deletions: files.iter().map(|f| f.deletions).sum(),
        files,
        pending_count: pending.len(),
        unresolved_count,
        branches,
        threads: review_threads,
        error,
    })
}

/// The Pending scope's file list, straight from #211's model.
fn pending_files(
    pending: &[crate::review::PendingFileDto],
    viewed: &HashMap<String, String>,
    per_file: &HashMap<String, (usize, usize)>,
) -> Vec<ReviewFileDto> {
    pending
        .iter()
        .map(|p| {
            let (threads, unresolved) = per_file.get(&p.path).copied().unwrap_or((0, 0));
            let marked = viewed.get(&p.path);
            ReviewFileDto {
                path: p.path.clone(),
                name: p.name.clone(),
                change: p.change,
                additions: p.additions,
                deletions: p.deletions,
                binary: p.blocked == Some("binary"),
                viewed: marked.is_some_and(|h| *h == p.content_hash),
                changed_since_viewed: marked.is_some_and(|h| *h != p.content_hash),
                content_hash: p.content_hash.clone(),
                thread_count: threads,
                unresolved_count: unresolved,
                has_pending: true,
                harness_id: (!p.harness_id.is_empty()).then(|| p.harness_id.clone()),
            }
        })
        .collect()
}

pub(crate) fn file_impl(
    db: &Database,
    room_id: &str,
    cwd: &str,
    path: &str,
    scope: Scope,
    commit_sha: Option<&str>,
) -> Result<FileDetailDto, String> {
    let key = relative_key(cwd, path).unwrap_or_else(|| norm(path));
    let comments = comments_by_thread(db, room_id)?;
    let file_threads: Vec<ReviewThreadRow> = db
        .review_threads_for_room(room_id)?
        .into_iter()
        .filter(|t| t.file_path.as_deref().map(norm).as_deref() == Some(key.as_str()))
        .collect();

    let repo = Repo::open(Path::new(cwd)).ok();
    let addressed = addressed_by_thread(db, room_id)?;

    // Pending keeps #211's own diff: baseline → disk, which git cannot
    // see and which carries the accept/reject verbs.
    if scope == Scope::Pending {
        let pending = pending_impl(db, room_id, cwd)?;
        let found = pending.into_iter().find(|p| p.path == key);
        let ctx = PlaceCtx::new(repo.as_ref(), cwd, None, scope, None);
        let mut threads = place_threads(db, &ctx, &key, file_threads, &comments);
        apply_addressed(&mut threads, &addressed);
        return Ok(match found {
            Some(p) => FileDetailDto {
                name: p.name,
                change: p.change,
                binary: p.blocked == Some("binary"),
                blocked: p.blocked,
                content_hash: p.content_hash,
                hunks: p.hunks,
                threads,
                path: p.path,
            },
            None => FileDetailDto {
                name: key.rsplit('/').next().unwrap_or(&key).to_owned(),
                change: "modified",
                binary: false,
                blocked: None,
                content_hash: String::new(),
                hunks: Vec::new(),
                threads,
                path: key,
            },
        });
    }

    let repo_ref = repo.as_ref().ok_or("not a git repository")?;
    let range = resolve_range(db, room_id, repo_ref)?;
    let diffs = scope_diffs(repo_ref, &range, scope, commit_sha)?;
    let found = diffs.iter().find(|f| norm(&f.path) == key);

    let ctx = PlaceCtx::new(
        repo.as_ref(),
        cwd,
        range.base_sha.as_deref(),
        scope,
        commit_sha,
    );
    let mut threads = place_threads(db, &ctx, &key, file_threads, &comments);
    apply_addressed(&mut threads, &addressed);

    let hash = file_hash(cwd, &key, scope, commit_sha, repo_ref);
    Ok(match found {
        Some(f) => FileDetailDto {
            name: key.rsplit('/').next().unwrap_or(&key).to_owned(),
            change: status_str(f.kind),
            binary: f.binary,
            blocked: if f.binary { Some("binary") } else { None },
            content_hash: hash,
            hunks: f.hunks.iter().map(to_review_hunk).collect(),
            threads,
            path: key,
        },
        // A file with threads but no diff in this scope — reviewed and
        // accepted, or commented in another scope. Its threads still
        // render; dropping them would be the silent loss D6 forbids.
        None => FileDetailDto {
            name: key.rsplit('/').next().unwrap_or(&key).to_owned(),
            change: "modified",
            binary: false,
            blocked: None,
            content_hash: hash,
            hunks: Vec::new(),
            threads,
            path: key,
        },
    })
}
