//! Reading a review: the header and file list, and one file's diff.
//!
//! The two commands the pane leans on hardest, and the only place the
//! three scopes visibly differ. `pending` is #211's model — baseline to
//! disk, git-independent, and the only scope where accept and reject
//! mean anything. The other two are git ranges, and both come back in
//! the same hunk shape so the pane needs one renderer rather than three.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use skein_git::{FileDiff, Repo};
use skein_review::Placement;

use super::anchoring::{
    PlaceCtx, addressed_by_thread, apply_addressed, comments_by_thread, parse_anchor_lines,
    place_threads, to_thread_dto,
};
use super::dto::{
    AddressedDto, CommentDto, CommitDto, FileDetailDto, ReviewFileDto, ReviewScopeDto, ThreadDto,
};
use super::element::{apply_element, element_rows_by_thread};
use super::git::{
    Range, additions, deletions, file_hash, norm, resolve_range, scope_diffs, status_str,
    to_review_hunk,
};
use super::proposal::{Proposal, apply_proposals, proposals_by_thread};
use super::source_view::{
    SourceIndex, apply_source_counts, index_sources, mirror_threads, push_source_only_files,
    source_counts,
};
use super::{Scope, thread_scope};
use crate::db::{Database, ReviewElementAnchorRow, ReviewThreadRow};
use crate::review::{PendingFileDto, pending_impl, relative_key};

pub(crate) fn scope_impl(
    db: &Database,
    room_id: &str,
    cwd: &str,
    scope: Scope,
    commit_sha: Option<&str>,
) -> Result<ReviewScopeDto, String> {
    scope_impl_with(db, room_id, cwd, scope, commit_sha, None)
}

/// A git scope's resolved range and its diff, computed once so that a
/// caller wanting both the summary and per-file hunks (the agent API's
/// `get_diff`, #287) builds both from the very same diff rather than two
/// that a concurrent edit could make disagree.
pub(crate) struct ScopeDiff {
    range: Range,
    diffs: Vec<FileDiff>,
}

impl ScopeDiff {
    fn compute(
        db: &Database,
        room_id: &str,
        repo: &Repo,
        scope: Scope,
        commit_sha: Option<&str>,
        paths: &[String],
    ) -> Result<Self, String> {
        let range = resolve_range(db, room_id, repo)?;
        let path_refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let diffs = scope_diffs(repo, &range, scope, commit_sha, &path_refs)?;
        Ok(Self { range, diffs })
    }
}

/// The scope summary plus the diff it was built from, for a caller that
/// wants to check the summary (an unresolved base, a missing file) before
/// paying for a [`ScopeFiles`] snapshot. `paths` restricts that diff exactly
/// as [`ScopeFiles::load`] does, and so the summary's file list too. Hand
/// the returned [`ScopeDiff`] to [`scope_snapshot`] so both come from ONE
/// `scope_diffs` call (#287).
pub(crate) fn scope_summary(
    db: &Database,
    room_id: &str,
    cwd: &str,
    scope: Scope,
    commit_sha: Option<&str>,
    paths: &[String],
) -> Result<(ReviewScopeDto, Option<ScopeDiff>), String> {
    let pre = if scope == Scope::Pending {
        None
    } else if let Ok(repo) = Repo::open(Path::new(cwd)) {
        Some(ScopeDiff::compute(
            db, room_id, &repo, scope, commit_sha, paths,
        )?)
    } else {
        None
    };
    let summary = scope_impl_with(db, room_id, cwd, scope, commit_sha, pre.as_ref())?;
    Ok((summary, pre))
}

/// The [`ScopeFiles`] snapshot, reusing the diff [`scope_summary`] computed.
pub(crate) fn scope_snapshot(
    db: &Database,
    room_id: &str,
    cwd: &str,
    scope: Scope,
    commit_sha: Option<&str>,
    paths: &[String],
    pre: Option<&ScopeDiff>,
) -> Result<ScopeFiles, String> {
    ScopeFiles::load_with(db, room_id, cwd, scope, commit_sha, paths, pre)
}

