//! Landing a room's branch — the two terminal actions (#214, epic #52
//! D9).
//!
//! A room is a task on its own branch in its own worktree. When the
//! review cycle is done the branch has to leave, and there are exactly
//! two ways it does:
//!
//! * **merge to base** — a solo project where nobody else needs to
//!   look; and
//! * **push and open a pull request** — collaborators review it on
//!   GitHub.
//!
//! Skein-local review comments stay local in both. The in-room review
//! was the pre-flight; replaying it into a PR body is noise for people
//! who were not part of it, and there is deliberately no code here that
//! can put a comment into a PR.
//!
//! # Look before you leap
//!
//! [`preflight`] answers everything the confirmation dialog needs to
//! say — what will move, what will *not* (uncommitted work is not part
//! of a merge), and every reason the action cannot run. Both actions
//! then re-run it and refuse on a blocker rather than trusting what the
//! UI was showing a minute ago.
//!
//! # Merging without a checkout
//!
//! The room's worktree has the room's branch checked out, so the base
//! branch is somewhere else — in the main checkout, in another
//! worktree, or nowhere at all. That gives three cases, and only two
//! of them can be honoured:
//!
//! * base is checked out somewhere and that worktree is clean → run the
//!   merge there, which is what the user would have done by hand;
//! * base is checked out nowhere and the merge is a fast-forward → move
//!   the ref, which is exactly what the merge would have produced;
//! * anything else → refuse, and say which. Merging into a dirty
//!   worktree the user cannot see, or synthesising a merge commit for a
//!   branch nobody has checked out, are both ways of being clever with
//!   someone else's repository.
//!
//! A conflicted merge is aborted before it is reported. Conflict
//! resolution is out of scope for the epic, and leaving a foreign
//! worktree mid-merge would be a trap the user did not set — the error
//! names the files instead.

use std::path::Path;

use crate::cli::{self, Output};
use crate::{GitError, Repo, Result};

/// How many entries the preflight will list before it stops counting
/// them out. The dialog shows a sample and a total; a room with 400
/// dirty files needs the number, not the names.
const SAMPLE: usize = 12;

/// How many commit summaries feed the suggested PR body.
const MAX_COMMITS: usize = 100;

/// Where a merge into the base branch would actually run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeTarget {
    /// The base branch is checked out here, so the merge runs here.
    /// `dirty` is that worktree's uncommitted-file count — a merge into
    /// a worktree the user is mid-edit in is refused.
    Worktree { path: String, dirty: usize },
    /// The base branch is checked out nowhere, so a fast-forward can
    /// move the ref and nothing else can be done at all.
    Unchecked,
    /// There is no local base branch to merge into.
    None,
}

/// Everything the confirmation needs to say, gathered before anything
/// moves.
///
/// A flat bag of facts rather than a state machine, on purpose: the
/// dialog renders all of them at once, and the two `*_blockers` lists
/// are the only derived verdicts anything acts on.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct LandPreflight {
    pub is_repo: bool,
    /// The room's branch. `None` on a detached HEAD, which neither
    /// action can work with.
    pub branch: Option<String>,
    pub base: Option<String>,
    /// The local branch a merge would actually target. Equal to `base`
    /// when that is already a local branch; the branch a
    /// remote-tracking `base` follows when it is not; `None` when
    /// neither exists, which is the one case a merge cannot happen.
    pub merge_base: Option<String>,
    /// Whether the ref the counts are against (`merge_base` if there is
    /// one, else `base`) resolves to a commit.
    pub base_resolved: bool,
    /// Whether `base` itself is a local branch. Only interesting
    /// alongside `merge_base`, to explain a substitution.
    pub base_is_local: bool,
    pub head_sha: Option<String>,
    /// Where `merge_base ?? base` points — what `ahead`, `behind` and
    /// `commits` are all measured from.
    pub base_sha: Option<String>,
    /// Commits on the branch that base does not have — what lands.
    pub ahead: usize,
    /// Commits on base the branch does not have.
    pub behind: usize,
    /// Uncommitted paths in the *room's* worktree. Not a blocker: a
    /// merge takes commits, so these simply do not travel, and the
    /// dialog says so rather than the action guessing.
    pub dirty: Vec<String>,
    pub dirty_total: usize,
    pub merge_target: MergeTarget,
    pub can_fast_forward: bool,
    pub remote: Option<String>,
    pub upstream: Option<String>,
    /// The base as a branch name on the remote — `origin/main` becomes
    /// `main`, which is what `gh --base` wants.
    pub pr_base: Option<String>,
    /// `gh --version`'s first line, or `None` when it is not installed.
    pub gh_version: Option<String>,
    /// Commit summaries, oldest first, capped at [`MAX_COMMITS`].
    pub commits: Vec<String>,
    pub suggested_title: String,
    pub suggested_body: String,
    /// Reasons merge-to-base cannot run. Empty means it can.
    pub merge_blockers: Vec<String>,
    /// Reasons push-and-open-PR cannot run. Empty means it can.
    pub pr_blockers: Vec<String>,
}

