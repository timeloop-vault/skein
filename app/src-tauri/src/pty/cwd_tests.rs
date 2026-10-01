use super::check_cwd;

#[test]
fn an_existing_directory_is_fine() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(check_cwd(dir.path()).is_ok());
}

#[test]
fn a_missing_path_errors_with_the_path_in_the_message() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("removed-worktree");

    let err = check_cwd(&missing).expect_err("missing cwd must be refused");
    assert!(
        err.0.contains(&missing.display().to_string()),
        "error should name the missing path, got: {}",
        err.0
    );
    assert!(
        err.0.contains("does not exist"),
        "error should say the path is missing, got: {}",
        err.0
    );
}

#[test]
fn a_file_path_is_refused_as_not_a_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("not-a-dir");
    std::fs::write(&file, "hi").expect("write file");

    let err = check_cwd(&file).expect_err("a file cwd must be refused");
    assert!(
        err.0.contains(&file.display().to_string()),
        "error should name the file path, got: {}",
        err.0
    );
    assert!(
        err.0.contains("is not a directory"),
        "error should say it isn't a directory, got: {}",
        err.0
    );
}
