use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum GitError {
    #[error("path does not exist: {0}")]
    PathMissing(PathBuf),

    #[error("not a git repository: {0}")]
    NotARepo(PathBuf),

    #[error("branch not found: {0}")]
    BranchNotFound(String),

    #[error("worktree {0} already exists")]
    WorktreeExists(String),

    #[error("branch {0} is already checked out in another worktree")]
    BranchCheckedOut(String),

    #[error("worktree {0} could not be confirmed absent: {1}")]
    WorktreeUnreachable(String, String),

    #[error("git: {0}")]
    Git(#[from] git2::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, GitError>;
