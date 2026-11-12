//! Wire types shared by skeind and its clients (issue #580). The `hello`
//! handshake is #584; no wire framing is defined here.
//!
//! What lives here: pure data only. A type belongs in this crate when it
//! derives serde `Serialize`/`Deserialize` and has no logic beyond std-only
//! impls; anything with server-side behaviour (defaulting, validation,
//! storage) stays in `skein-daemon`, which re-exports these at their
//! original paths. The types are versioned as a set by [`PROTO_VERSION`].

pub mod harness_kind;
pub mod harness_log;
pub mod resume;
pub mod room_mutations;
pub mod rooms;
pub mod spawn_env;

/// The protocol version #584's `hello` handshake will carry.
pub const PROTO_VERSION: u32 = 1;
