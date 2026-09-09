//! The land actions' Tauri boundary (#214, epic #52 D9).
//!
//! Three commands, and no logic worth the name: the git work lives in
//! `skein-git`'s `land` module, where it is tested against real
//! repositories without a Tauri runtime in the way. What is here is the
//! DTO shape the review pane receives, the room's base ref (which lives
//! in sqlite, not in git), and the blocking-pool hop — every one of
//! these spawns processes, and one of them talks to a network.
//!
//! # What this deliberately does not do
//!
//! Nothing here reads `review_threads` or `review_comments`. D9: the
//! Skein-local review stays local, the pull request starts clean. The
//! commands take the PR title and body from the caller, which prefills
//! them from the branch's own commits.
//!
//! # Confirmation
//!
//! Neither action fires without one, and the confirmation is the
//! frontend's — a native dialog naming exactly what will move. The
//! backend's half of that contract is [`land_preflight`], which is the
//! only reason the dialog can name it, plus the re-check both actions
//! run inside `skein-git` before touching anything.

use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use skein_git::land::{self, LandPreflight, MergeKind, MergeOutcome, MergeTarget, PrOutcome};

use crate::db::Database;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeTargetDto {
    /// "worktree" | "unchecked" | "none".
    pub kind: &'static str,
    pub path: Option<String>,
    pub dirty: usize,
}

impl From<&MergeTarget> for MergeTargetDto {
    fn from(t: &MergeTarget) -> Self {
        match t {
            MergeTarget::Worktree { path, dirty } => Self {
                kind: "worktree",
                path: Some(path.clone()),
                dirty: *dirty,
            },
            MergeTarget::Unchecked => Self {
                kind: "unchecked",
                path: None,
                dirty: 0,
            },
            MergeTarget::None => Self {
                kind: "none",
                path: None,
                dirty: 0,
            },
        }
    }
}

/// The preflight, as the pane receives it. A flat bag of facts, like
/// the struct it mirrors: the dialog renders all of them at once.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)]
pub struct LandPreflightDto {
    pub is_repo: bool,
    pub branch: Option<String>,
    /// The review's base, as the user picked or Skein guessed it.
    pub base: Option<String>,
    /// The local branch a merge would actually hit. Differs from `base`
    /// when that is a remote-tracking ref (`origin/main`), which is the
    /// ordinary case — the dialog names this one, so the substitution
    /// is visible rather than quiet.
    pub merge_base: Option<String>,
    pub base_resolved: bool,
    pub base_is_local: bool,
    pub ahead: usize,
    pub behind: usize,
    /// A sample of the room's uncommitted paths — a warning, never a
    /// blocker: a merge takes commits, so these do not travel.
    pub dirty: Vec<String>,
    pub dirty_total: usize,
    pub merge_target: MergeTargetDto,
    pub can_fast_forward: bool,
    pub remote: Option<String>,
    pub upstream: Option<String>,
    pub pr_base: Option<String>,
    pub gh_version: Option<String>,
    pub commits: Vec<String>,
    pub suggested_title: String,
    pub suggested_body: String,
    pub merge_blockers: Vec<String>,
    pub pr_blockers: Vec<String>,
}

