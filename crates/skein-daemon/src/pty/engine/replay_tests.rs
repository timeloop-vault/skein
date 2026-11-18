//! Replays the two recorded terminal streams through `AlacrittyEngine` and
//! pins an engine-neutral picture at each named cut in
//! `fixtures/NAME.engine.json` (#793, slice T-1). A JavaScript test compares
//! those pictures with xterm.js later; the format is in `fixtures/README.md`.
//!
//! Regenerate after an intended engine change and review the diff:
//!
//!     SKEIN_UPDATE_ENGINE_DUMPS=1 cargo test -p skein-daemon replay_tests
//!
//! Synchronized updates: the replay is not real time. The engine reads time
//! from a clock it is given, and the replay gives it a manual clock that
//! never advances. The 150 ms deadline therefore never passes, and an update
//! ends only at the stream's own `?2026l`. The replay never calls
//! `flush_sync` and never sleeps, so a slow machine cannot make it flake.

use std::collections::BTreeSet;
use std::path::PathBuf;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;
use serde_json::Value;

use super::TerminalEngine;
use super::compact::{CompactCut, StyleTable, compact};
use super::tests::ManualClock;

#[derive(Deserialize)]
pub(in crate::pty) struct Recording {
    pub(in crate::pty) cols: u16,
    pub(in crate::pty) rows: u16,
    pub(in crate::pty) events: Vec<Event>,
}

#[derive(Deserialize)]
pub(in crate::pty) struct Event {
    pub(in crate::pty) kind: String,
    pub(in crate::pty) b64: Option<String>,
    pub(in crate::pty) cols: Option<u16>,
    pub(in crate::pty) rows: Option<u16>,
}

#[derive(Deserialize)]
pub(in crate::pty) struct Cut {
    pub(in crate::pty) name: String,
    #[serde(rename = "afterEvent")]
    pub(in crate::pty) after_event: usize,
}

pub(in crate::pty) const FIXTURES: [&str; 2] = ["claude-session", "scrollback-session"];

pub(in crate::pty) fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/pty/engine/fixtures")
}

pub(in crate::pty) fn load(name: &str) -> (Recording, Vec<Cut>) {
    let rec = std::fs::read_to_string(dir().join(format!("{name}.json"))).expect("recording");
    let cuts = std::fs::read_to_string(dir().join(format!("{name}.cuts.json"))).expect("cuts");
    (
        serde_json::from_str(&rec).expect("recording parses"),
        serde_json::from_str(&cuts).expect("cuts parse"),
    )
}

struct Replayed {
    cuts: Vec<CompactCut>,
    styles: StyleTable,
    replies: Vec<Vec<u8>>,
    /// The first event after which `rebuilds()` was non-zero.
    first_rebuild: Option<usize>,
}

/// Replays `name`. With `dump_every_event` the engine is also dumped after
/// every event, to show that reading changes nothing.
fn replay(name: &str, dump_every_event: bool) -> Replayed {
    let (rec, cuts) = load(name);
    // A clock that never advances: the replay can never time out an update.
    let mut e = ManualClock::new().engine(rec.rows, rec.cols);
    let mut styles = StyleTable::new();
    let mut scratch = StyleTable::new();
    let mut out = Vec::new();
    let mut replies = Vec::new();
    let mut first_rebuild = None;
    for (i, ev) in rec.events.iter().enumerate() {
        match ev.kind.as_str() {
            "data" => {
                let bytes = STANDARD
                    .decode(ev.b64.as_ref().expect("b64"))
                    .expect("base64");
                let r = e.feed(&bytes);
                if !r.is_empty() {
                    replies.push(r);
                }
            }
            "resize" => e.resize(ev.rows.expect("rows"), ev.cols.expect("cols")),
            k => panic!("unknown event kind {k}"),
        }
        if first_rebuild.is_none() && e.rebuilds() > 0 {
            first_rebuild = Some(i);
        }
        if dump_every_event {
            let _ = compact(&e, &mut scratch, "scratch", i);
        }
        for c in cuts.iter().filter(|c| c.after_event == i) {
            out.push(compact(&e, &mut styles, &c.name, i));
        }
    }
    Replayed {
        cuts: out,
        styles,
        replies,
        first_rebuild,
    }
}

/// The file text: one style and one cut per line, LF endings, fields in
/// struct order, so a regenerate diffs line by line.
fn render(name: &str, r: &Replayed) -> String {
    let mut s = format!("{{\"version\":1,\"fixture\":\"{name}\",\"styles\":[\n");
    let styles: Vec<String> = r
        .styles
        .styles
        .iter()
        .map(|x| serde_json::to_string(x).expect("style"))
        .collect();
    s.push_str(&styles.join(",\n"));
    s.push_str("\n],\"cuts\":[\n");
    let cuts: Vec<String> = r
        .cuts
        .iter()
        .map(|x| serde_json::to_string(x).expect("cut"))
        .collect();
    s.push_str(&cuts.join(",\n"));
    s.push_str("\n]}\n");
    s
}

fn regenerate_hint(name: &str) -> String {
    format!(
        "fixture {name}: regenerate with `SKEIN_UPDATE_ENGINE_DUMPS=1 cargo test -p skein-daemon replay_tests` and review the diff"
    )
}