/// Summary and snapshot in one go, both from one `scope_diffs` call.
#[cfg(test)]
pub(crate) fn scope_with_files(
    db: &Database,
    room_id: &str,
    cwd: &str,
    scope: Scope,
    commit_sha: Option<&str>,
    paths: &[String],
) -> Result<(ReviewScopeDto, ScopeFiles), String> {
    let (summary, pre) = scope_summary(db, room_id, cwd, scope, commit_sha, paths)?;
    let files = scope_snapshot(db, room_id, cwd, scope, commit_sha, paths, pre.as_ref())?;
    Ok((summary, files))
}

#[allow(clippy::too_many_lines)]
fn scope_impl_with(
    db: &Database,
    room_id: &str,
    cwd: &str,
    scope: Scope,
    commit_sha: Option<&str>,
    pre: Option<&ScopeDiff>,
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
    let sources = index_sources(&threads, &element_rows_by_thread(db, room_id)?);
    let source_counts = source_counts(&sources);
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
        if t.scope == thread_scope::COMMIT
            && let Some(sha) = t.commit_sha.as_deref()
        {
            *per_commit.entry(sha.to_owned()).or_default() += 1;
        }
    }

    let Ok(repo) = Repo::open(Path::new(cwd)) else {
        // A non-git room still has a review: #211's pending set is not
        // git-dependent, and saying "not a repo" beats an empty pane.
        let mut files = pending_files(&pending, &viewed, &per_file);
        push_element_only_files(
            &mut files,
            &threads,
            &per_file,
            &viewed,
            &pending_paths,
            |_| String::new(),
        );
        push_source_only_files(
            &mut files,
            &sources,
            &per_file,
            &viewed,
            &pending_paths,
            |_| String::new(),
        );
        apply_source_counts(&mut files, &source_counts);
        return Ok(ReviewScopeDto {
            is_repo: false,
            base_ref: None,
            base_resolved: false,
            base_sha: None,
            head_branch: None,
            head_sha: None,
            commits: Vec::new(),
            truncated: false,
            files,
            additions: pending.iter().map(|p| p.additions).sum(),
            deletions: pending.iter().map(|p| p.deletions).sum(),
            pending_count: pending.len(),
            unresolved_count,
            branches: Vec::new(),
            threads: review_threads,
            error: None,
        });
    };

    let range = match pre {
        Some(p) => p.range.clone(),
        None => resolve_range(db, room_id, &repo)?,
    };
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
    let mut files = if scope == Scope::Pending {
        pending_files(&pending, &viewed, &per_file)
    } else {
        let computed;
        let diffs = if let Some(p) = pre {
            &p.diffs
        } else {
            computed = scope_diffs(&repo, &range, scope, commit_sha, &[])?;
            &computed
        };
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
                    // `ReviewFileDto` has no `blocked` field (kept
                    // minimal — the list doesn't need to explain, only
                    // the detail view does), so a too-large file simply
                    // reads as binary here, same as before the cap.
                    binary: f.binary,
                    viewed: marked.is_some_and(|h| *h == hash),
                    changed_since_viewed: marked.is_some_and(|h| *h != hash),
                    content_hash: hash,
                    thread_count: threads,
                    unresolved_count: unresolved,
                    source_thread_count: 0,
                    source_unresolved_count: 0,
                    has_pending: pending_paths.contains(&path),
                    harness_id: harness_of.get(&path).cloned(),
                    path,
                }
            })
            .collect()
    };
    let hash_of = |p: &str| {
        if scope == Scope::Pending {
            String::new()
        } else {
            file_hash(cwd, p, scope, commit_sha, &repo)
        }
    };
    push_element_only_files(
        &mut files,
        &threads,
        &per_file,
        &viewed,
        &pending_paths,
        hash_of,
    );
    push_source_only_files(
        &mut files,
        &sources,
        &per_file,
        &viewed,
        &pending_paths,
        hash_of,
    );
    apply_source_counts(&mut files, &source_counts);

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

