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
use super::git::{
    Range, additions, deletions, file_hash, norm, resolve_range, scope_diffs, status_str,
    to_review_hunk,
};
use super::{Scope, thread_scope};
use crate::db::{Database, ReviewThreadRow};
use crate::review::{PendingFileDto, pending_impl, relative_key};

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
        let diffs = scope_diffs(&repo, &range, scope, commit_sha, &[])?;
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
        let comments = comments_by_thread(db, room_id)?;
        let addressed = addressed_by_thread(db, room_id)?;
        let mut threads_by_file: HashMap<String, Vec<ReviewThreadRow>> = HashMap::new();
        for t in db.review_threads_for_room(room_id)? {
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
                repo,
                range: None,
                diffs: HashMap::new(),
                pending,
                requested: HashSet::new(),
            });
        }

        let repo_ref = repo.as_ref().ok_or("not a git repository")?;
        let range = resolve_range(db, room_id, repo_ref)?;
        let path_refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let diffs = scope_diffs(repo_ref, &range, scope, commit_sha, &path_refs)?
            .into_iter()
            .map(|f| (norm(&f.path), f))
            .collect();

        Ok(Self {
            cwd: cwd.to_owned(),
            scope,
            commit_sha: commit_sha.map(str::to_owned),
            comments,
            threads_by_file,
            addressed,
            repo,
            range: Some(range),
            diffs,
            pending: HashMap::new(),
            requested,
        })
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
mod tests {
    use super::*;

    #[test]
    fn file_impl_maps_toolarge_and_binary_to_distinct_blocked_reasons() {
        // Pins the #171 slice (c) DTO mapping: a too-large file and a
        // real binary both come back from skein-git as `binary: true`
        // (`max_size` makes an oversized file look binary before any
        // content is read), and `file_impl` is the one place that tells
        // them apart for the pane — `blocked: Some("toolarge")` vs
        // `Some("binary")`. A small text file gets neither and keeps
        // its hunks.
        let (db, tmp) = tests_support::repo_with_commit();
        let cwd = tmp.path().to_str().unwrap();

        let big = "x".repeat(usize::try_from(skein_git::MAX_DIFF_FILE_BYTES).unwrap() + 1);
        std::fs::write(tmp.path().join("big.txt"), &big).unwrap();
        std::fs::write(tmp.path().join("bin.dat"), [0u8, 1, 2, 0, 3, 4]).unwrap();
        std::fs::write(tmp.path().join("small.txt"), "hello\n").unwrap();

        let big_detail = file_impl(&db, "r1", cwd, "big.txt", Scope::Branch, None).unwrap();
        assert_eq!(big_detail.blocked, Some("toolarge"));
        assert!(!big_detail.binary, "too-large is not the same as binary");
        assert!(big_detail.hunks.is_empty());

        let binary_detail = file_impl(&db, "r1", cwd, "bin.dat", Scope::Branch, None).unwrap();
        assert_eq!(binary_detail.blocked, Some("binary"));
        assert!(binary_detail.binary);
        assert!(binary_detail.hunks.is_empty());

        let small_detail = file_impl(&db, "r1", cwd, "small.txt", Scope::Branch, None).unwrap();
        assert_eq!(small_detail.blocked, None);
        assert!(!small_detail.binary);
        assert!(!small_detail.hunks.is_empty());
    }

    #[test]
    fn scope_files_snapshot_matches_file_impl_for_every_loaded_file() {
        // #171 slice (f): a `ScopeFiles` snapshot built over several
        // paths must answer each of them exactly as `file_impl`'s own
        // one-path call would — byte for byte, including placement and
        // the "not in the diff" branch — which is what lets a batch
        // caller reuse it instead of paying for one diff per file.
        let (db, tmp) = tests_support::repo_with_base_and_uncommitted_edits();
        let cwd = tmp.path().to_str().unwrap();
        db.set_review_base_ref("r1", "base", 1).unwrap();

        // A thread on the one file that has both a change and a comment
        // — exercises placement, not just the diff lookup.
        db.insert_review_thread(&ReviewThreadRow {
            id: "t1".into(),
            room_id: "r1".into(),
            scope: thread_scope::LINE.into(),
            file_path: Some("b.txt".into()),
            commit_sha: None,
            side: Some("new".into()),
            line_start: Some(1),
            line_end: Some(1),
            anchor_hash: None,
            anchor_lines: Some(serde_json::to_string(&["one"]).unwrap()),
            resolved_ms: None,
            created_ms: 1,
            updated_ms: 1,
        })
        .unwrap();

        // All three loaded — including c.txt, which is on disk
        // unchanged from the base and has no diff entry either way, so
        // it exercises the "not in the diff" branch while still being
        // a key this snapshot was actually asked to cover.
        let paths = vec!["a.txt".to_owned(), "b.txt".to_owned(), "c.txt".to_owned()];
        let snapshot = ScopeFiles::load(&db, "r1", cwd, Scope::Branch, None, &paths).unwrap();

        for path in ["a.txt", "b.txt", "c.txt"] {
            let want = file_impl(&db, "r1", cwd, path, Scope::Branch, None).unwrap();
            let got = snapshot.file(&db, path).unwrap();
            assert_eq!(
                serde_json::to_value(&want).unwrap(),
                serde_json::to_value(&got).unwrap(),
                "{path} diverged between file_impl and the ScopeFiles snapshot"
            );
        }
    }