/// What a merge did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeKind {
    /// Base already contained the branch; nothing moved.
    AlreadyUpToDate,
    /// Base moved to the branch tip. No merge commit exists.
    FastForwarded,
    /// A merge commit was created.
    Merged,
}

#[derive(Debug, Clone)]
pub struct MergeOutcome {
    pub kind: MergeKind,
    pub base: String,
    pub branch: String,
    /// Where base points now.
    pub base_sha: Option<String>,
    /// Where the merge ran, when it ran in a worktree.
    pub worktree: Option<String>,
    /// git's own account of it, for the result line.
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct PushOutcome {
    pub remote: String,
    pub branch: String,
    /// git push reports on stderr; this is that, verbatim.
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct PrOutcome {
    pub url: String,
    /// `false` when a pull request for this branch already existed —
    /// the push still happened, and the existing PR is the answer.
    pub created: bool,
    /// Whether the body read back from GitHub matches what was sent.
    /// `gh` has a long history of eating bodies passed the wrong way;
    /// a silent mismatch is the failure this exists to catch.
    pub body_verified: bool,
}

// ── preflight ─────────────────────────────────────────────────────

/// Gather the facts. Reads only; nothing here changes the repository.
///
/// `base` overrides the guess — the review pane already lets the user
/// pick one per room, and landing has to agree with what they reviewed
/// against.
pub fn preflight(cwd: &Path, base: Option<&str>) -> Result<LandPreflight> {
    let Ok(repo) = Repo::open(cwd) else {
        return Ok(LandPreflight {
            is_repo: false,
            merge_blockers: vec!["this room's folder is not a git repository".to_owned()],
            pr_blockers: vec!["this room's folder is not a git repository".to_owned()],
            ..empty()
        });
    };

    let branch = repo.head_branch();
    let base = base
        .map(str::to_owned)
        .or_else(|| repo.default_base_branch());
    let head_sha = repo.resolve_commit("HEAD")?;
    let base_is_local = base.as_deref().is_some_and(|b| is_local_branch(cwd, b));

    // The review's base is very often a remote-tracking ref — the pane
    // guesses `origin/HEAD` first, so `origin/main` is the ordinary
    // case, and nothing can merge into one of those. The local branch
    // it tracks is what the user means, and substituting it here is
    // safe *because the dialog names it*: the button reads "merge into
    // main", not "merge into the base".
    let merge_base = base
        .as_deref()
        .and_then(|b| local_equivalent(cwd, b, base_is_local));

    // Everything counted below is counted against what a landing would
    // actually target, which is that local branch when there is one.
    let land_ref = merge_base.as_deref().or(base.as_deref());
    let base_sha = match land_ref {
        Some(b) => repo.resolve_commit(b)?,
        None => None,
    };
    let base_resolved = base_sha.is_some();

    let (behind, ahead) = match (&base_sha, &head_sha) {
        (Some(b), Some(h)) => ahead_behind(cwd, b, h)?,
        _ => (0, 0),
    };

    let (dirty, dirty_total) = dirty_paths(cwd)?;

    let worktrees = worktrees(cwd)?;
    let merge_target = match &merge_base {
        Some(b) => match worktrees.iter().find(|w| w.branch.as_deref() == Some(b)) {
            Some(w) => MergeTarget::Worktree {
                path: w.path.clone(),
                dirty: dirty_paths(Path::new(&w.path)).map_or(0, |(_, n)| n),
            },
            None => MergeTarget::Unchecked,
        },
        None => MergeTarget::None,
    };

    // A fast-forward is possible exactly when base is an ancestor of
    // HEAD — which is what `behind == 0` says, given both resolved.
    let can_fast_forward = base_resolved && head_sha.is_some() && behind == 0 && ahead > 0;

    let remote = branch.as_deref().and_then(|b| remote_for(cwd, b));
    let upstream = upstream_of(cwd);
    let pr_base = base.as_deref().map(|b| strip_remote(cwd, b));
    let gh_version = gh_version(cwd);

    let commits = match (&base_sha, &head_sha) {
        (Some(b), Some(_)) => commit_summaries(cwd, b)?,
        _ => Vec::new(),
    };
    let (suggested_title, suggested_body) = suggest_pr(cwd, branch.as_deref(), &commits);

    let mut pre = LandPreflight {
        is_repo: true,
        branch,
        base,
        merge_base,
        base_resolved,
        base_is_local,
        head_sha,
        base_sha,
        ahead,
        behind,
        dirty,
        dirty_total,
        merge_target,
        can_fast_forward,
        remote,
        upstream,
        pr_base,
        gh_version,
        commits,
        suggested_title,
        suggested_body,
        merge_blockers: Vec::new(),
        pr_blockers: Vec::new(),
    };
    pre.merge_blockers = merge_blockers(&pre);
    pre.pr_blockers = pr_blockers(&pre);
    Ok(pre)
}

fn merge_blockers(p: &LandPreflight) -> Vec<String> {
    let mut out = Vec::new();
    let Some(branch) = &p.branch else {
        out.push("this room is on a detached HEAD, so there is no branch to merge".to_owned());
        return out;
    };
    let Some(base) = &p.base else {
        out.push("no base branch — pick one in the review pane's base picker".to_owned());
        return out;
    };
    if !p.base_resolved {
        out.push(format!("the base branch {base} does not exist"));
        return out;
    }
    let Some(target) = &p.merge_base else {
        out.push(format!(
            "{base} is a remote-tracking branch and there is no local {} to merge into — pick a local branch as the base",
            base.rsplit('/').next().unwrap_or(base)
        ));
        return out;
    };
    if target == branch {
        out.push(format!("this room is already on {target}"));
        return out;
    }
    if p.ahead == 0 {
        // Sole blocker on purpose: with nothing to land, *how* the
        // merge would have run is not a second thing wrong, and
        // `merge_to_base` reports this one as "already up to date"
        // rather than as a failure.
        out.push(format!(
            "{target} already contains every commit on {branch}"
        ));
        return out;
    }
    match &p.merge_target {
        MergeTarget::Worktree { path, dirty } if *dirty > 0 => out.push(format!(
            "{target} is checked out in {path}, which has {dirty} uncommitted change{} — a merge there would run over them",
            if *dirty == 1 { "" } else { "s" }
        )),
        MergeTarget::Unchecked if !p.can_fast_forward => out.push(format!(
            "{target} is not checked out in any worktree and cannot be fast-forwarded — check it out and merge there"
        )),
        _ => {}
    }
    out
}

fn pr_blockers(p: &LandPreflight) -> Vec<String> {
    let mut out = Vec::new();
    if p.branch.is_none() {
        out.push("this room is on a detached HEAD, so there is no branch to push".to_owned());
        return out;
    }
    if p.remote.is_none() {
        out.push("this repository has no git remote to push to".to_owned());
    }
    if p.gh_version.is_none() {
        out.push(
            "`gh` is not installed or not on PATH, so no pull request can be opened".to_owned(),
        );
    }
    // A base that does not resolve locally may still exist on the
    // remote, so `base_resolved` is deliberately not a blocker here —
    // only the absence of a base at all is.
    if p.pr_base.is_none() {
        out.push("no base branch — pick one in the review pane's base picker".to_owned());
    }
    if p.ahead == 0 && p.base_resolved {
        out.push("there is nothing on this branch that the base does not already have".to_owned());
    }
    out
}

// ── merge ─────────────────────────────────────────────────────────

/// Merge the room's branch into `base`.
///
/// Re-runs [`preflight`] and refuses on any blocker: the dialog the
/// user confirmed was rendered from a snapshot, and an agent commits
/// while they read.
pub fn merge_to_base(cwd: &Path, base: Option<&str>) -> Result<MergeOutcome> {
    let pre = preflight(cwd, base)?;
    if let Some(reason) = pre.merge_blockers.first() {
        // `ahead == 0` is the one blocker that is not a problem: there
        // is simply nothing to do, and saying so is a better answer
        // than an error.
        if pre.ahead == 0 && pre.base_resolved && pre.merge_base.is_some() {
            return Ok(MergeOutcome {
                kind: MergeKind::AlreadyUpToDate,
                base: pre.merge_base.clone().unwrap_or_default(),
                branch: pre.branch.clone().unwrap_or_default(),
                base_sha: pre.base_sha.clone(),
                worktree: None,
                detail: reason.clone(),
            });
        }
        return Err(GitError::Refused(reason.clone()));
    }

    // `merge_base`, not `base`: the blockers above guarantee it is set,
    // and it is the local branch — the review's base may be the
    // remote-tracking ref that branch follows.
    let base_name = pre.merge_base.clone().unwrap_or_default();
    let branch = pre.branch.clone().unwrap_or_default();
    let before = pre.base_sha.clone();
    let head_sha = pre.head_sha.clone();

    let (detail, worktree) = match &pre.merge_target {
        MergeTarget::Worktree { path, .. } => {
            let target = Path::new(path);
            let out = cli::git(target, &["merge", "--no-edit", &branch])?;
            if !out.ok() {
                return Err(conflict_or_failure(target, &out));
            }
            (out.stdout.trim().to_owned(), Some(path.clone()))
        }
        MergeTarget::Unchecked => {
            // Nothing has base checked out and base is an ancestor of
            // HEAD, so moving the ref *is* the merge git would have
            // done. The old value is passed so a concurrent update
            // loses the race instead of being overwritten.
            let head = head_sha.clone().unwrap_or_default();
            let old = before.clone().unwrap_or_default();
            let refname = format!("refs/heads/{base_name}");
            let out = cli::git(cwd, &["update-ref", &refname, &head, &old])?;
            if !out.ok() {
                return Err(GitError::CommandFailed(
                    format!("update-ref {refname}"),
                    out.failure(),
                ));
            }
            (format!("fast-forwarded {base_name} to {branch}"), None)
        }
        MergeTarget::None => {
            return Err(GitError::Refused("no base branch to merge into".to_owned()));
        }
    };

    let after = Repo::open(cwd)?.resolve_commit(&base_name)?;
    let kind = if after == before {
        MergeKind::AlreadyUpToDate
    } else if after.is_some() && after == head_sha {
        MergeKind::FastForwarded
    } else {
        MergeKind::Merged
    };

    Ok(MergeOutcome {
        kind,
        base: base_name,
        branch,
        base_sha: after,
        worktree,
        detail,
    })
}

/// Turn a failed `git merge` into the right error, and leave nothing
/// half-merged behind.
///
/// A conflicted merge in a worktree the user is not looking at is worse
/// than no merge: they would find it days later with no idea what put
/// it there. Conflict resolution is out of scope for the epic, so the
/// merge is aborted and the conflicting files are named instead.
fn conflict_or_failure(target: &Path, out: &Output) -> GitError {
    let mid_merge =
        cli::git(target, &["rev-parse", "--verify", "--quiet", "MERGE_HEAD"]).is_ok_and(|o| o.ok());
    if !mid_merge {
        return GitError::CommandFailed("merge".to_owned(), out.failure());
    }
    let files = cli::git(target, &["diff", "--name-only", "--diff-filter=U"])
        .map(|o| {
            o.stdout
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let _ = cli::git(target, &["merge", "--abort"]);
    GitError::MergeConflict(if files.is_empty() {
        out.failure()
    } else {
        files.join(", ")
    })
}

// ── push and pull request ─────────────────────────────────────────

/// Push the room's branch, setting upstream.
///
/// Never forced, for the reason `grok-build`'s permission layer hard-
/// codes `git push` as never auto-approvable: it is the one action here
/// that other people can see, and it is not undoable in practice.
pub fn push_branch(cwd: &Path, remote: &str, branch: &str) -> Result<PushOutcome> {
    let out = cli::git_networked(cwd, &["push", "--set-upstream", remote, branch])?;
    if !out.ok() {
        return Err(GitError::CommandFailed(
            format!("push {remote} {branch}"),
            out.failure(),
        ));
    }
    Ok(PushOutcome {
        remote: remote.to_owned(),
        branch: branch.to_owned(),
        // git push narrates on stderr even when it succeeds.
        detail: out.stderr.trim().to_owned(),
    })
}

/// Open a pull request for `branch` against `base`, or return the one
/// that already exists.
///
/// `body` reaches `gh` on stdin via `--body-file -`. `gh` has no
/// `--body @-`: passing that sets the body to the literal string `@-`,
/// silently. The body is read back afterwards for the same reason.
///
/// Nothing from the room's review reaches this function — see the
/// module docs.
pub fn open_pr(cwd: &Path, base: &str, branch: &str, title: &str, body: &str) -> Result<PrOutcome> {
    if let Some(url) = existing_pr(cwd, branch)? {
        return Ok(PrOutcome {
            url,
            created: false,
            body_verified: true,
        });
    }

    let out = cli::gh(
        cwd,
        &[
            "pr",
            "create",
            "--base",
            base,
            "--head",
            branch,
            "--title",
            title,
            "--body-file",
            "-",
        ],
        Some(body),
    )?;
    if !out.ok() {
        return Err(GitError::CommandFailed(
            "gh pr create".to_owned(),
            out.failure(),
        ));
    }

    // gh prints the URL last; anything before it is progress chatter.
    let url = out
        .stdout
        .lines()
        .map(str::trim)
        .rfind(|l| l.starts_with("http"))
        .unwrap_or(out.line())
        .to_owned();

    let body_verified = read_back_body(cwd, &url).is_none_or(|got| got.trim() == body.trim());

    Ok(PrOutcome {
        url,
        created: true,
        body_verified,
    })
}

/// The URL of an open pull request for `branch`, if there is one.
fn existing_pr(cwd: &Path, branch: &str) -> Result<Option<String>> {
    let out = cli::gh(
        cwd,
        &["pr", "view", branch, "--json", "url", "--jq", ".url"],
        None,
    )?;
    if !out.ok() {
        // "no pull requests found" is the common case and is not a
        // failure. Anything else — no auth, no network — will surface
        // from `gh pr create` a moment later with a better message.
        return Ok(None);
    }
    let url = out.line().trim();
    Ok((!url.is_empty()).then(|| url.to_owned()))
}

/// The body GitHub actually stored, or `None` if it could not be read
/// back — an unreadable body is not evidence of a wrong one.
fn read_back_body(cwd: &Path, url: &str) -> Option<String> {
    let out = cli::gh(
        cwd,
        &["pr", "view", url, "--json", "body", "--jq", ".body"],
        None,
    )
    .ok()?;
    out.ok().then(|| out.line().to_owned())
}

// ── the small git questions ───────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreeEntry {
    pub path: String,
    /// `None` for a detached or bare worktree.
    pub branch: Option<String>,
}

fn worktrees(cwd: &Path) -> Result<Vec<WorktreeEntry>> {
    let out = cli::git(cwd, &["worktree", "list", "--porcelain"])?;
    if !out.ok() {
        return Ok(Vec::new());
    }
    Ok(parse_worktrees(&out.stdout))
}

/// Parse `git worktree list --porcelain`: blank-line-separated records
/// of `worktree <path>`, then `HEAD <sha>`, then either
/// `branch refs/heads/<name>`, `detached`, or `bare`.
pub(crate) fn parse_worktrees(text: &str) -> Vec<WorktreeEntry> {
    let mut out = Vec::new();
    let mut path: Option<String> = None;
    let mut branch: Option<String> = None;
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            if let Some(p) = path.take() {
                out.push(WorktreeEntry {
                    path: p,
                    branch: branch.take(),
                });
            }
            branch = None;
        } else if let Some(rest) = line.strip_prefix("worktree ") {
            // A record with no trailing blank line (the last one, on
            // some git versions) would otherwise be dropped.
            if let Some(p) = path.take() {
                out.push(WorktreeEntry {
                    path: p,
                    branch: branch.take(),
                });
            }
            path = Some(rest.replace('\\', "/"));
        } else if let Some(rest) = line.strip_prefix("branch refs/heads/") {
            branch = Some(rest.to_owned());
        }
    }
    if let Some(p) = path {
        out.push(WorktreeEntry { path: p, branch });
    }
    out
}

/// The local branch a merge would target, given the review's base.
///
/// A remote-tracking base is the ordinary case rather than the odd one:
/// the review pane guesses `origin/HEAD` first. `origin/main` cannot be
/// merged into, but `main` almost always exists beside it and is what
/// the user means — so this substitutes it, and the caller shows the
/// result rather than acting on it quietly.
fn local_equivalent(cwd: &Path, base: &str, base_is_local: bool) -> Option<String> {
    if base_is_local {
        return Some(base.to_owned());
    }
    let stripped = strip_remote(cwd, base);
    (stripped != base && is_local_branch(cwd, &stripped)).then_some(stripped)
}

fn is_local_branch(cwd: &Path, name: &str) -> bool {
    let refname = format!("refs/heads/{name}");
    cli::git(cwd, &["rev-parse", "--verify", "--quiet", &refname]).is_ok_and(|o| o.ok())
}

/// `(behind, ahead)` — commits base has that HEAD does not, and the
/// other way round.
fn ahead_behind(cwd: &Path, base_sha: &str, head_sha: &str) -> Result<(usize, usize)> {
    let range = format!("{base_sha}...{head_sha}");
    let out = cli::git(cwd, &["rev-list", "--left-right", "--count", &range])?;
    if !out.ok() {
        // Unrelated histories have no symmetric difference to count.
        return Ok((0, 0));
    }
    let mut parts = out.line().split_whitespace();
    let behind = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let ahead = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    Ok((behind, ahead))
}

/// Uncommitted paths, and how many there are in total.
fn dirty_paths(cwd: &Path) -> Result<(Vec<String>, usize)> {
    let out = cli::git(cwd, &["status", "--porcelain"])?;
    if !out.ok() {
        return Ok((Vec::new(), 0));
    }
    let all: Vec<String> = out
        .stdout
        .lines()
        .filter(|l| l.len() > 3)
        .map(|l| l[3..].trim().replace('\\', "/"))
        .collect();
    let total = all.len();
    Ok((all.into_iter().take(SAMPLE).collect(), total))
}

/// The remote to push to: the branch's configured one, else `origin`,
/// else whatever single remote exists.
fn remote_for(cwd: &Path, branch: &str) -> Option<String> {
    let key = format!("branch.{branch}.remote");
    if let Ok(out) = cli::git(cwd, &["config", "--get", &key])
        && out.ok()
        && !out.line().trim().is_empty()
    {
        return Some(out.line().trim().to_owned());
    }
    let out = cli::git(cwd, &["remote"]).ok()?;
    if !out.ok() {
        return None;
    }
    let names: Vec<&str> = out
        .stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    names
        .iter()
        .find(|n| **n == "origin")
        .or_else(|| names.first())
        .map(|n| (*n).to_owned())
}

fn upstream_of(cwd: &Path) -> Option<String> {
    let out = cli::git(
        cwd,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )
    .ok()?;
    out.ok()
        .then(|| out.line().trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// `origin/main` → `main`. `gh --base` wants a branch on the remote,
/// not a remote-tracking ref, and the review pane's base picker offers
/// both shapes.
fn strip_remote(cwd: &Path, base: &str) -> String {
    let Ok(out) = cli::git(cwd, &["remote"]) else {
        return base.to_owned();
    };
    for remote in out.stdout.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(rest) = base.strip_prefix(&format!("{remote}/")) {
            return rest.to_owned();
        }
    }
    base.to_owned()
}

fn gh_version(cwd: &Path) -> Option<String> {
    let out = cli::gh(cwd, &["--version"], None).ok()?;
    out.ok()
        .then(|| {
            out.stdout
                .lines()
                .next()
                .unwrap_or_default()
                .trim()
                .to_owned()
        })
        .filter(|s| !s.is_empty())
}

/// Commit summaries on the branch, oldest first.
fn commit_summaries(cwd: &Path, base_sha: &str) -> Result<Vec<String>> {
    let range = format!("{base_sha}..HEAD");
    let cap = format!("-{MAX_COMMITS}");
    let out = cli::git(cwd, &["log", "--reverse", "--format=%s", &cap, &range])?;
    if !out.ok() {
        return Ok(Vec::new());
    }
    Ok(out
        .stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect())
}

/// A title and body to prefill the PR form with.
///
/// One commit means the commit *is* the change, so its message is the
/// PR. Several means the branch is the change, and the commit the
/// branch opened with is the closest thing to a statement of intent —
/// with the rest listed so the user can see what they are editing.
///
/// Nothing here reads the review: D9 says the PR starts clean.
fn suggest_pr(cwd: &Path, branch: Option<&str>, commits: &[String]) -> (String, String) {
    let title = commits
        .first()
        .cloned()
        .or_else(|| branch.map(str::to_owned))
        .unwrap_or_default();
    let body = if commits.is_empty() {
        // Nothing on the branch yet. HEAD's message belongs to the
        // base, not to this change, and suggesting it would put a
        // stranger's commit in the user's pull request.
        String::new()
    } else if commits.len() == 1 {
        // Exactly one commit means HEAD *is* that commit.
        cli::git(cwd, &["log", "-1", "--format=%b"])
            .map(|o| o.stdout.trim().to_owned())
            .unwrap_or_default()
    } else {
        commits
            .iter()
            .map(|s| format!("- {s}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    (title, body)
}

fn empty() -> LandPreflight {
    LandPreflight {
        is_repo: false,
        branch: None,
        base: None,
        merge_base: None,
        base_resolved: false,
        base_is_local: false,
        head_sha: None,
        base_sha: None,
        ahead: 0,
        behind: 0,
        dirty: Vec::new(),
        dirty_total: 0,
        merge_target: MergeTarget::None,
        can_fast_forward: false,
        remote: None,
        upstream: None,
        pr_base: None,
        gh_version: None,
        commits: Vec::new(),
        suggested_title: String::new(),
        suggested_body: String::new(),
        merge_blockers: Vec::new(),
        pr_blockers: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_records_parse_including_the_last_one_without_a_blank_line() {
        let text = "worktree /repo\nHEAD abc\nbranch refs/heads/main\n\n\
                    worktree /repo-wt/task\nHEAD def\nbranch refs/heads/feat/x\n";
        let out = parse_worktrees(text);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].branch.as_deref(), Some("main"));
        assert_eq!(out[1].path, "/repo-wt/task");
        assert_eq!(out[1].branch.as_deref(), Some("feat/x"));
    }

    #[test]
    fn a_detached_or_bare_worktree_has_no_branch() {
        let text = "worktree /repo\nHEAD abc\ndetached\n\nworktree /bare\nbare\n";
        let out = parse_worktrees(text);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].branch, None);
        assert_eq!(out[1].branch, None);
    }

    #[test]
    fn windows_worktree_paths_normalise_to_forward_slashes() {
        // The rest of the review model keys on forward slashes, and the
        // path is shown to the user next to worktree-relative ones.
        let text = "worktree C:\\git\\skein\nHEAD abc\nbranch refs/heads/main\n";
        let out = parse_worktrees(text);
        assert_eq!(out[0].path, "C:/git/skein");
    }

    #[test]
    fn a_detached_head_blocks_both_actions_before_anything_else_is_asked() {
        let mut p = empty();
        p.is_repo = true;
        p.base = Some("main".to_owned());
        p.base_resolved = true;
        assert!(merge_blockers(&p)[0].contains("detached HEAD"));
        assert!(pr_blockers(&p)[0].contains("detached HEAD"));
    }

    #[test]
    fn a_remote_tracking_base_with_no_local_twin_is_refused_by_name() {
        // Only when there is genuinely nothing to merge into: the usual
        // case, `origin/main` beside a local `main`, is substituted in
        // `preflight` and never reaches here as a blocker.
        let mut p = empty();
        p.is_repo = true;
        p.branch = Some("feat/x".to_owned());
        p.base = Some("origin/main".to_owned());
        p.base_resolved = true;
        p.base_is_local = false;
        p.merge_base = None;
        p.ahead = 2;
        let blockers = merge_blockers(&p);
        assert_eq!(blockers.len(), 1);
        assert!(blockers[0].contains("remote-tracking"), "{blockers:?}");
        assert!(blockers[0].contains("no local main"), "{blockers:?}");
    }

    #[test]
    fn a_dirty_base_worktree_blocks_the_merge_and_says_where() {
        let mut p = empty();
        p.is_repo = true;
        p.branch = Some("feat/x".to_owned());
        p.base = Some("main".to_owned());
        p.base_resolved = true;
        p.merge_base = Some("main".to_owned());
        p.ahead = 1;
        p.merge_target = MergeTarget::Worktree {
            path: "/repo".to_owned(),
            dirty: 3,
        };
        let blockers = merge_blockers(&p);
        assert_eq!(blockers.len(), 1);
        assert!(blockers[0].contains("/repo"), "{blockers:?}");
        assert!(blockers[0].contains('3'), "{blockers:?}");
    }

    #[test]
    fn a_base_nobody_has_checked_out_is_fine_only_as_a_fast_forward() {
        let mut p = empty();
        p.is_repo = true;
        p.branch = Some("feat/x".to_owned());
        p.base = Some("main".to_owned());
        p.base_resolved = true;
        p.merge_base = Some("main".to_owned());
        p.ahead = 1;
        p.merge_target = MergeTarget::Unchecked;

        p.can_fast_forward = true;
        assert!(merge_blockers(&p).is_empty());

        p.can_fast_forward = false;
        assert!(merge_blockers(&p)[0].contains("not checked out"));
    }

    #[test]
    fn opening_a_pull_request_needs_a_remote_and_gh() {
        let mut p = empty();
        p.is_repo = true;
        p.branch = Some("feat/x".to_owned());
        p.base = Some("main".to_owned());
        p.pr_base = Some("main".to_owned());
        p.base_resolved = true;
        p.ahead = 1;

        let blockers = pr_blockers(&p);
        assert_eq!(blockers.len(), 2, "{blockers:?}");
        assert!(blockers.iter().any(|b| b.contains("no git remote")));
        assert!(blockers.iter().any(|b| b.contains("`gh`")));

        p.remote = Some("origin".to_owned());
        p.gh_version = Some("gh version 2.0.0".to_owned());
        assert!(pr_blockers(&p).is_empty());
    }

    #[test]
    fn nothing_to_land_is_a_blocker_on_both_paths() {
        let mut p = empty();
        p.is_repo = true;
        p.branch = Some("feat/x".to_owned());
        p.base = Some("main".to_owned());
        p.pr_base = Some("main".to_owned());
        p.base_resolved = true;
        p.merge_base = Some("main".to_owned());
        p.remote = Some("origin".to_owned());
        p.gh_version = Some("gh version 2.0.0".to_owned());
        p.ahead = 0;
        p.merge_target = MergeTarget::Unchecked;
        // Exactly one reason, even though a base nobody has checked out
        // also cannot be fast-forwarded: with nothing to land, that is
        // not a second thing wrong, and `merge_to_base` keys its
        // "already up to date" answer on this being the only blocker.
        let blockers = merge_blockers(&p);
        assert_eq!(blockers.len(), 1, "{blockers:?}");
        assert!(blockers[0].contains("already contains"));
        assert!(pr_blockers(&p)[0].contains("nothing on this branch"));
    }
}