/// List every entry that carries unresolved element threads but has no
/// row yet (#434). Sign-off counts those threads, so the reviewer must
/// be able to find them even when the prototype itself never changed.
/// Such a row says `change: "unchanged"` with zero counts and no hunks;
/// `file_impl` already answers for it with its threads and an empty
/// diff. Resolved-only entries are not added, and file-scope threads are
/// deliberately untouched.
fn push_element_only_files(
    files: &mut Vec<ReviewFileDto>,
    threads: &[ReviewThreadRow],
    per_file: &HashMap<String, (usize, usize)>,
    viewed: &HashMap<String, String>,
    pending_paths: &[String],
    hash_of: impl Fn(&str) -> String,
) {
    let listed: HashSet<String> = files.iter().map(|f| f.path.clone()).collect();
    let wanted: std::collections::BTreeSet<String> = threads
        .iter()
        .filter(|t| t.scope == thread_scope::ELEMENT && t.resolved_ms.is_none())
        .filter_map(|t| t.file_path.as_deref().map(norm))
        .filter(|p| !listed.contains(p))
        .collect();
    for path in wanted {
        let (thread_count, unresolved_count) = per_file.get(&path).copied().unwrap_or((0, 0));
        let hash = hash_of(&path);
        let marked = viewed.get(&path);
        files.push(ReviewFileDto {
            name: path.rsplit('/').next().unwrap_or(&path).to_owned(),
            change: "unchanged",
            additions: 0,
            deletions: 0,
            binary: false,
            viewed: marked.is_some_and(|h| *h == hash),
            changed_since_viewed: marked.is_some_and(|h| *h != hash),
            content_hash: hash,
            thread_count,
            unresolved_count,
            source_thread_count: 0,
            source_unresolved_count: 0,
            has_pending: pending_paths.contains(&path),
            harness_id: None,
            path,
        });
    }
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
                source_thread_count: 0,
                source_unresolved_count: 0,
                has_pending: true,
                harness_id: (!p.harness_id.is_empty()).then(|| p.harness_id.clone()),
            }
        })
        .collect()
}

/// One scope's comments, threads, repo and diff, captured once for
/// whichever files a caller needs — the pane's single-file open asks
/// for one path, the agent API's batch reads (#171 slice (f)) ask for
/// several — so a many-file caller pays for one scope diff instead of
/// one per file. Everything a single file's detail needs beyond the
/// diff itself (comments, threads, the repo, the resolved range) is
/// scope-wide anyway, which is what makes [`file_impl`] itself now just
/// a one-path snapshot: [`ScopeFiles::load`] with a single-element
/// slice, then [`ScopeFiles::file`].
pub(crate) struct ScopeFiles {
    cwd: String,
    scope: Scope,
    commit_sha: Option<String>,
    comments: HashMap<String, Vec<CommentDto>>,
    /// Threads grouped by their (normalised) file path — the same
    /// filter `file_impl` used to run per call, done once here.
    threads_by_file: HashMap<String, Vec<ReviewThreadRow>>,
    addressed: HashMap<String, AddressedDto>,
    /// Anchor rows of the room's element threads (#434).
    elements: HashMap<String, ReviewElementAnchorRow>,
    proposals: HashMap<String, Proposal>,
    /// Element threads by the JSX source file they also appear in (#467).
    sources: SourceIndex,
    repo: Option<Repo>,
    /// `None` only for `Scope::Pending`, which has no git range at all.
    range: Option<Range>,
    /// The scope's diff, restricted to the paths this snapshot was
    /// built for and keyed the same way. Empty for `Scope::Pending`.
    diffs: HashMap<String, FileDiff>,
    /// `Scope::Pending`'s own file list, keyed by path. Empty for the
    /// two git scopes.
    pending: HashMap<String, PendingFileDto>,
    /// The paths this snapshot was restricted to, normalised the same
    /// way as `diffs`' keys. Empty means unfiltered — the diff was not
    /// pathspec-restricted at all, so every key it might contain is
    /// fair game. Non-empty means [`ScopeFiles::file`] must refuse any
    /// other key rather than answer "no diff" for a file it was simply
    /// never asked to load — that would be indistinguishable from a
    /// file with a genuinely empty diff, which D6 forbids.
    ///
    /// Only checked for the two git scopes. `Scope::Pending` loads its
    /// whole file list from `pending_impl` regardless of `paths` (that
    /// function takes no path filter at all), so every key is always
    /// answerable there.
    requested: HashSet<String>,
}

