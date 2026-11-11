//! `list_rooms` filtering, ordering and paging (#612).

use super::state::AgentApiState;
use super::tests::{agent_api_state, caller_for, fixture, harness, room, save};
use super::verbs::{self, ListRoomsArgs, ListRoomsOut, MailContext, VerbError};
use crate::db::Room;

fn open_room(id: &str) -> Room {
    room(id, vec![harness(&format!("h-{id}"), "claude", "main")])
}

fn archived_room(id: &str, archived: i64, retired: bool) -> Room {
    let mut r = open_room(id);
    r.archived = Some(archived);
    r.retired = retired.then_some(archived);
    r
}

/// Persisted order o1, o2, o3 (o3 newest-added), then archived a1 (100),
/// a3 (200), a2 (200) and retired r1 (300).
fn seeded() -> (super::tests::Fixture, AgentApiState) {
    let f = fixture();
    save(
        &f.db,
        &[
            open_room("o1"),
            archived_room("a1", 100, false),
            open_room("o2"),
            archived_room("a3", 200, false),
            archived_room("r1", 300, true),
            open_room("o3"),
            archived_room("a2", 200, false),
        ],
    );
    let state = agent_api_state(&f);
    (f, state)
}

async fn call(
    f: &super::tests::Fixture,
    state: &AgentApiState,
    args: ListRoomsArgs,
) -> Result<ListRoomsOut, VerbError> {
    let caller = caller_for(&f.db, "o1", Some("h-o1"));
    verbs::list_rooms(state, &caller, &args, &MailContext::permissive()).await
}

fn ids(out: &ListRoomsOut) -> Vec<&str> {
    out.rooms.iter().map(|r| r.room_id.as_str()).collect()
}

#[tokio::test]
async fn default_hides_archived_and_retired_rooms() {
    let (f, state) = seeded();
    let out = call(&f, &state, ListRoomsArgs::default()).await.unwrap();
    assert_eq!(ids(&out), ["o3", "o2", "o1"]);
    assert!(!out.has_more);
}

#[tokio::test]
async fn include_archived_shows_archived_but_not_retired() {
    let (f, state) = seeded();
    let args = ListRoomsArgs {
        include_archived: true,
        ..Default::default()
    };
    let out = call(&f, &state, args).await.unwrap();
    assert_eq!(ids(&out), ["o3", "o2", "o1", "a2", "a3", "a1"]);
    assert!(out.rooms.iter().all(|r| !r.retired));
}

#[tokio::test]
async fn include_retired_shows_everything_and_orders_newest_first() {
    let (f, state) = seeded();
    let args = ListRoomsArgs {
        include_retired: true,
        ..Default::default()
    };
    let out = call(&f, &state, args).await.unwrap();
    // Open rooms newest-added first, then archived by archived desc,
    // ties by id ascending.
    assert_eq!(ids(&out), ["o3", "o2", "o1", "r1", "a2", "a3", "a1"]);
    let r1 = out.rooms.iter().find(|r| r.room_id == "r1").unwrap();
    assert!(r1.archived && r1.retired);
}

#[tokio::test]
async fn created_by_me_with_the_default_hides_an_archived_child() {
    let f = fixture();
    let mut live = open_room("live");
    live.created_by = Some(super::room_listing_tests::created_by("o1", "h-o1"));
    let mut done = archived_room("done", 5, false);
    done.created_by = Some(super::room_listing_tests::created_by("o1", "h-o1"));
    save(&f.db, &[open_room("o1"), live, done]);
    let state = agent_api_state(&f);
    let args = ListRoomsArgs {
        created_by: Some("me".to_owned()),
        ..Default::default()
    };
    let out = call(&f, &state, args).await.unwrap();
    assert_eq!(ids(&out), ["live"]);
}

#[tokio::test]
async fn lifecycle_filters_after_visibility_and_refuses_unknown_values() {
    let (f, state) = seeded();
    let with = |values: &[&str], include_archived: bool| ListRoomsArgs {
        include_archived,
        lifecycle: Some(values.iter().map(|s| (*s).to_owned()).collect()),
        ..Default::default()
    };
    // No webview in this test: every open room reads "unknown".
    let out = call(&f, &state, with(&["unknown"], true)).await.unwrap();
    assert_eq!(ids(&out), ["o3", "o2", "o1"]);
    let out = call(&f, &state, with(&["archived"], true)).await.unwrap();
    assert_eq!(ids(&out), ["a2", "a3", "a1"]);
    let out = call(&f, &state, with(&["archived"], false)).await.unwrap();
    assert!(out.rooms.is_empty(), "archived rooms are not visible here");
    let out = call(&f, &state, with(&["running"], true)).await.unwrap();
    assert_eq!(out.rooms.len(), 0);
    let out = call(&f, &state, with(&[], false)).await.unwrap();
    assert_eq!(ids(&out), ["o3", "o2", "o1"], "empty list = no filter");

    let err = call(&f, &state, with(&["runing"], false))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("runing")),
        "{err:?}"
    );
}

