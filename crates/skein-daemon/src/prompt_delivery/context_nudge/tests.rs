use super::*;

const L: u64 = 100;

fn m(id: Option<i64>, armed: bool) -> Mark {
    Mark {
        last_row_id: id,
        armed,
    }
}

fn due(tokens: u64, id: i64) -> Decision {
    Decision::Due {
        tokens,
        limit: L,
        after: m(Some(id), false),
    }
}

#[test]
fn rules_table() {
    type Case = (
        &'static str,
        Option<Mark>,
        Option<(i64, u64)>,
        Option<u64>,
        Decision,
    );
    let cases: Vec<Case> = vec![
        (
            "a: no limit, new row",
            Some(m(Some(1), false)),
            Some((2, 500)),
            None,
            Decision::Store(m(Some(2), true)),
        ),
        (
            "a: no limit, same row",
            Some(m(Some(1), true)),
            Some((1, 500)),
            None,
            Decision::Nothing,
        ),
        (
            "a: no limit, no row",
            Some(m(Some(1), true)),
            None,
            None,
            Decision::Nothing,
        ),
        (
            "a: no limit, no mark",
            None,
            Some((3, 500)),
            None,
            Decision::Store(m(Some(3), true)),
        ),
        (
            "a: no limit, no mark, no row",
            None,
            None,
            None,
            Decision::Store(m(None, true)),
        ),
        (
            "b: unseen, no rows",
            None,
            None,
            Some(L),
            Decision::Store(m(None, true)),
        ),
        (
            "c: unseen, below",
            None,
            Some((3, 50)),
            Some(L),
            Decision::Store(m(Some(3), true)),
        ),
        (
            "c: unseen, over",
            None,
            Some((3, 150)),
            Some(L),
            Decision::Store(m(Some(3), false)),
        ),
        (
            "c: unseen, at limit",
            None,
            Some((3, L)),
            Some(L),
            Decision::Store(m(Some(3), false)),
        ),
        (
            "d: no newest",
            Some(m(Some(1), true)),
            None,
            Some(L),
            Decision::Nothing,
        ),
        (
            "d: same row",
            Some(m(Some(1), true)),
            Some((1, 500)),
            Some(L),
            Decision::Nothing,
        ),
        (
            "e: new below",
            Some(m(Some(1), false)),
            Some((2, 99)),
            Some(L),
            Decision::Store(m(Some(2), true)),
        ),
        (
            "e: new over, armed",
            Some(m(Some(1), true)),
            Some((2, 150)),
            Some(L),
            due(150, 2),
        ),
        (
            "e: new at limit, armed",
            Some(m(Some(1), true)),
            Some((2, L)),
            Some(L),
            due(L, 2),
        ),
        (
            "e: new over, disarmed",
            Some(m(Some(1), false)),
            Some((2, 150)),
            Some(L),
            Decision::Store(m(Some(2), false)),
        ),
        (
            "first row after none",
            Some(m(None, true)),
            Some((2, 150)),
            Some(L),
            due(150, 2),
        ),
    ];
    for (name, mark, newest, limit, want) in cases {
        assert_eq!(decide(mark, newest, limit), want, "{name}");
    }
}

/// Apply a decision the way the caller does: store, or store after send.
fn apply(mark: Option<Mark>, d: Decision) -> (Option<Mark>, bool) {
    match d {
        Decision::Nothing => (mark, false),
        Decision::Store(n) => (Some(n), false),
        Decision::Due { after, .. } => (Some(after), true),
    }
}

#[test]
fn full_sequence() {
    let mut mark = None;
    let mut sent = Vec::new();
    for (id, t) in [(1, 10), (2, 50), (3, 120), (4, 130), (5, 40), (6, 101)] {
        let (next, due) = apply(mark, decide(mark, Some((id, t)), Some(L)));
        mark = next;
        sent.push(due);
    }
    assert_eq!(sent, [false, false, true, false, false, true]);
}

#[test]
fn lowered_limit_waits_for_a_new_row() {
    let mark = Some(m(Some(1), true));
    // Limit lowered below the current size: same row, no nudge.
    assert_eq!(decide(mark, Some((1, 80)), Some(50)), Decision::Nothing);
    // The next row is over the new limit: due.
    assert_eq!(
        decide(mark, Some((2, 80)), Some(50)),
        Decision::Due {
            tokens: 80,
            limit: 50,
            after: m(Some(2), false)
        }
    );
}

#[test]
fn undelivered_nudge_is_due_again() {
    let mark = Some(m(Some(1), true));
    let first = decide(mark, Some((2, 150)), Some(L));
    // Caller did not store `after`; the same inputs decide the same.
    assert_eq!(decide(mark, Some((2, 150)), Some(L)), first);
}

#[test]
fn render_cases() {
    assert_eq!(
        render(None, 150_000, 120_000),
        "Your context is at 150000 tokens, over the limit of 120000 set in Skein. Tell the user it is time to hand over or start a fresh session. Do not compact or restart on your own."
    );
    assert_eq!(render(Some("  \n\t"), 1, 2), render(None, 1, 2));
    assert_eq!(render(Some("{tokens}/{limit} {tokens}"), 7, 9), "7/9 7");
    assert_eq!(render(Some("no placeholders"), 7, 9), "no placeholders");
}