impl ScopeFiles {
    /// Build the snapshot. `paths` restricts the underlying diff the
    /// same way `file_impl`'s single path used to — non-empty and it is
    /// literal, not a glob; empty means unfiltered, the whole scope's
    /// diff. Either way, [`ScopeFiles::file`] only answers for a key
    /// that was actually loaded — see `requested`.
    pub(crate) fn load(
        db: &Database,
        room_id: &str,
        cwd: &str,
        scope: Scope,
        commit_sha: Option<&str>,
        paths: &[String],
    ) -> Result<Self, String> {
        Self::load_with(db, room_id, cwd, scope, commit_sha, paths, None)
    }

    /// [`ScopeFiles::load`], optionally reusing a diff already computed
    /// by [`scope_summary`] (handed over via [`scope_snapshot`]) instead
    /// of running `scope_diffs` again.
    fn load_with(
        db: &Database,
        room_id: &str,
        cwd: &str,
        scope: Scope,
        commit_sha: Option<&str>,
        paths: &[String],
        pre: Option<&ScopeDiff>,
    ) -> Result<Self, String> {
        let comments = comments_by_thread(db, room_id)?;
        let addressed = addressed_by_thread(db, room_id)?;
        let elements = element_rows_by_thread(db, room_id)?;
        let proposals = proposals_by_thread(db, room_id)?;
        let mut threads_by_file: HashMap<String, Vec<ReviewThreadRow>> = HashMap::new();
        let all_threads = db.review_threads_for_room(room_id)?;
        let sources = index_sources(&all_threads, &elements);
        for t in all_threads {
            if let Some(fp) = t.file_path.as_deref() {
                threads_by_file.entry(norm(fp)).or_default().push(t);
            }
        }
        let repo = Repo::open(Path::new(cwd)).ok();
        let requested: HashSet<String> = paths.iter().map(|p| norm(p)).collect();

        // Pending keeps #211's own diff: baseline → disk, which git
        // cannot see and which carries the accept/reject verbs.
        // `pending_impl` has no path filter, so `requested` is left
        // empty here regardless of `paths` — every key is answerable.
        if scope == Scope::Pending {
            let pending = pending_impl(db, room_id, cwd)?
                .into_iter()
                .map(|p| (p.path.clone(), p))
                .collect();
            return Ok(Self {
                cwd: cwd.to_owned(),
                scope,
                commit_sha: commit_sha.map(str::to_owned),
                comments,
                threads_by_file,
                addressed,
                elements,
                proposals,
                sources,
                repo,
                range: None,
                diffs: HashMap::new(),
                pending,
                requested: HashSet::new(),
            });
        }

        let repo_ref = repo.as_ref().ok_or("not a git repository")?;
        let (range, raw) = if let Some(p) = pre {
            (p.range.clone(), p.diffs.clone())
        } else {
            let p = ScopeDiff::compute(db, room_id, repo_ref, scope, commit_sha, paths)?;
            (p.range, p.diffs)
        };
        let diffs = raw.into_iter().map(|f| (norm(&f.path), f)).collect();

        Ok(Self {
            cwd: cwd.to_owned(),
            scope,
            commit_sha: commit_sha.map(str::to_owned),
            comments,
            threads_by_file,
            addressed,
            elements,
            proposals,
            sources,
            repo,
            range: Some(range),
            diffs,
            pending: HashMap::new(),
            requested,
        })
    }

    fn mirrors(&self, ctx: &PlaceCtx<'_>, key: &str) -> Vec<ThreadDto> {
        mirror_threads(
            &self.sources,
            key,
            ctx,
            &self.comments,
            &self.addressed,
            &self.elements,
            &self.proposals,
            Some(&self.cwd),
        )
    }