#[tokio::test]
async fn fields_summary_drops_the_bulky_fields_and_bad_values_are_refused() {
    let f = fixture();
    let mut child = open_room("child");
    child.cwd = Some("/work/child".to_owned());
    child.repo_root = Some("/work".to_owned());
    child.branch = Some("feat".to_owned());
    child.created_by = Some(crate::db::CreatedBy {
        prompt_first_line: Some("do it".to_owned()),
        base_sha: Some("abc".to_owned()),
        ..super::room_listing_tests::created_by("o1", "h-o1")
    });
    save(&f.db, &[open_room("o1"), child]);
    let state = agent_api_state(&f);
    let fields = |v: &str| ListRoomsArgs {
        fields: Some(v.to_owned()),
        ..Default::default()
    };

    let full = call(&f, &state, fields("full")).await.unwrap();
    let row = full.rooms.iter().find(|r| r.room_id == "child").unwrap();
    assert!(row.cwd.is_some() && row.created_by.is_some() && row.lead_harness_id.is_some());
    assert!(row.base_sha.is_some() && row.prompt_first_line.is_some() && row.repo_root.is_some());

    let sum = call(&f, &state, fields("summary")).await.unwrap();
    let row = sum.rooms.iter().find(|r| r.room_id == "child").unwrap();
    assert!(row.cwd.is_none() && row.repo_root.is_none() && row.created_by.is_none());
    assert!(row.lead_harness_id.is_none() && row.base_sha.is_none());
    assert!(row.prompt_first_line.is_none());
    assert_eq!(row.branch.as_deref(), Some("feat"));
    assert_eq!(row.harness_count, 1);

    let err = call(&f, &state, fields("tiny")).await.unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("tiny")),
        "{err:?}"
    );
}

#[tokio::test]
async fn limit_boundaries_set_has_more() {
    let (f, state) = seeded();
    let limited = |n: u32| ListRoomsArgs {
        include_retired: true,
        limit: Some(n),
        ..Default::default()
    };
    let exact = call(&f, &state, limited(7)).await.unwrap();
    assert_eq!(exact.rooms.len(), 7);
    assert!(!exact.has_more, "limit == count leaves nothing more");
    let short = call(&f, &state, limited(6)).await.unwrap();
    assert_eq!(short.rooms.len(), 6);
    assert!(short.has_more);
}

#[tokio::test]
async fn paging_with_limit_two_returns_every_room_exactly_once() {
    let (f, state) = seeded();
    let mut seen: Vec<String> = Vec::new();
    let mut since: Option<String> = None;
    let mut pages = 0;
    loop {
        let out = call(
            &f,
            &state,
            ListRoomsArgs {
                include_retired: true,
                limit: Some(2),
                since: since.clone(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        pages += 1;
        seen.extend(out.rooms.iter().map(|r| r.room_id.clone()));
        if !out.has_more {
            break;
        }
        since = out.rooms.last().map(|r| r.room_id.clone());
        assert!(pages < 20, "paging must terminate");
    }
    assert_eq!(pages, 4);
    assert_eq!(seen, ["o3", "o2", "o1", "r1", "a2", "a3", "a1"]);
}

#[tokio::test]
async fn an_unknown_or_hidden_since_is_refused() {
    let (f, state) = seeded();
    for since in ["ghost", "a1"] {
        // "a1" exists but is archived, so the default listing hides it.
        let err = call(
            &f,
            &state,
            ListRoomsArgs {
                since: Some(since.to_owned()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, VerbError::Refused(m) if m.contains("since")),
            "{err:?}"
        );
    }
}

#[tokio::test]
async fn limit_zero_means_the_default_and_a_huge_limit_is_clamped() {
    let f = fixture();
    let rooms: Vec<Room> = (0..520)
        .map(|i| open_room(&format!("o{}", i + 1)))
        .collect();
    save(&f.db, &rooms);
    let state = agent_api_state(&f);
    let with = |limit: u32| ListRoomsArgs {
        limit: Some(limit),
        ..Default::default()
    };
    let zero = call(&f, &state, with(0)).await.unwrap();
    assert_eq!(zero.rooms.len(), 50);
    assert!(zero.has_more);
    let none = call(&f, &state, ListRoomsArgs::default()).await.unwrap();
    assert_eq!(none.rooms.len(), 50);
    let huge = call(&f, &state, with(100_000)).await.unwrap();
    assert_eq!(huge.rooms.len(), 500);
    assert!(huge.has_more);
}

#[tokio::test]
async fn a_cursor_room_that_no_longer_matches_the_lifecycle_filter_still_pages() {
    // The lifecycle filter is applied after the cursor is resolved, so
    // a cursor room whose lifecycle changed between pages (here: it
    // does not match the filter at all) neither refuses nor repeats.
    let (f, state) = seeded();
    let first = call(
        &f,
        &state,
        ListRoomsArgs {
            include_archived: true,
            limit: Some(1),
            lifecycle: Some(vec!["unknown".to_owned()]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(ids(&first), ["o3"]);
    assert!(first.has_more);
    // "a2" is archived, so it never matches ["unknown"], yet it is a
    // valid cursor in the visible order: the next matching room after
    // it is none, with nothing repeated.
    let after_archived = call(
        &f,
        &state,
        ListRoomsArgs {
            include_archived: true,
            lifecycle: Some(vec!["unknown".to_owned()]),
            since: Some("a2".to_owned()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(after_archived.rooms.len(), 0);
    assert!(!after_archived.has_more);
    // And from the filtered page's cursor onward, no repeats.
    let next = call(
        &f,
        &state,
        ListRoomsArgs {
            include_archived: true,
            lifecycle: Some(vec!["unknown".to_owned()]),
            since: Some("o3".to_owned()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(ids(&next), ["o2", "o1"]);
}
