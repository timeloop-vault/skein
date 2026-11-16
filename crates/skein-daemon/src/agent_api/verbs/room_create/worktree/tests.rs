use super::*;

fn db(dir: &tempfile::TempDir) -> Database {
    Database::open(&dir.path().join("skein.db")).unwrap()
}

fn spec<'a>(path: &'a str, mode: BranchMode, task: &'a str) -> CheckoutSpec<'a> {
    CheckoutSpec {
        path,
        mode,
        task,
        branch: None,
        base_branch: None,
    }
}

fn message(result: VerbResult<Checkout>) -> String {
    match result {
        Err(VerbError::Refused(m)) => m,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn an_empty_task_is_refused() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();
    let got = prepare(&db(&dir), &spec(path, BranchMode::Current, "   "));
    assert_eq!(message(got), "task can't be empty");
}

#[test]
fn a_missing_folder_is_refused() {
    let dir = tempfile::TempDir::new().unwrap();
    let gone = dir.path().join("gone");
    let path = gone.to_str().unwrap();
    let got = prepare(&db(&dir), &spec(path, BranchMode::Current, "x"));
    assert_eq!(message(got), "folder not found");
}

#[test]
fn worktree_mode_needs_a_repository() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();
    let got = prepare(&db(&dir), &spec(path, BranchMode::Worktree, "x"));
    assert_eq!(message(got), "not a git repo");
}

#[test]
fn current_mode_in_a_plain_folder_has_no_branch_and_no_repo_root() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();
    let got = prepare(&db(&dir), &spec(path, BranchMode::Current, "x")).unwrap();
    assert_eq!(
        got,
        Checkout {
            cwd: path.to_owned(),
            branch: None,
            repo_root: None,
            base_behind_upstream: None,
            base_sha: None,
            created_worktree: false,
        }
    );
}

#[test]
fn a_repository_with_no_commits_has_no_base_branch() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git2::Repository::init(&repo).unwrap();
    let path = repo.to_str().unwrap();
    let got = prepare(&db(&dir), &spec(path, BranchMode::Worktree, "x"));
    assert_eq!(message(got), "no base branch available");
}