impl From<LandPreflight> for LandPreflightDto {
    fn from(p: LandPreflight) -> Self {
        let merge_target = MergeTargetDto::from(&p.merge_target);
        Self {
            is_repo: p.is_repo,
            branch: p.branch,
            base: p.base,
            merge_base: p.merge_base,
            base_resolved: p.base_resolved,
            base_is_local: p.base_is_local,
            ahead: p.ahead,
            behind: p.behind,
            dirty: p.dirty,
            dirty_total: p.dirty_total,
            merge_target,
            can_fast_forward: p.can_fast_forward,
            remote: p.remote,
            upstream: p.upstream,
            pr_base: p.pr_base,
            gh_version: p.gh_version,
            commits: p.commits,
            suggested_title: p.suggested_title,
            suggested_body: p.suggested_body,
            merge_blockers: p.merge_blockers,
            pr_blockers: p.pr_blockers,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeOutcomeDto {
    /// "uptodate" | "fastforward" | "merged".
    pub kind: &'static str,
    pub base: String,
    pub branch: String,
    pub base_sha: Option<String>,
    pub worktree: Option<String>,
    pub detail: String,
}

impl From<MergeOutcome> for MergeOutcomeDto {
    fn from(o: MergeOutcome) -> Self {
        Self {
            kind: match o.kind {
                MergeKind::AlreadyUpToDate => "uptodate",
                MergeKind::FastForwarded => "fastforward",
                MergeKind::Merged => "merged",
            },
            base: o.base,
            branch: o.branch,
            base_sha: o.base_sha,
            worktree: o.worktree,
            detail: o.detail,
        }
    }
}

/// Push and pull-request are one action to the user and two to git, so
/// the result has to be able to say the first half worked and the
/// second did not. A failed push is an error; a failed PR *after* a
/// successful push is this, with `pr_error` set — losing that would
/// leave the user believing nothing happened while the branch is on the
/// remote.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LandPrDto {
    pub remote: String,
    pub branch: String,
    pub push_detail: String,
    pub url: Option<String>,
    /// `false` when the pull request already existed.
    pub created: bool,
    /// Whether the body GitHub stored matches the body that was sent.
    pub body_verified: bool,
    pub pr_error: Option<String>,
}

/// Everything the land dialog needs to say, and every reason it cannot
/// act. Reads only.
#[tauri::command]
pub async fn land_preflight(
    room_id: String,
    cwd: String,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<LandPreflightDto, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        // The room's reviewed base, not a fresh guess: landing has to
        // agree with what the user was reading in the review pane.
        let base = db.review_base_ref(&room_id)?;
        land::preflight(Path::new(&cwd), base.as_deref())
            .map(LandPreflightDto::from)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Merge the room's branch into its base.
///
/// Confirmed in the UI first; re-checked in `skein-git` immediately
/// before it acts, because an agent commits while a dialog is open.
#[tauri::command]
pub async fn land_merge(
    room_id: String,
    cwd: String,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<MergeOutcomeDto, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        let base = db.review_base_ref(&room_id)?;
        land::merge_to_base(Path::new(&cwd), base.as_deref())
            .map(MergeOutcomeDto::from)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Push the room's branch and open a pull request for it.
///
/// `title` and `body` come from the user, prefilled from the branch's
/// commits. No Skein review comment reaches either.
#[tauri::command]
pub async fn land_open_pr(
    room_id: String,
    cwd: String,
    title: String,
    body: String,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<LandPrDto, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        let base = db.review_base_ref(&room_id)?;
        let path = Path::new(&cwd);
        let pre = land::preflight(path, base.as_deref()).map_err(|e| e.to_string())?;
        if let Some(reason) = pre.pr_blockers.first() {
            return Err(reason.clone());
        }
        let (Some(branch), Some(remote), Some(pr_base)) = (&pre.branch, &pre.remote, &pre.pr_base)
        else {
            // Unreachable given the blockers above; still not worth an
            // unwrap on a path that pushes to a remote.
            return Err("nothing to push — no branch, remote or base".to_owned());
        };

        let push = land::push_branch(path, remote, branch).map_err(|e| e.to_string())?;

        // Past this point the branch is on the remote. A pull-request
        // failure must not read as "nothing happened".
        match land::open_pr(path, pr_base, branch, &title, &body) {
            Ok(PrOutcome {
                url,
                created,
                body_verified,
            }) => Ok(LandPrDto {
                remote: push.remote,
                branch: push.branch,
                push_detail: push.detail,
                url: Some(url),
                created,
                body_verified,
                pr_error: None,
            }),
            Err(e) => Ok(LandPrDto {
                remote: push.remote,
                branch: push.branch,
                push_detail: push.detail,
                url: None,
                created: false,
                body_verified: false,
                pr_error: Some(e.to_string()),
            }),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_targets_serialise_to_the_names_the_dialog_switches_on() {
        let dto = MergeTargetDto::from(&MergeTarget::Worktree {
            path: "/repo".to_owned(),
            dirty: 2,
        });
        assert_eq!(dto.kind, "worktree");
        assert_eq!(dto.path.as_deref(), Some("/repo"));
        assert_eq!(dto.dirty, 2);

        assert_eq!(
            MergeTargetDto::from(&MergeTarget::Unchecked).kind,
            "unchecked"
        );
        assert_eq!(MergeTargetDto::from(&MergeTarget::None).kind, "none");
    }

    #[test]
    fn merge_kinds_serialise_to_the_names_the_result_line_switches_on() {
        let outcome = |kind| MergeOutcome {
            kind,
            base: "main".to_owned(),
            branch: "feat/x".to_owned(),
            base_sha: None,
            worktree: None,
            detail: String::new(),
        };
        assert_eq!(
            MergeOutcomeDto::from(outcome(MergeKind::AlreadyUpToDate)).kind,
            "uptodate"
        );
        assert_eq!(
            MergeOutcomeDto::from(outcome(MergeKind::FastForwarded)).kind,
            "fastforward"
        );
        assert_eq!(
            MergeOutcomeDto::from(outcome(MergeKind::Merged)).kind,
            "merged"
        );
    }

    #[test]
    fn the_preflight_dto_carries_both_blocker_lists_through_unchanged() {
        // The dialog renders these verbatim, so a lost or reordered
        // blocker is a dialog that offers an action that cannot run.
        let mut pre = land::preflight(Path::new("/definitely/not/a/repo"), None).unwrap();
        pre.merge_blockers.push("second reason".to_owned());
        let dto = LandPreflightDto::from(pre);
        assert!(!dto.is_repo);
        assert_eq!(dto.merge_blockers.len(), 2);
        assert_eq!(dto.merge_blockers[1], "second reason");
        assert_eq!(dto.pr_blockers.len(), 1);
    }
}
