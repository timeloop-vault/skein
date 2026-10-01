//! Shared fixtures for the `skein-git` integration tests. Each test
//! binary uses a subset, hence the `dead_code` allowance.

#![allow(dead_code)]

use std::fs;
use std::path::Path;

use git2::{Repository, Signature};
use tempfile::TempDir;

/// Create a repo with one initial commit on `main` so `branches()` returns
/// at least one entry.
pub fn init_repo() -> (TempDir, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_path_buf();
    let repo = Repository::init(&path).unwrap();
    {
        // Need a commit before HEAD points anywhere meaningful — write a
        // tiny README and commit it.
        fs::write(path.join("README.md"), b"hello\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("README.md")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = Signature::now("test", "test@example.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        // Some libgit2 versions default the initial branch to `master`;
        // rename it to `main` so the tests don't depend on libgit2's
        // default.
        let head_branch_name = repo.head().unwrap().shorthand().map(str::to_owned).unwrap();
        if head_branch_name != "main" {
            let mut branch = repo
                .find_branch(&head_branch_name, git2::BranchType::Local)
                .unwrap();
            branch.rename("main", false).unwrap();
        }
    }
    (tmp, path)
}

/// Link `link` (relative to `root`) to the directory `target`, the way
/// a Skein worktree gets its `node_modules`: a junction on Windows
/// (`mklink /J` needs no elevation, unlike a real symlink) and a
/// symlink elsewhere. `false` when the platform refuses — the caller
/// then skips rather than fails, so a locked-down CI box still runs
/// the rest of the suite.
pub fn link_dir(root: &Path, link: &str, target: &Path) -> bool {
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(root.join(link))
            .arg(target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, root.join(link)).is_ok()
    }
}

/// A repo whose `.gitignore` excludes `node_modules/` and which has a
/// directory link by that name, mirroring the real worktree layout.
/// `None` when the platform won't make the link.
pub fn repo_with_node_modules_link() -> Option<(TempDir, std::path::PathBuf)> {
    let (tmp, path) = init_repo();
    fs::write(path.join(".gitignore"), b"node_modules/\n").unwrap();
    let target = path.join("real_modules");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("index.js"), b"module.exports = 1;\n").unwrap();
    if !link_dir(&path, "node_modules", &target) {
        return None;
    }
    Some((tmp, path))
}
