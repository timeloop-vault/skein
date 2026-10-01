use super::{resolve_program_in, windows_resolved_program};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// PATHEXT as Windows hands it to us, lowercased the way
/// `path_exts` does.
fn exts() -> Vec<String> {
    [".com", ".exe", ".bat", ".cmd"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}

fn touch_exe(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, "shim").expect("write");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    path
}

#[test]
fn the_npm_sh_shim_never_wins_over_the_cmd_shim() {
    // The exact layout `npm i -g opencode-ai` leaves behind.
    let dir = tempfile::tempdir().expect("tempdir");
    touch_exe(dir.path(), "opencode");
    let shim = touch_exe(dir.path(), "opencode.cmd");
    touch_exe(dir.path(), "opencode.ps1");
    let path = OsString::from(dir.path());
    assert_eq!(
        resolve_program_in("opencode", &path, &exts(), false).as_deref(),
        Some(shim.display().to_string().as_str())
    );
}

#[test]
fn an_extensionless_program_is_unresolvable_when_bare_names_are_off() {
    let dir = tempfile::tempdir().expect("tempdir");
    touch_exe(dir.path(), "opencode");
    let path = OsString::from(dir.path());
    assert_eq!(resolve_program_in("opencode", &path, &exts(), false), None);
    // Unix passes `bare_ok = true`, and there the same file resolves.
    assert!(resolve_program_in("opencode", &path, &exts(), true).is_some());
}

#[test]
fn a_program_spelled_with_its_extension_still_resolves() {
    // `powershell.exe` would otherwise only ever be probed as
    // `powershell.exe.com`, `powershell.exe.exe`, …
    let dir = tempfile::tempdir().expect("tempdir");
    let exe = touch_exe(dir.path(), "powershell.exe");
    let path = OsString::from(dir.path());
    assert_eq!(
        resolve_program_in("powershell.exe", &path, &exts(), true).as_deref(),
        Some(exe.display().to_string().as_str())
    );
}

#[test]
fn earlier_path_dirs_still_win() {
    let first = tempfile::tempdir().expect("tempdir");
    let second = tempfile::tempdir().expect("tempdir");
    let wanted = touch_exe(first.path(), "gh.exe");
    touch_exe(second.path(), "gh.exe");
    let path = std::env::join_paths([first.path(), second.path()]).expect("join");
    assert_eq!(
        resolve_program_in("gh", &path, &exts(), false).as_deref(),
        Some(wanted.display().to_string().as_str())
    );
}

#[test]
fn a_program_that_already_names_a_file_is_left_alone() {
    // Not ours to re-resolve — and off Windows nothing is, so this
    // holds on both.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = OsString::from(dir.path());
    for spelled in [
        r"C:\npm\opencode.cmd",
        "./opencode",
        "/usr/local/bin/opencode",
    ] {
        assert_eq!(windows_resolved_program(spelled, &path), None);
    }
}

#[test]
fn an_unresolvable_program_defers_to_portable_pty() {
    // Better a spawn error naming `nosuchtool` than a rewrite to
    // something we guessed at.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = OsString::from(dir.path());
    assert_eq!(windows_resolved_program("nosuchtool", &path), None);
}