    #[test]
    fn scope_files_snapshot_refuses_a_key_it_was_never_asked_to_load() {
        // The D6 footgun: a file excluded from `paths` must not answer
        // "no diff" — that is indistinguishable from a file that
        // genuinely has none. b.txt has a real, non-empty diff here;
        // excluding it from the snapshot must make `.file` refuse it
        // outright rather than silently reporting it as unchanged.
        let (db, tmp) = tests_support::repo_with_base_and_uncommitted_edits();
        let cwd = tmp.path().to_str().unwrap();
        db.set_review_base_ref("r1", "base", 1).unwrap();

        let paths = vec!["a.txt".to_owned()];
        let snapshot = ScopeFiles::load(&db, "r1", cwd, Scope::Branch, None, &paths).unwrap();

        assert!(
            snapshot.file(&db, "b.txt").is_err(),
            "a key outside the requested set must be refused, not answered"
        );

        let real = file_impl(&db, "r1", cwd, "b.txt", Scope::Branch, None).unwrap();
        assert!(
            !real.hunks.is_empty(),
            "b.txt does have a real diff — file_impl still sees it"
        );
    }

    #[test]
    fn an_unfiltered_snapshot_serves_any_changed_file_like_file_impl() {
        // An empty `paths` means "the whole scope's diff", so every
        // changed file is fair game with no exclusion at all.
        let (db, tmp) = tests_support::repo_with_base_and_uncommitted_edits();
        let cwd = tmp.path().to_str().unwrap();
        db.set_review_base_ref("r1", "base", 1).unwrap();

        let snapshot = ScopeFiles::load(&db, "r1", cwd, Scope::Branch, None, &[]).unwrap();

        for path in ["a.txt", "b.txt", "c.txt"] {
            let want = file_impl(&db, "r1", cwd, path, Scope::Branch, None).unwrap();
            let got = snapshot.file(&db, path).unwrap();
            assert_eq!(
                serde_json::to_value(&want).unwrap(),
                serde_json::to_value(&got).unwrap(),
                "{path} diverged between file_impl and the unfiltered snapshot"
            );
        }
    }

    /// Fixtures. Kept beside the tests rather than in a shared helper —
    /// see `signoff.rs`'s copy of the same note.
    mod tests_support {
        use std::fs;
        use std::path::Path;

        use tempfile::TempDir;

        use crate::db::Database;

        /// A repository with one commit, and a database beside it.
        ///
        /// **git2, never a spawned `git`.** The pre-commit hook runs
        /// these tests with `GIT_DIR` exported, and a spawned git
        /// inherits it — see `signoff.rs`'s `tests_support` for the
        /// full story of why that is dangerous.
        pub fn repo_with_commit() -> (Database, TempDir) {
            let tmp = TempDir::new().unwrap();
            git2::Repository::init(tmp.path()).unwrap();
            commit(tmp.path(), &[("a.txt", "one\n")], "init");
            let db = Database::open(&tmp.path().join("skein.db")).unwrap();
            (db, tmp)
        }

        /// A repository with a `base` branch holding three files, and
        /// uncommitted edits to two of them — the third stays byte for
        /// byte what `base` committed, so it has no diff at all.
        pub fn repo_with_base_and_uncommitted_edits() -> (Database, TempDir) {
            let tmp = TempDir::new().unwrap();
            commit(
                tmp.path(),
                &[("a.txt", "one\n"), ("b.txt", "one\n"), ("c.txt", "same\n")],
                "base",
            );
            let repo = git2::Repository::open(tmp.path()).unwrap();
            let head = repo.head().unwrap().peel_to_commit().unwrap();
            repo.branch("base", &head, false).unwrap();

            fs::write(tmp.path().join("a.txt"), "one\ntwo\n").unwrap();
            fs::write(tmp.path().join("b.txt"), "one\nchanged\n").unwrap();

            let db = Database::open(&tmp.path().join("skein.db")).unwrap();
            (db, tmp)
        }

        fn commit(cwd: &Path, files: &[(&str, &str)], msg: &str) {
            for (name, body) in files {
                fs::write(cwd.join(name), body).unwrap();
            }
            let repo = git2::Repository::open(cwd)
                .or_else(|_| git2::Repository::init(cwd))
                .unwrap();
            let mut index = repo.index().unwrap();
            for (name, _) in files {
                index.add_path(Path::new(name)).unwrap();
            }
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
    }
}
