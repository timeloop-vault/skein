//! Which live rows a two-session stream records (#678): the root's user
//! prompt, never a known child's, and every tool/cost/reasoning row.

use skein_harness::opencode::v2::{V2Event, drain_sse_data, parse_action_event, parse_event};

use super::actions::rows_for;
use super::translate::V2Translator;
use crate::db::action_kind;
use crate::harness_actions_opencode::v2::V2ActionTracker;

const SSE_F: &str = include_str!("../../../../skein-harness/src/opencode/v2/fixtures/store-F.sse");
const ROOT: &str = "ses_e11423fa0ffelpQoAfc4t2pakl";
const CHILD: &str = "ses_e11422fd4ffeQ2X4Ze2OS4FGCk";

/// (session, kind) of every recorded row, with the translator's knowledge
/// applied the way the connection does it.
fn run(skip_child_created: bool) -> Vec<(String, &'static str)> {
    let mut buf = SSE_F.as_bytes().to_vec();
    let mut tr = V2Translator::new(Some(ROOT));
    let mut tracker = V2ActionTracker::default();
    let mut out = Vec::new();
    for payload in drain_sse_data(&mut buf) {
        if let Some(event) = parse_event(&payload) {
            let skip = skip_child_created
                && matches!(&event, V2Event::SessionCreated { session_id, .. } if session_id == CHILD);
            if !skip {
                let _ = tr.translate(event);
            }
        }
        let Some(action) = parse_action_event(&payload) else {
            continue;
        };
        let root = tr.root_of(&action.session_id);
        for row in rows_for(&mut tracker, &action, root) {
            out.push((action.session_id.clone(), row.kind));
        }
    }
    out
}

fn count(rows: &[(String, &str)], sid: &str, kind: &str) -> usize {
    rows.iter().filter(|(s, k)| s == sid && *k == kind).count()
}

#[test]
fn known_child_prompt_is_dropped_but_its_work_is_recorded() {
    let rows = run(false);
    assert!(count(&rows, ROOT, action_kind::USER_PROMPT) >= 1);
    assert_eq!(count(&rows, CHILD, action_kind::USER_PROMPT), 0);
    assert!(count(&rows, CHILD, action_kind::TURN_COST) >= 1);
    assert!(
        rows.iter()
            .any(|(s, k)| s == CHILD && (*k == action_kind::TOOL_CALL || *k == action_kind::PATCH))
    );
    assert!(count(&rows, ROOT, action_kind::TURN_COST) >= 1);
}

#[test]
fn unknown_session_prompt_is_shown() {
    // The child's `session.created` never seen: its status is unknown, so
    // (as in v1) the prompt is emitted rather than risk a dead-looking feed.
    let rows = run(true);
    assert!(count(&rows, CHILD, action_kind::USER_PROMPT) >= 1);
}
