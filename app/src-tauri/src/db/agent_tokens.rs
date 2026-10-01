//! Per-room agent API bearer tokens. Split out of db.rs (#454).

use rusqlite::{OptionalExtension, params};

use super::Database;

/// What a bearer token resolved to (issue #213).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenLookup {
    /// No such token was ever minted.
    Unknown,
    /// Minted, then rotated out from under the holder.
    Revoked,
    Active {
        room_id: String,
    },
}

impl Database {
    /// The room's live bearer token, minting one the first time it is
    /// asked for. Idempotent: called on every harness spawn.
    pub fn ensure_room_token(&self, room_id: &str, now_ms: i64) -> Result<String, String> {
        let conn = self.conn.lock();
        let existing: Option<String> = conn
            .query_row(
                "SELECT token FROM agent_tokens \
                 WHERE room_id = ?1 AND revoked_ms IS NULL \
                 ORDER BY created_ms DESC LIMIT 1",
                params![room_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(token) = existing {
            return Ok(token);
        }
        // 256 bits from two v4 UUIDs rather than a `rand` dependency:
        // uuid already sources them from the OS CSPRNG, and this is the
        // only place in the tree that needs unguessable bytes.
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        conn.execute(
            "INSERT INTO agent_tokens (token, room_id, created_ms, revoked_ms) \
             VALUES (?1, ?2, ?3, NULL)",
            params![token, room_id, now_ms],
        )
        .map_err(|e| e.to_string())?;
        Ok(token)
    }

    /// Revoke live tokens — one room's, or every room's when `room_id`
    /// is `None`. Returns how many were revoked.
    ///
    /// Called with `None` on every boot. A token's lifetime is the
    /// lifetime of the Skein process that handed it out: PTYs die with
    /// the app, so nothing legitimate is still holding one, and a token
    /// that leaked into an old transcript or log stops working the next
    /// time Skein starts.
    pub fn revoke_agent_tokens(&self, room_id: Option<&str>, now_ms: i64) -> Result<usize, String> {
        let conn = self.conn.lock();
        match room_id {
            Some(id) => conn.execute(
                "UPDATE agent_tokens SET revoked_ms = ?2 \
                 WHERE room_id = ?1 AND revoked_ms IS NULL",
                params![id, now_ms],
            ),
            None => conn.execute(
                "UPDATE agent_tokens SET revoked_ms = ?1 WHERE revoked_ms IS NULL",
                params![now_ms],
            ),
        }
        .map_err(|e| e.to_string())
    }

    /// Resolve a bearer token. The three outcomes are deliberately
    /// distinct — "revoked" and "never existed" mean different things
    /// to whoever is holding it.
    pub fn room_for_token(&self, token: &str) -> Result<TokenLookup, String> {
        let conn = self.conn.lock();
        let row: Option<(String, Option<i64>)> = conn
            .query_row(
                "SELECT room_id, revoked_ms FROM agent_tokens WHERE token = ?1",
                params![token],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        Ok(match row {
            None => TokenLookup::Unknown,
            Some((_, Some(_))) => TokenLookup::Revoked,
            Some((room_id, None)) => TokenLookup::Active { room_id },
        })
    }
}
