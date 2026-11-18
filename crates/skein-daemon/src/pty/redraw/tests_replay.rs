//! The gate, second version, Rust half (#807): each #793 recording is replayed
//! through `AlacrittyEngine` and the redraw encoder, and what the daemon would
//! send is pinned in `fixtures/NAME.frames.json`. A JavaScript test feeds those
//! frames to `@xterm/headless` and compares with xterm.js fed the raw stream.
//! The format is in `fixtures/README.md`.
//!
//! Regenerate after an intended encoder or engine change and review the diff:
//!
//!     SKEIN_UPDATE_ENGINE_DUMPS=1 cargo test -p skein-daemon replay
//!
//! Like the engine replay this is not real time: a manual clock that never
//! advances, no `flush_sync`, so a synchronized update ends only at the
//! stream's own `?2026l`.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::pty::engine::replay_tests::{FIXTURES, dir, load};
use crate::pty::engine::tests::ManualClock;
use crate::pty::engine::{AlacrittyEngine, TerminalEngine};

use super::ViewerState;

enum Item {
    Frame { after: usize, bytes: Vec<u8> },
    Resize { after: usize, cols: u16, rows: u16 },
}

struct CutOut {
    name: String,
    after: usize,
    cols: u16,
    rows: u16,
    full: Vec<u8>,
    skipped: Vec<u8>,
}

struct Generated {
    stream: Vec<Item>,
    cuts: Vec<CutOut>,
    cols: u16,
    rows: u16,
    /// `frame()` calls that returned `None` while a sync update was pending.
    held: usize,
    /// Cuts where a fresh viewer's full redraw, fed to the stand-in client,
    /// did not give the source's screen text.
    mismatches: Vec<String>,
}

fn b64(b: &[u8]) -> String {
    STANDARD.encode(b)
}

/// Replays `name` through the three viewers, running the sanity checks.
fn generate(name: &str) -> Generated {
    let (rec, cuts) = load(name);
    let mut e = ManualClock::new().engine(rec.rows, rec.cols);
    let mut client = AlacrittyEngine::new(rec.rows, rec.cols);
    let mut continuous = ViewerState::new();
    let mut skipped = ViewerState::new();
    let mut stream = Vec::new();
    let mut out = Vec::new();
    let mut held = 0;
    let mut mismatches = Vec::new();
    for (i, ev) in rec.events.iter().enumerate() {
        match ev.kind.as_str() {
            "data" => {
                let bytes = STANDARD.decode(ev.b64.as_ref().expect("b64")).expect("b64");
                e.feed(&bytes);
            }
            "resize" => {
                let (rows, cols) = (ev.rows.expect("rows"), ev.cols.expect("cols"));
                e.resize(rows, cols);
                client.resize(rows, cols);
                stream.push(Item::Resize {
                    after: i,
                    cols,
                    rows,
                });
            }
            k => panic!("unknown event kind {k}"),
        }
        let frame = e.frame(&mut continuous);
        if e.modes().sync_pending {
            assert!(
                frame.is_none(),
                "{name}: a frame inside a sync update, after event {i}"
            );
            held += 1;
        }
        if let Some(bytes) = frame {
            client.feed(&bytes);
            stream.push(Item::Frame { after: i, bytes });
        }
        for c in cuts.iter().filter(|c| c.after_event == i) {
            let at = format!("{name}/{}", c.name);
            let mut fresh = ViewerState::new();
            let full = e.frame(&mut fresh).unwrap_or_default();
            let skip = e.frame(&mut skipped).unwrap_or_default();
            let (rows, cols) = e.size();
            if !e.modes().sync_pending {
                // Compared: the continuous viewer's state against a fresh
                // viewer's. On the alternate screen a fresh viewer knows
                // nothing of the main screen, so those fields come from the
                // continuous one.
                let mut f = fresh.clone();
                if e.modes().alt_screen {
                    f.history_len = continuous.history_len;
                    f.history_entered = continuous.history_entered;
                    f.main_screen.clone_from(&continuous.main_screen);
                }
                assert_eq!(f, continuous, "{at}: viewer state");
                // A fresh viewer's full redraw alone, fed to a stand-in client.
                // Not asserted, but pinned: see `known_stand_in_mismatches`.
                let mut attached = AlacrittyEngine::new(rows, cols);
                attached.feed(&full);
                if attached.screen_text() != e.screen_text() {
                    mismatches.push(format!("full redraw {at}"));
                }
            }
            // Compared: the screen text of the client the continuous stream
            // built against the source's (not the cell dump: the stand-in
            // client keeps a different spacer flag on a wide character at the
            // last column, which the JS gate judges with xterm.js instead).
            if client.screen_text() != e.screen_text() {
                mismatches.push(format!("stream {at}"));
            }
            out.push(CutOut {
                name: c.name.clone(),
                after: i,
                cols,
                rows,
                full,
                skipped: skip,
            });
        }
    }
    Generated {
        stream,
        cuts: out,
        cols: rec.cols,
        rows: rec.rows,
        held,
        mismatches,
    }
}

