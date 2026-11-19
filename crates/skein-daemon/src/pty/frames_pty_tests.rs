//! `PtyManager` in frames mode against a real child (#760, T-3), plus the
//! raw-mode guard. Runs on Windows (`ConPTY`, which echoes and repaints on
//! its own, so assertions stick to markers and event order) and unix.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::frames::TerminalMode;
use super::{AttachOutcome, PtyEvent, PtyManager, SpawnRequest};
use crate::harness_kind::HarnessKind;
use crate::spawn_settings::SpawnSettings;
use crate::stream_sink::StreamSink;

const BEGIN: &str = "\x1b[?2026h";
const END: &str = "\x1b[?2026l";
const ERASE: &str = "\x1b[2J";

#[cfg(windows)]
const SHELL: &str = "cmd.exe";
#[cfg(not(windows))]
const SHELL: &str = "sh";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Ev {
    Grid(u16, u16),
    Data(String),
    Exit(Option<u32>),
}

#[derive(Clone, Default)]
struct Rec(Arc<Mutex<Vec<Ev>>>);

impl Rec {
    fn events(&self) -> Vec<Ev> {
        self.0.lock().clone()
    }
    fn datas(&self) -> Vec<String> {
        self.events()
            .into_iter()
            .filter_map(|e| if let Ev::Data(d) = e { Some(d) } else { None })
            .collect()
    }
    fn text(&self) -> String {
        self.datas().concat()
    }
    fn grids(&self) -> Vec<(u16, u16)> {
        self.events()
            .into_iter()
            .filter_map(|e| {
                if let Ev::Grid(r, c) = e {
                    Some((r, c))
                } else {
                    None
                }
            })
            .collect()
    }
    fn exited(&self) -> bool {
        self.events().iter().any(|e| matches!(e, Ev::Exit(_)))
    }
}

impl StreamSink<PtyEvent> for Rec {
    fn send(&self, item: PtyEvent) -> Result<(), String> {
        self.0.lock().push(match item {
            PtyEvent::Data { chunk } => Ev::Data(chunk),
            PtyEvent::Exit { code } => Ev::Exit(code),
            PtyEvent::Grid { rows, cols } => Ev::Grid(rows, cols),
        });
        Ok(())
    }
}

fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for {what}");
}

fn wait_text(rec: &Rec, needle: &str) {
    wait_until(needle, || rec.text().contains(needle));
}

/// Spawn the platform shell as `id` at 24x80.
fn spawn(manager: &PtyManager, id: &str, sink: Option<Box<dyn StreamSink<PtyEvent>>>) {
    let cmd = vec![SHELL.to_owned()];
    let cwd = std::env::temp_dir();
    let settings = SpawnSettings::default();
    manager
        .spawn_with_sink(
            SpawnRequest {
                id: id.to_owned(),
                cmd: &cmd,
                cwd: Path::new(&cwd),
                rows: 24,
                cols: 80,
                settings: &settings,
                env_profile: None,
                agent: None,
                kind: HarnessKind::Byoh,
                harness_config: None,
                secret_env: Vec::new(),
                sidecar: None,
            },
            sink,
            None,
            None,
        )
        .expect("spawn");
}

fn frames() -> PtyManager {
    PtyManager::with_terminal_mode(TerminalMode::Frames)
}

fn echo(manager: &PtyManager, id: &str, viewer: Option<u64>, marker: &str) {
    manager
        .write_from(id, viewer, format!("echo {marker}\r\n").as_bytes())
        .expect("write");
}

#[test]
fn an_initial_sink_gets_the_grid_then_frames_with_the_output() {
    let manager = frames();
    let rec = Rec::default();
    spawn(&manager, "fr-1", Some(Box::new(rec.clone())));
    echo(&manager, "fr-1", None, "skein-frame-one");
    wait_text(&rec, "skein-frame-one");

    assert_eq!(rec.events().first(), Some(&Ev::Grid(24, 80)));
    let datas = rec.datas();
    for d in &datas {
        assert!(d.starts_with(BEGIN), "frame without begin: {d:?}");
        assert!(d.ends_with(END), "frame without end: {d:?}");
    }
    manager.kill("fr-1");
}

