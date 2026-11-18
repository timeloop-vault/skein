//! The `url` / `harnessUrl` fields of the room verbs (#812).

use super::create_room_tests::{create_room_args, git_env};
use super::tests::{agent_api_state, caller_for, fixture, harness, room, save};
use super::verbs::{self, MailContext};

fn two_harness_state(scheme: Option<&str>) -> (super::state::AgentApiState, super::tests::Fixture) {
    let f = fixture();
    save(
        &f.db,
        &[room(
            "r1",
            vec![
                harness("h1", "claude", "main"),
                harness("h2", "shell", "sh"),
            ],
        )],
    );
    let mut state = agent_api_state(&f);
    state.link_scheme = scheme.map(str::to_owned);
    (state, f)
}

#[tokio::test]
async fn list_rooms_carries_the_profile_scheme_link() {
    let (state, f) = two_harness_state(Some("skein-dev"));
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let out = verbs::list_rooms(
        &state,
        &caller,
        &verbs::ListRoomsArgs::default(),
        &MailContext::permissive(),
    )
    .await
    .unwrap();
    assert_eq!(out.rooms[0].url.as_deref(), Some("skein-dev://room/r1"));
    let v = serde_json::to_value(&out.rooms[0]).unwrap();
    assert_eq!(v["url"], "skein-dev://room/r1");
}

#[tokio::test]
async fn get_room_links_the_room_and_each_harness() {
    let (state, _f) = two_harness_state(Some("skein-dev"));
    let out = verbs::get_room(
        &state,
        &verbs::GetRoomArgs {
            room: "r1".to_owned(),
        },
    )
    .await
    .unwrap();
    assert_eq!(out.url.as_deref(), Some("skein-dev://room/r1"));
    let urls: Vec<_> = out.harnesses.iter().map(|h| h.url.as_deref()).collect();
    assert_eq!(
        urls,
        [
            Some("skein-dev://room/r1?harness=h1"),
            Some("skein-dev://room/r1?harness=h2")
        ]
    );
}

#[tokio::test]
async fn list_harnesses_links_each_harness() {
    let (state, _f) = two_harness_state(Some("skein-local"));
    let out = verbs::list_harnesses(&state, &verbs::ListHarnessesArgs::default())
        .await
        .unwrap();
    let urls: Vec<_> = out.harnesses.iter().map(|h| h.url.as_deref()).collect();
    assert_eq!(
        urls,
        [
            Some("skein-local://room/r1?harness=h1"),
            Some("skein-local://room/r1?harness=h2")
        ]
    );
}

#[tokio::test]
async fn without_a_scheme_no_url_is_emitted() {
    let (state, f) = two_harness_state(None);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let rooms = verbs::list_rooms(
        &state,
        &caller,
        &verbs::ListRoomsArgs::default(),
        &MailContext::permissive(),
    )
    .await
    .unwrap();
    assert_eq!(rooms.rooms[0].url, None);
    assert!(
        serde_json::to_value(&rooms.rooms[0])
            .unwrap()
            .get("url")
            .is_none()
    );
    let got = verbs::get_room(
        &state,
        &verbs::GetRoomArgs {
            room: "r1".to_owned(),
        },
    )
    .await
    .unwrap();
    assert!(serde_json::to_value(&got).unwrap().get("url").is_none());
    assert!(got.harnesses.iter().all(|h| h.url.is_none()));
    let hs = verbs::list_harnesses(&state, &verbs::ListHarnessesArgs::default())
        .await
        .unwrap();
    assert!(hs.harnesses.iter().all(|h| h.url.is_none()));
}

#[tokio::test]
async fn create_room_returns_room_and_harness_links() {
    let mut env = git_env();
    env.state.link_scheme = Some("skein-dev".to_owned());
    let out = verbs::create_room(
        &env.state,
        &env.caller,
        &create_room_args("link me"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap();
    assert_eq!(
        out.url.as_deref(),
        Some(format!("skein-dev://room/{}", out.room_id).as_str())
    );
    assert_eq!(
        out.harness_url.as_deref(),
        Some(
            format!(
                "skein-dev://room/{}?harness={}",
                out.room_id, out.harness_id
            )
            .as_str()
        )
    );
    let v = serde_json::to_value(&out).unwrap();
    assert_eq!(v["harnessUrl"], out.harness_url.clone().unwrap());
}

#[tokio::test]
async fn create_room_without_a_scheme_has_no_links() {
    let env = git_env();
    let out = verbs::create_room(
        &env.state,
        &env.caller,
        &create_room_args("no links"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap();
    assert!(out.url.is_none() && out.harness_url.is_none());
}
