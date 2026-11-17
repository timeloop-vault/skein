//! `EnsureRequest::run_shell` (#756): the daemon, not the client, writes the
//! shell launch, and the request proceeds as a restart.

use skein_proto::room_mutations::{ROOMS_CHANGED_EVENT, RoomsChanged};
use skein_proto::rooms::{LaunchMode, ShellClaim};

use super::super::{RUNNER_ORIGIN, Refusal};
use super::*;

const SID: &str = "7f3c0a52-0000-4000-8000-000000000756";

/// A Claude harness in tool mode with a shell claim, over a stand-in process.
fn tool_fx() -> Fx {
    let fx = Fx::new();
    let mut h = claude_stand_in(&fx, shell_cmd(), SID);
    h.shell_claim = Some(ShellClaim {
        session_id: SID.to_owned(),
        port: None,
    });
    fx.create(room("r1", &fx.work, vec![h]));
    fx
}

fn stored(fx: &Fx) -> Harness {
    fx.daemon.rooms.harness_record("r1", "h1").unwrap()
}

fn revision(fx: &Fx) -> u64 {
    fx.daemon
        .rooms
        .rooms()
        .iter()
        .find(|r| r.id == "r1")
        .unwrap()
        .revision
}

/// The shell argv asked for, distinct from the `claude ...` a tool launch derives.
fn shell_argv() -> Vec<String> {
    let mut argv = shell_cmd();
    argv.push(if cfg!(windows) { "/q" } else { "-i" }.to_owned());
    argv
}

fn request(fx: &Fx, restart: bool, run_shell: Option<Vec<String>>) -> EnsureRequest {
    let _ = fx;
    EnsureRequest {
        room_id: "r1".to_owned(),
        harness_id: "h1".to_owned(),
        rows: 24,
        cols: 80,
        env_profile: None,
        restart,
        expect_cmd: None,
        expect_launch: None,
        run_shell,
    }
}

#[test]
fn run_shell_stores_the_launch_clears_the_claim_and_spawns_the_shell() {
    let fx = tool_fx();
    let before = stored(&fx);
    let out = fx
        .runner()
        .ensure_running(&request(&fx, false, Some(shell_argv())), None)
        .unwrap();
    assert!(out.started);

    let after = stored(&fx);
    assert_eq!(after.launch, Some(LaunchMode::Shell { argv: shell_argv() }));
    assert!(after.shell_claim.is_none());
    assert_eq!(after.cmd, before.cmd);
    assert_eq!(after.session_id, before.session_id);
    assert_eq!(fx.runner().spawned_argv(), vec![shell_argv()]);
}

#[test]
fn run_shell_emits_one_runner_origin_change_and_bumps_the_revision() {
    let fx = tool_fx();
    let (rev, seen) = (revision(&fx), fx.events.0.lock().len());
    fx.runner()
        .ensure_running(&request(&fx, false, Some(shell_argv())), None)
        .unwrap();
    assert!(revision(&fx) > rev);

    let origins: Vec<String> = fx.events.0.lock()[seen..]
        .iter()
        .filter(|(name, _)| name == ROOMS_CHANGED_EVENT)
        .map(|(_, payload)| {
            serde_json::from_str::<RoomsChanged>(payload)
                .unwrap()
                .origin
        })
        .collect();
    assert!(
        origins.iter().any(|o| o == RUNNER_ORIGIN),
        "origins: {origins:?}"
    );
}

#[test]
fn run_shell_replaces_a_running_process_even_without_restart() {
    let fx = tool_fx();
    let first = fx.ensure(false, None).unwrap();
    let old_pid = fx.daemon.pty.pid(&first.spawn.id).expect("pid");

    let second = fx
        .runner()
        .ensure_running(&request(&fx, false, Some(shell_argv())), None)
        .unwrap();
    assert!(second.started);
    assert_ne!(second.spawn.id, first.spawn.id);
    assert!(wait_until(|| !pid_exists(old_pid)), "the old process lives");
    assert_eq!(fx.runner().spawned_argv().last(), Some(&shell_argv()));
}

