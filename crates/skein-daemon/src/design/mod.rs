//! The design preview server: a room's worktree served over HTTP so a
//! sandboxed iframe can render it (#433, recon §2).
//!
//! A SECOND `127.0.0.1:0` listener was chosen over the agent-API one.
//! Preview routes are unauthenticated by design: an iframe cannot send
//! a bearer header, so the capability is the unguessable token in the
//! path, and responses must carry `Access-Control-Allow-Origin: *`. A
//! separate listener keeps both off `/api` and `/mcp`, so no preview bug
//! can reach an agent verb, and the agent API's CORS stance stays
//! untouched.
//!
//! Tokens live in memory only. That is how "revoked on boot" holds:
//! nothing survives a restart, so a URL that leaked into a log is dead
//! by the next launch. The room's folder is resolved at REQUEST time
//! from the database, never cached, so a repointed room follows.

mod change_filter;
mod device;
pub mod entries;
mod rewrite;
mod serve;

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::db::Database;

pub use serve::{MAX_SERVED, resolve_under, serve};

pub struct PreviewState {
    db: Arc<Database>,
    /// token → room id.
    tokens: Mutex<HashMap<String, String>>,
    /// room id → worktree-relative path → digest of the raw bytes last
    /// served (#434). What an element placement is stamped against.
    served: Mutex<HashMap<String, HashMap<String, String>>>,
}

/// Most distinct files remembered per room. Past it a new path is not
/// recorded and its stamp falls back to disk, which is the weaker
/// direction but bounded.
const MAX_SERVED_PER_ROOM: usize = 500;

impl PreviewState {
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            tokens: Mutex::new(HashMap::new()),
            served: Mutex::new(HashMap::new()),
        }
    }

    /// Remember the digest of the bytes just served for `rel`.
    fn record_served(&self, token: &str, rel: &str, digest: String) {
        let Some(room) = self.room_for(token) else {
            return;
        };
        let mut served = self.served.lock();
        let files = served.entry(room).or_default();
        if files.len() < MAX_SERVED_PER_ROOM || files.contains_key(rel) {
            files.insert(rel.to_owned(), digest);
        }
    }

    /// A copy of what was last served for the room.
    pub fn served_digests(&self, room_id: &str) -> HashMap<String, String> {
        self.served.lock().get(room_id).cloned().unwrap_or_default()
    }

    /// The room's preview token, minting one the first time. Idempotent
    /// for as long as the process lives.
    pub fn mint(&self, room_id: &str) -> String {
        let mut tokens = self.tokens.lock();
        if let Some((token, _)) = tokens.iter().find(|(_, room)| room.as_str() == room_id) {
            return token.clone();
        }
        // Same 256 bits as `Database::ensure_room_token`.
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        tokens.insert(token.clone(), room_id.to_owned());
        token
    }

    fn room_for(&self, token: &str) -> Option<String> {
        self.tokens.lock().get(token).cloned()
    }

    /// The folder to serve for a token, looked up now.
    fn root_for(&self, token: &str) -> Option<String> {
        let room_id = self.room_for(token)?;
        match self.db.room_by_id(&room_id) {
            Ok(room) => room?.cwd,
            Err(e) => {
                tracing::warn!(error = %e, "design preview: room lookup failed");
                None
            }
        }
    }
}

/// FNV-1a, 64-bit. A change detector, not a security boundary; the same
/// family `skein-review` hashes content with.
pub struct Fnv(pub u64);

impl Fnv {
    pub fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    pub fn update(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

impl Default for Fnv {
    fn default() -> Self {
        Self::new()
    }
}

/// The digest of a file's raw bytes. The preview server records this
/// for what it serves and the freshness check recomputes it from disk,
/// so the two must stay the same form.
pub fn digest_bytes(bytes: &[u8]) -> String {
    let mut h = Fnv::new();
    h.update(bytes);
    format!("{:016x}", h.0)
}