#[test]
fn a_late_viewer_sees_earlier_output_and_both_follow_the_child() {
    let manager = frames();
    let first = Rec::default();
    spawn(&manager, "fr-2", Some(Box::new(first.clone())));
    echo(&manager, "fr-2", None, "skein-before-attach");
    wait_text(&first, "skein-before-attach");

    let second = Rec::default();
    let (outcome, second_id) = manager
        .attach_viewer("fr-2", Box::new(second.clone()), Some((24, 80)))
        .expect("attach");
    assert_eq!(outcome, AttachOutcome::Live);
    assert_eq!(manager.attach_id("fr-2"), Some(second_id));
    wait_until("the second viewer's first frame", || {
        !second.datas().is_empty()
    });
    assert_eq!(second.events().first(), Some(&Ev::Grid(24, 80)));
    assert!(
        second.datas()[0].contains("skein-before-attach"),
        "first frame lacks earlier output: {:?}",
        second.datas()[0]
    );

    echo(&manager, "fr-2", Some(second_id), "skein-both-see");
    wait_text(&first, "skein-both-see");
    wait_text(&second, "skein-both-see");

    // The first viewer is the one with attach id 1.
    assert!(manager.detach_if("fr-2", 1));
    echo(&manager, "fr-2", Some(second_id), "skein-only-second");
    wait_text(&second, "skein-only-second");
    assert!(!first.text().contains("skein-only-second"));
    manager.kill("fr-2");
}

#[test]
fn only_the_size_owner_resizes_the_pty() {
    let manager = frames();
    spawn(&manager, "fr-3", None);
    let (a, b) = (Rec::default(), Rec::default());
    let (_, a_id) = manager
        .attach_viewer("fr-3", Box::new(a.clone()), Some((24, 80)))
        .expect("attach a");
    let (_, b_id) = manager
        .attach_viewer("fr-3", Box::new(b.clone()), Some((30, 100)))
        .expect("attach b");
    assert_eq!(b.grids(), vec![(24, 80)]);

    manager
        .resize_from("fr-3", Some(b_id), 31, 101)
        .expect("non-owner resize is recorded, not an error");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(a.grids(), vec![(24, 80)], "a non-owner moved the grid");
    assert_eq!(b.grids(), vec![(24, 80)], "a non-owner moved the grid");

    manager
        .resize_from("fr-3", Some(a_id), 20, 60)
        .expect("owner resize");
    wait_until("both to get 20x60", || {
        a.grids().last() == Some(&(20, 60)) && b.grids().last() == Some(&(20, 60))
    });

    // Input from b makes it the owner and applies its recorded size.
    manager
        .write_from("fr-3", Some(b_id), b"\r\n")
        .expect("write");
    wait_until("both to get 31x101", || {
        a.grids().last() == Some(&(31, 101)) && b.grids().last() == Some(&(31, 101))
    });
    manager.kill("fr-3");
}

#[test]
fn an_exit_reaches_every_viewer_after_their_last_frame() {
    let manager = frames();
    let first = Rec::default();
    spawn(&manager, "fr-4", Some(Box::new(first.clone())));
    let second = Rec::default();
    manager
        .attach_viewer("fr-4", Box::new(second.clone()), None)
        .expect("attach");
    echo(&manager, "fr-4", None, "skein-before-exit");
    wait_text(&first, "skein-before-exit");
    manager.write("fr-4", b"exit\r\n").expect("write");
    wait_until("both to see Exit", || first.exited() && second.exited());

    for rec in [&first, &second] {
        let events = rec.events();
        let last_data = events.iter().rposition(|e| matches!(e, Ev::Data(_)));
        let exit = events.iter().position(|e| matches!(e, Ev::Exit(_)));
        assert!(last_data < exit, "Data after Exit: {events:?}");
        assert!(rec.text().contains("skein-before-exit"));
    }
    assert!(manager.exit_code("fr-4").is_some());
    assert!(!manager.is_alive("fr-4"));

    let late = Rec::default();
    let (outcome, _) = manager
        .attach_viewer("fr-4", Box::new(late.clone()), None)
        .expect("late attach");
    assert!(matches!(outcome, AttachOutcome::Exited { .. }));
    let events = late.events();
    assert!(matches!(events.first(), Some(Ev::Grid(_, _))));
    assert!(matches!(events.last(), Some(Ev::Exit(_))));
    assert!(late.text().contains("skein-before-exit"));
    manager.kill("fr-4");
}