#[test]
fn run_shell_with_an_empty_argv_is_refused_and_writes_nothing() {
    let fx = tool_fx();
    let before = stored(&fx);
    let rev = revision(&fx);
    let err = fx
        .runner()
        .ensure_running(&request(&fx, false, Some(Vec::new())), None)
        .err()
        .expect("refused");
    assert_eq!(err, EnsureError::Refused(Refusal::EmptyShellArgv));
    assert_eq!(err.code(), "empty_shell_argv");
    assert_eq!(stored(&fx).launch, before.launch);
    assert!(stored(&fx).shell_claim.is_some());
    assert_eq!(revision(&fx), rev);
    assert_eq!(fx.runner().spawned_argv().len(), 0);
}

fn repo_with_commit(dir: &Path, message: &str) -> git2::Oid {
    std::fs::create_dir_all(dir).unwrap();
    let repo = git2::Repository::init(dir).unwrap();
    let sig = git2::Signature::now("Test", "test@example.invalid").unwrap();
    let tree_id = repo.index().unwrap().write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[])
        .unwrap()
}

/// A shell request refused by the spawn plan writes nothing: not the launch,
/// not the claim, no revision bump and no rooms-changed event.
fn assert_refused_without_writing(fx: &Fx, code: &str) {
    let before = stored(fx);
    let rev = revision(fx);
    let seen = fx.events.0.lock().len();
    let err = fx
        .runner()
        .ensure_running(&request(fx, false, Some(shell_argv())), None)
        .err()
        .expect("refused");
    assert_eq!(err.code(), code);
    let after = stored(fx);
    assert_eq!(after.launch, before.launch);
    assert_eq!(after.shell_claim, before.shell_claim);
    assert!(after.shell_claim.is_some());
    assert_eq!(revision(fx), rev);
    let changed = fx.events.0.lock()[seen..]
        .iter()
        .filter(|(name, _)| name == ROOMS_CHANGED_EVENT)
        .count();
    assert_eq!(changed, 0, "a refused request emitted rooms-changed");
    assert_eq!(fx.runner().spawned_argv().len(), 0);
}

fn claimed_harness(fx: &Fx) -> Harness {
    let mut h = claude_stand_in(fx, shell_cmd(), SID);
    h.shell_claim = Some(ShellClaim {
        session_id: SID.to_owned(),
        port: None,
    });
    h
}

#[test]
fn run_shell_for_a_missing_folder_is_refused_and_writes_nothing() {
    let fx = Fx::new();
    let h = claimed_harness(&fx);
    fx.create(room("r1", &fx.work.join("does-not-exist"), vec![h]));
    assert_refused_without_writing(&fx, "folder_missing");
}

#[test]
fn run_shell_for_a_repo_mismatch_is_refused_and_writes_nothing() {
    let fx = Fx::new();
    let recorded = repo_with_commit(&fx.work.join("repo-a"), "root of a");
    repo_with_commit(&fx.work.join("repo-b"), "root of b");
    let h = claimed_harness(&fx);
    let mut r = room("r1", &fx.work.join("repo-b"), vec![h]);
    r.repo_identity = Some(skein_proto::rooms::RepoIdentity {
        root_commits: vec![recorded.to_string()],
        origin_url: None,
    });
    fx.create(r);
    assert_refused_without_writing(&fx, "repo_mismatch");
}

#[test]
fn run_shell_ignores_expect_launch_but_not_a_stale_expect_cmd() {
    let fx = tool_fx();
    let mut req = request(&fx, false, Some(shell_argv()));
    req.expect_launch = Some(LaunchMode::Shell { argv: shell_argv() });
    fx.runner().ensure_running(&req, None).unwrap();

    let fx = tool_fx();
    let mut req = request(&fx, false, Some(shell_argv()));
    req.expect_cmd = Some(vec!["other".to_owned()]);
    let err = fx.runner().ensure_running(&req, None).err().unwrap();
    assert_eq!(err, EnsureError::Refused(Refusal::StaleRecord));
    assert_eq!(stored(&fx).launch, None);
}