#[test]
fn engine_pictures_match_the_pinned_files() {
    for name in FIXTURES {
        let r = replay(name, false);
        let text = render(name, &r);
        let path = dir().join(format!("{name}.engine.json"));
        if std::env::var_os("SKEIN_UPDATE_ENGINE_DUMPS").is_some() {
            std::fs::write(&path, text).expect("write the dump");
            continue;
        }
        let stored: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|_| {
                panic!("missing {}: {}", path.display(), regenerate_hint(name))
            }))
            .expect("stored dump parses");
        let fresh: Value = serde_json::from_str(&text).expect("fresh dump parses");
        assert_eq!(
            stored["styles"],
            fresh["styles"],
            "style table differs: {}",
            regenerate_hint(name)
        );
        let stored_cuts = stored["cuts"].as_array().expect("cuts array");
        let fresh_cuts = fresh["cuts"].as_array().expect("cuts array");
        for f in fresh_cuts {
            let cut = f["name"].as_str().expect("cut name");
            let Some(s) = stored_cuts.iter().find(|s| s["name"] == f["name"]) else {
                panic!(
                    "cut {cut} is missing from the stored file: {}",
                    regenerate_hint(name)
                );
            };
            assert!(
                s == f,
                "cut {cut} differs from the stored one: {}",
                regenerate_hint(name)
            );
        }
        assert_eq!(
            stored_cuts.len(),
            fresh_cuts.len(),
            "the stored file has stale cuts: {}",
            regenerate_hint(name)
        );
    }
}

#[test]
fn reading_the_engine_does_not_disturb_it() {
    for name in FIXTURES {
        let quiet = replay(name, false);
        let loud = replay(name, true);
        assert_eq!(
            quiet.cuts, loud.cuts,
            "{name}: cuts differ when dumping every event"
        );
        assert_eq!(
            render(name, &quiet),
            render(name, &loud),
            "{name}: rendered files differ"
        );
    }
}

#[test]
fn the_panic_guard_never_fires_on_real_input() {
    for name in FIXTURES {
        let r = replay(name, false);
        assert_eq!(
            r.first_rebuild, None,
            "{name}: the engine rebuilt itself after this event"
        );
        assert!(
            r.cuts.iter().all(|c| c.rebuilds == 0),
            "{name}: rebuilds at a cut"
        );
    }
}

#[test]
fn a_synchronized_update_is_pending_inside_the_frame_only() {
    let r = replay("scrollback-session", false);
    let pending = |n: &str| {
        r.cuts
            .iter()
            .find(|c| c.name == n)
            .unwrap_or_else(|| panic!("cut {n}"))
            .modes
            .sync_pending
    };
    assert!(
        pending("in-sync-frame"),
        "the update is open inside the frame"
    );
    assert!(
        !pending("after-sync-frame"),
        "the stream's own ?2026l ended it"
    );
    for c in &r.cuts {
        if c.name != "in-sync-frame" {
            assert!(
                !c.modes.sync_pending,
                "unexpected pending update at {}",
                c.name
            );
        }
    }
}

#[test]
fn history_accounting_holds_at_every_cut() {
    for name in FIXTURES {
        let r = replay(name, false);
        let (rec, _) = load(name);
        let first_resize = rec.events.iter().position(|e| e.kind == "resize");
        let mut last = 0;
        for c in &r.cuts {
            // After a resize the count and the contents are unrelated (the
            // documented limit of `history_rows_entered`: a narrowing resize
            // reflows rows into history without counting them; at
            // scrollback-session/resize-60x30, 192 entered and 202 present),
            // so "at least the rows present" holds only before the first one.
            let resized = first_resize.is_some_and(|f| c.after_event >= f);
            assert!(
                resized || c.history_rows_entered >= c.history.len() as u64,
                "{name}/{}: {} rows entered but {} present",
                c.name,
                c.history_rows_entered,
                c.history.len()
            );
            if name == "scrollback-session" {
                assert!(
                    c.history_rows_entered >= last,
                    "{name}/{}: the count went down",
                    c.name
                );
                last = c.history_rows_entered;
            }
        }
    }
}

#[test]
fn the_cuts_files_are_sane() {
    for name in FIXTURES {
        let (rec, cuts) = load(name);
        let mut names = BTreeSet::new();
        let mut prev: Option<usize> = None;
        for c in &cuts {
            assert!(
                names.insert(c.name.clone()),
                "{name}: duplicate cut {}",
                c.name
            );
            assert!(
                c.after_event < rec.events.len(),
                "{name}/{}: out of range",
                c.name
            );
            assert!(
                prev.is_none_or(|p| c.after_event > p),
                "{name}/{}: not increasing",
                c.name
            );
            prev = Some(c.after_event);
        }
    }
}

/// The distinct replies, as lossy text, in order of first appearance.
fn distinct(replies: &[Vec<u8>]) -> Vec<String> {
    let mut seen = Vec::new();
    for r in replies {
        let s = String::from_utf8_lossy(r).replace('\x1b', "\\e");
        if !seen.contains(&s) {
            seen.push(s);
        }
    }
    seen
}

#[test]
fn the_replies_each_fixture_provokes_are_pinned() {
    let got: Vec<(usize, Vec<String>)> = FIXTURES
        .iter()
        .map(|n| {
            let r = replay(n, false);
            (r.replies.len(), distinct(&r.replies))
        })
        .collect();
    let expected: Vec<(usize, Vec<String>)> = vec![PINNED_CLAUDE, PINNED_SCROLLBACK]
        .into_iter()
        .map(|(n, v): (usize, &[&str])| (n, v.iter().map(|s| (*s).to_owned()).collect()))
        .collect();
    assert_eq!(got, expected, "the replies changed: {got:?}");
}

const PINNED_CLAUDE: (usize, &[&str]) = (2, &[r"\e[1;1R", r"\e[?0u"]);
const PINNED_SCROLLBACK: (usize, &[&str]) = (0, &[]);
