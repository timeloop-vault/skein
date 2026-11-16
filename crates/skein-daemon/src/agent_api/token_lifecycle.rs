//! A room's agent token lives as long as the room is open (#734, design D1).
//!
//! One [`RoomStore`] observer sees every applied batch, whichever path made
//! it (an agent verb, a client's `rooms_apply`, `RemoveRoom`), and revokes the
//! tokens of each room that left the open set. A reopened room mints a fresh
//! token at its next spawn (`ensure_room_token`).

use std::collections::HashSet;
use std::sync::Arc;

use skein_proto::rooms::Room;

use crate::db::Database;
use crate::room_store::RoomStore;

fn open_ids(rooms: &[Room]) -> HashSet<String> {
    rooms
        .iter()
        .filter(|r| r.archived.is_none())
        .map(|r| r.id.clone())
        .collect()
}

/// Register the revoking observer. It keeps no state: on every batch it reads
/// the store's current rooms and revokes the token of every room that still
/// holds one but is not open. That is correct when it is registered before the
/// rooms are loaded (the load runs no observers) and whatever order observers
/// of concurrent batches run in (the batch argument is not trusted).
///
/// A room id absent from the store counts as closed. That is only safe after
/// the first load, and a batch is only ever observed after one: `apply`
/// refuses until `fill`/`load_with` has run (`apply_locked`'s `loaded` check).
///
/// The observer holds the store weakly: the store owns its observers.
pub fn revoke_tokens_of_closed_rooms(rooms: &Arc<RoomStore>, db: &Arc<Database>) {
    let store = Arc::downgrade(rooms);
    let db = Arc::clone(db);
    rooms.observe(move |_batch| {
        let Some(store) = store.upgrade() else {
            return;
        };
        // Holders first, open rooms second: a token is minted at spawn, from a
        // stored record, so every holder listed here was in the store before
        // `open` is read. The other order could revoke a room created and
        // spawned by a concurrent batch in between.
        let holders = match db.rooms_with_live_tokens() {
            Ok(ids) => ids,
            Err(e) => {
                tracing::warn!(error = %e, "agent api: listing rooms with live tokens failed");
                return;
            }
        };
        let open = open_ids(&store.rooms());
        for id in holders.into_iter().filter(|id| !open.contains(id)) {
            match db.revoke_agent_tokens(Some(&id), crate::review::now_ms()) {
                Ok(n) => tracing::info!(room_id = %id, revoked = n, "agent api: room closed, token revoked"),
                Err(e) => tracing::warn!(room_id = %id, error = %e, "agent api: revoking a closed room's token failed"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_api::auth::{self, AuthError};
    use crate::agent_api::tests::{agent_api_state_with_rooms, fixture, room};
    use skein_proto::room_mutations::RoomMutation;

    fn open_store(db: &Arc<Database>, ids: &[&str]) -> Arc<RoomStore> {
        let store = crate::agent_api::tests::store_with(db, ids);
        revoke_tokens_of_closed_rooms(&store, db);
        store
    }

    fn archive(store: &RoomStore, id: &str, at: Option<i64>) {
        let mut r = store.rooms().into_iter().find(|r| r.id == id).unwrap();
        r.archived = at;
        store
            .apply("test", vec![RoomMutation::UpdateRoom { room: r }])
            .unwrap();
    }

    #[test]
    fn archiving_a_room_revokes_its_token_and_only_its_own() {
        let f = fixture();
        let store = open_store(&f.db, &["r1", "r2"]);
        let t1 = f.db.ensure_room_token("r1", 1).unwrap();
        let t2 = f.db.ensure_room_token("r2", 1).unwrap();
        archive(&store, "r1", Some(5));
        assert!(matches!(
            auth::authenticate(&f.db, Some(&t1), None),
            Err(AuthError::Revoked)
        ));
        assert!(auth::authenticate(&f.db, Some(&t2), None).is_ok());
    }

    #[test]
    fn removing_a_room_revokes_its_token() {
        let f = fixture();
        let store = open_store(&f.db, &["r1", "r2"]);
        let t1 = f.db.ensure_room_token("r1", 1).unwrap();
        let t2 = f.db.ensure_room_token("r2", 1).unwrap();
        store
            .apply(
                "test",
                vec![RoomMutation::RemoveRoom {
                    room_id: "r1".to_owned(),
                }],
            )
            .unwrap();
        assert!(matches!(
            auth::authenticate(&f.db, Some(&t1), None),
            Err(AuthError::Revoked)
        ));
        assert!(auth::authenticate(&f.db, Some(&t2), None).is_ok());
    }

    #[test]
    fn a_reopened_room_gets_a_fresh_working_token() {
        let f = fixture();
        let store = open_store(&f.db, &["r1"]);
        let old = f.db.ensure_room_token("r1", 1).unwrap();
        archive(&store, "r1", Some(5));
        archive(&store, "r1", None);
        let new = f.db.ensure_room_token("r1", 10).unwrap();
        assert_ne!(old, new);
        assert!(auth::authenticate(&f.db, Some(&new), None).is_ok());
        assert!(matches!(
            auth::authenticate(&f.db, Some(&old), None),
            Err(AuthError::Revoked)
        ));
    }

    #[test]
    fn a_batch_that_leaves_a_room_open_does_not_touch_its_token() {
        let f = fixture();
        let store = open_store(&f.db, &["r1"]);
        let t1 = f.db.ensure_room_token("r1", 1).unwrap();
        let mut r = store.rooms().remove(0);
        r.name = "renamed".to_owned();
        store
            .apply("test", vec![RoomMutation::UpdateRoom { room: r }])
            .unwrap();
        assert!(auth::authenticate(&f.db, Some(&t1), None).is_ok());
    }

    /// An observer registered on an EMPTY store, then filled with `ids`, as
    /// `Daemon::build` does before the first rooms load.
    fn late_filled_store(db: &Arc<Database>, ids: &[&str]) -> Arc<RoomStore> {
        let store = crate::agent_api::tests::store_with(db, &[]);
        revoke_tokens_of_closed_rooms(&store, db);
        let rooms: Vec<_> = ids.iter().map(|id| room(id, vec![])).collect();
        crate::agent_api::tests::save(db, &rooms);
        store.fill(rooms).unwrap();
        store
    }

    #[test]
    fn the_first_batch_after_a_late_fill_revokes_an_archived_room() {
        let f = fixture();
        let store = late_filled_store(&f.db, &["r1", "r2"]);
        let t1 = f.db.ensure_room_token("r1", 1).unwrap();
        let t2 = f.db.ensure_room_token("r2", 1).unwrap();
        archive(&store, "r1", Some(5));
        assert!(matches!(
            auth::authenticate(&f.db, Some(&t1), None),
            Err(AuthError::Revoked)
        ));
        assert!(auth::authenticate(&f.db, Some(&t2), None).is_ok());
    }

    #[test]
    fn the_first_batch_after_a_late_fill_revokes_a_removed_room() {
        let f = fixture();
        let store = late_filled_store(&f.db, &["r1", "r2"]);
        let t1 = f.db.ensure_room_token("r1", 1).unwrap();
        let t2 = f.db.ensure_room_token("r2", 1).unwrap();
        store
            .apply(
                "test",
                vec![RoomMutation::RemoveRoom {
                    room_id: "r1".to_owned(),
                }],
            )
            .unwrap();
        assert!(matches!(
            auth::authenticate(&f.db, Some(&t1), None),
            Err(AuthError::Revoked)
        ));
        assert!(auth::authenticate(&f.db, Some(&t2), None).is_ok());
    }

    #[test]
    fn a_token_minted_after_a_reopen_survives_a_later_unrelated_batch() {
        let f = fixture();
        let store = late_filled_store(&f.db, &["r1", "r2"]);
        let old = f.db.ensure_room_token("r1", 1).unwrap();
        archive(&store, "r1", Some(5));
        archive(&store, "r1", None);
        let new = f.db.ensure_room_token("r1", 10).unwrap();
        archive(&store, "r2", Some(6));
        assert!(matches!(
            auth::authenticate(&f.db, Some(&old), None),
            Err(AuthError::Revoked)
        ));
        assert!(auth::authenticate(&f.db, Some(&new), None).is_ok());
    }

    #[test]
    fn an_open_rooms_token_survives_many_batches() {
        let f = fixture();
        let store = late_filled_store(&f.db, &["r1"]);
        let t1 = f.db.ensure_room_token("r1", 1).unwrap();
        for i in 0..4 {
            let mut r = store.rooms().remove(0);
            r.name = format!("name {i}");
            store
                .apply("test", vec![RoomMutation::UpdateRoom { room: r }])
                .unwrap();
            assert!(auth::authenticate(&f.db, Some(&t1), None).is_ok());
        }
    }

    #[test]
    fn the_test_state_exposes_the_attached_store_and_no_runner() {
        let f = fixture();
        let (state, store) = agent_api_state_with_rooms(&f, &["r1"]);
        assert!(Arc::ptr_eq(state.room_store().unwrap(), &store));
        assert!(state.runner().is_none());
    }
}