#[test]
fn output_with_no_viewer_is_not_wedged_by_the_cursor_query() {
    let manager = frames();
    spawn(&manager, "fr-5", None);
    echo(&manager, "fr-5", None, "skein-unattended");
    // Nothing observes the child until a viewer attaches, so attach until a
    // first frame carries the marker (the child may not have printed yet).
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let rec = Rec::default();
        let (_, id) = manager
            .attach_viewer("fr-5", Box::new(rec.clone()), Some((24, 80)))
            .expect("attach");
        wait_until("a first frame", || !rec.datas().is_empty());
        if rec.datas()[0].contains("skein-unattended") {
            break;
        }
        manager.detach_if("fr-5", id);
        assert!(
            Instant::now() < deadline,
            "the marker never reached the engine"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    manager.kill("fr-5");
}

#[test]
fn a_redraw_request_repaints_that_viewer_in_frames_mode_only() {
    let manager = frames();
    let rec = Rec::default();
    spawn(&manager, "fr-6", None);
    let (_, id) = manager
        .attach_viewer("fr-6", Box::new(rec.clone()), Some((24, 80)))
        .expect("attach");
    wait_until("the first frame", || !rec.datas().is_empty());
    let before = rec.datas().len();
    assert!(manager.request_redraw("fr-6", id));
    wait_until("a redraw frame", || {
        rec.datas().iter().skip(before).any(|d| d.contains(ERASE))
    });
    manager.kill("fr-6");

    let raw = PtyManager::new();
    spawn(&raw, "fr-6r", None);
    assert!(!raw.request_redraw("fr-6r", 1));
    raw.kill("fr-6r");
}

#[test]
fn raw_mode_is_the_default_and_sends_no_grid_or_frames() {
    let manager = PtyManager::new();
    assert_eq!(manager.terminal_mode(), TerminalMode::Raw);
    let rec = Rec::default();
    // No sink at spawn: with one, nothing here would answer ConPTY's cursor
    // query (a real view does); the slot answers while unattended.
    spawn(&manager, "fr-7", None);
    std::thread::sleep(Duration::from_millis(500));
    manager
        .attach("fr-7", Box::new(rec.clone()))
        .expect("attach");
    echo(&manager, "fr-7", None, "skein-raw-guard");
    wait_text(&rec, "skein-raw-guard");
    assert!(rec.grids().is_empty(), "raw mode sent a Grid");
    assert!(
        rec.datas().iter().all(|d| !d.starts_with(BEGIN)),
        "raw mode sent a frame"
    );
    manager.kill("fr-7");
}

#[test]
fn the_engine_is_readable_in_frames_mode_and_absent_in_raw_mode() {
    let manager = frames();
    spawn(&manager, "fr-8", None);
    assert!(manager.screen_text("fr-8").is_some());
    assert!(manager.modes("fr-8").is_some());
    assert!(manager.screen_text("nope").is_none());
    assert!(manager.modes("nope").is_none());
    manager.kill("fr-8");

    let raw = PtyManager::new();
    spawn(&raw, "fr-8r", None);
    assert!(raw.screen_text("fr-8r").is_none());
    assert!(raw.modes("fr-8r").is_none());
    raw.kill("fr-8r");
}

#[test]
fn killing_a_frames_pty_releases_its_hub() {
    let manager = frames();
    let rec = Rec::default();
    spawn(&manager, "fr-8", Some(Box::new(rec.clone())));
    echo(&manager, "fr-8", None, "skein-leak-check");
    wait_text(&rec, "skein-leak-check");
    let hub = manager
        .inner
        .lock()
        .get("fr-8")
        .and_then(|p| p.out.hub_weak())
        .expect("frames hub");
    manager.kill("fr-8");
    // The hub is held by the scheduler thread, which ends only when the
    // reader sees EOF, which needs the master to have been closed.
    wait_until("the hub to be released", || hub.upgrade().is_none());
}