/// One stream item and one cut per line, LF endings, so a regenerate diffs.
fn render(name: &str, g: &Generated) -> String {
    let mut s = format!(
        "{{\"version\":1,\"fixture\":\"{name}\",\"cols\":{},\"rows\":{},\n\"stream\":[\n",
        g.cols, g.rows
    );
    let items: Vec<String> = g
        .stream
        .iter()
        .map(|it| match it {
            Item::Frame { after, bytes } => format!(
                "{{\"afterEvent\":{after},\"kind\":\"frame\",\"b64\":\"{}\"}}",
                b64(bytes)
            ),
            Item::Resize { after, cols, rows } => format!(
                "{{\"afterEvent\":{after},\"kind\":\"resize\",\"cols\":{cols},\"rows\":{rows}}}"
            ),
        })
        .collect();
    s.push_str(&items.join(",\n"));
    s.push_str("\n],\"cuts\":[\n");
    let cuts: Vec<String> = g
        .cuts
        .iter()
        .map(|c| {
            format!(
                "{{\"name\":{},\"afterEvent\":{},\"cols\":{},\"rows\":{},\"full\":\"{}\",\"skipped\":\"{}\"}}",
                serde_json::to_string(&c.name).expect("name"),
                c.after,
                c.cols,
                c.rows,
                b64(&c.full),
                b64(&c.skipped)
            )
        })
        .collect();
    s.push_str(&cuts.join(",\n"));
    s.push_str("\n]}\n");
    s
}

fn regenerate_hint(name: &str) -> String {
    format!(
        "fixture {name}: regenerate with `SKEIN_UPDATE_ENGINE_DUMPS=1 cargo test -p skein-daemon replay` and review the diff"
    )
}

#[test]
fn frames_match_the_pinned_files() {
    for name in FIXTURES {
        let text = render(name, &generate(name));
        let path = dir().join(format!("{name}.frames.json"));
        if std::env::var_os("SKEIN_UPDATE_ENGINE_DUMPS").is_some() {
            std::fs::write(&path, text).expect("write the frames");
            continue;
        }
        let stored = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("missing {}: {}", path.display(), regenerate_hint(name)));
        // Line by line, so a failure names the first differing item.
        let (a, b): (Vec<&str>, Vec<&str>) = (stored.lines().collect(), text.lines().collect());
        if let Some(i) = a.iter().zip(&b).position(|(x, y)| x != y) {
            panic!("{name}: line {} differs: {}", i + 1, regenerate_hint(name));
        }
        assert_eq!(
            a.len(),
            b.len(),
            "{name}: line count: {}",
            regenerate_hint(name)
        );
    }
}

#[test]
fn nothing_is_cut_inside_a_synchronized_update() {
    // `generate` asserts a None frame whenever sync_pending; here: the
    // scrollback recording does hold frames back, and its cut inside the
    // update sends nothing to a fresh viewer or the skipped one.
    let g = generate("scrollback-session");
    assert!(g.held > 0, "the recording never held a frame");
    let c = g
        .cuts
        .iter()
        .find(|c| c.name == "in-sync-frame")
        .expect("cut");
    assert!(c.full.is_empty() && c.skipped.is_empty());
}

#[test]
fn every_resize_is_listed_once_before_its_frame() {
    for name in FIXTURES {
        let g = generate(name);
        let (rec, _) = load(name);
        let want = rec.events.iter().filter(|e| e.kind == "resize").count();
        let got = g
            .stream
            .iter()
            .filter(|i| matches!(i, Item::Resize { .. }))
            .count();
        assert_eq!(got, want, "{name}");
        for w in g.stream.windows(2) {
            if let [Item::Frame { after: a, .. }, Item::Resize { after: b, .. }] = w {
                assert!(a < b, "{name}: a resize listed after the frame it precedes");
            }
        }
    }
}

#[test]
#[ignore = "prints the summary"]
fn summary() {
    for name in FIXTURES {
        let g = generate(name);
        let frames = g
            .stream
            .iter()
            .filter(|i| matches!(i, Item::Frame { .. }))
            .count();
        println!(
            "{name}: {frames} frames, {} held, {} cuts, mismatches {:?}",
            g.held,
            g.cuts.len(),
            g.mismatches
        );
    }
}

/// Where the stand-in client (alacritty fed the frames) shows other screen
/// text than the source. Pinned so a change either way is noticed. All are
/// one case: after the resize to 80x24 the engine's reflow leaves a wide
/// character starting in the last column (`leading_spacer` false, width 2),
/// which the frame cannot draw (the client would wrap it and leave a ghost, so
/// the encoder writes one blank there, #807): the engine's row shows the
/// character, the stand-in client's shows the blank.
#[test]
fn known_stand_in_mismatches() {
    let got: Vec<String> = FIXTURES
        .iter()
        .flat_map(|n| generate(n).mismatches)
        .collect();
    let want = [
        "full redraw claude-session/resize-80x24",
        "stream claude-session/resize-80x24",
    ];
    assert_eq!(got, want);
}