    /// One file's diff, plus every thread on it re-anchored to right
    /// now — the same shape and the same rules `file_impl` always had,
    /// just sliced out of the snapshot instead of recomputing it.
    pub(crate) fn file(&self, db: &Database, key: &str) -> Result<FileDetailDto, String> {
        let file_threads = self.threads_by_file.get(key).cloned().unwrap_or_default();

        if self.scope == Scope::Pending {
            let found = self.pending.get(key).cloned();
            let ctx = PlaceCtx::new(self.repo.as_ref(), &self.cwd, None, self.scope, None);
            let mut threads = place_threads(db, &ctx, key, file_threads, &self.comments);
            apply_addressed(&mut threads, &self.addressed);
            apply_element(&mut threads, &self.elements, Some(&self.cwd));
            apply_proposals(&mut threads, &self.proposals);
            threads.extend(self.mirrors(&ctx, key));
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
                    name: key.rsplit('/').next().unwrap_or(key).to_owned(),
                    change: "modified",
                    binary: false,
                    blocked: None,
                    content_hash: String::new(),
                    hunks: Vec::new(),
                    threads,
                    path: key.to_owned(),
                },
            });
        }

        // A key outside what this snapshot loaded must not fall into
        // the same "no diff" placeholder as a file that genuinely has
        // none — the two are indistinguishable from the DTO alone, and
        // D6 requires the difference to be visible rather than guessed
        // away. Empty `requested` means unfiltered: every key is fair
        // game.
        if !self.requested.is_empty() && !self.requested.contains(key) {
            return Err(format!("{key} was not loaded into this snapshot"));
        }

        let Some(repo_ref) = self.repo.as_ref() else {
            return Err("not a git repository".to_owned());
        };
        let Some(range) = self.range.as_ref() else {
            return Err("not a git repository".to_owned());
        };
        let found = self.diffs.get(key);

        let ctx = PlaceCtx::new(
            self.repo.as_ref(),
            &self.cwd,
            range.base_sha.as_deref(),
            self.scope,
            self.commit_sha.as_deref(),
        );
        let mut threads = place_threads(db, &ctx, key, file_threads, &self.comments);
        apply_addressed(&mut threads, &self.addressed);
        apply_element(&mut threads, &self.elements, Some(&self.cwd));
        apply_proposals(&mut threads, &self.proposals);
        threads.extend(self.mirrors(&ctx, key));

        let hash = file_hash(
            &self.cwd,
            key,
            self.scope,
            self.commit_sha.as_deref(),
            repo_ref,
        );
        Ok(match found {
            Some(f) => FileDetailDto {
                name: key.rsplit('/').next().unwrap_or(key).to_owned(),
                change: status_str(f.kind),
                // A too-large file is not really binary — just never
                // read — so it gets its own `blocked` reason instead of
                // `binary`'s.
                binary: f.binary && !f.too_large,
                blocked: if f.too_large {
                    Some("toolarge")
                } else if f.binary {
                    Some("binary")
                } else {
                    None
                },
                content_hash: hash,
                hunks: f.hunks.iter().map(to_review_hunk).collect(),
                threads,
                path: key.to_owned(),
            },
            // A file with threads but no diff in this scope — reviewed
            // and accepted, or commented in another scope. Its threads
            // still render; dropping them would be the silent loss D6
            // forbids.
            None => FileDetailDto {
                name: key.rsplit('/').next().unwrap_or(key).to_owned(),
                change: "modified",
                binary: false,
                blocked: None,
                content_hash: hash,
                hunks: Vec::new(),
                threads,
                path: key.to_owned(),
            },
        })
    }
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
    let snapshot = ScopeFiles::load(
        db,
        room_id,
        cwd,
        scope,
        commit_sha,
        std::slice::from_ref(&key),
    )?;
    snapshot.file(db, &key)
}

#[cfg(test)]
mod tests;
