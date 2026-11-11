//! Who is calling, and whether they may.
//!
//! Two independent checks, in this order:
//!
//! 1. **Origin.** The MCP spec requires a local HTTP server to validate
//!    it, because a page in the user's browser can otherwise reach
//!    127.0.0.1 through DNS rebinding. A tool talking to us has no
//!    `Origin` at all; a browser always sends one. So: absent is fine,
//!    a localhost origin is fine, anything else is refused.
//! 2. **Bearer token.** The token resolves to exactly one room, and
//!    that resolution *is* the authorisation — no request names a room,
//!    so there is nothing to tamper with.
//!
//! `X-Skein-Harness` is attribution only. It says whose reply this is,
//! never what may be read: a wrong or missing value costs the comment
//! its byline and nothing else.

use crate::db::{Database, Room, TokenLookup};

/// Header the harness sends its own id in, for attribution.
pub const HARNESS_HEADER: &str = "x-skein-harness";

/// Everything a verb needs to know about its caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub room_id: String,
    /// The room's worktree. `None` for a room created without a folder,
    /// where the git-backed verbs cannot work and say so.
    pub cwd: Option<String>,
    /// The calling harness, when it identified itself and the room
    /// actually contains it.
    pub harness_id: Option<String>,
    /// How that harness should read in the review pane — "claude · main".
    pub harness_label: Option<String>,
}

/// Why a call was refused. Each variant maps to one HTTP status, and
/// each status means something different to the caller — "revoked" and
/// "never existed" are not the same problem (#176).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// No `Authorization: Bearer …`.
    Missing,
    /// A token, but not one we ever minted.
    Unknown,
    /// Minted, then rotated out from under the holder.
    Revoked,
    /// The token is good; the room it names is gone.
    NoRoom,
    /// The room was closed while the agent still held a review open.
    Archived,
    /// An `Origin` that is not localhost — see the module docs.
    BadOrigin,
    /// The room's stored blob will not parse. Loud on purpose.
    Corrupt(String),
}

impl AuthError {
    pub fn status(&self) -> u16 {
        match self {
            Self::Missing | Self::Unknown => 401,
            Self::Revoked | Self::BadOrigin => 403,
            Self::NoRoom => 404,
            Self::Archived => 410,
            Self::Corrupt(_) => 500,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::Missing => {
                "no bearer token — send Authorization: Bearer $SKEIN_REVIEW_TOKEN".to_owned()
            }
            Self::Unknown => "unknown review token".to_owned(),
            Self::Revoked => "this review token has been rotated; restart the harness".to_owned(),
            Self::NoRoom => "the room this token belongs to no longer exists".to_owned(),
            Self::Archived => "this room has been archived; its review is closed".to_owned(),
            Self::BadOrigin => "cross-origin requests are refused".to_owned(),
            Self::Corrupt(e) => format!("the room's stored record is unreadable: {e}"),
        }
    }
}

/// Reject a browser-driven request. Absent, `null`, `http://localhost:*`
/// and `http://127.0.0.1:*` (and their https/ipv6 spellings) pass.
pub fn check_origin(origin: Option<&str>) -> Result<(), AuthError> {
    let Some(origin) = origin else { return Ok(()) };
    let origin = origin.trim();
    if origin.is_empty() || origin == "null" {
        return Ok(());
    }
    if matches!(host_of(origin), "localhost" | "127.0.0.1" | "[::1]") {
        Ok(())
    } else {
        Err(AuthError::BadOrigin)
    }
}

/// The host part of an origin, port and scheme removed. A bracketed
/// IPv6 literal keeps its brackets, which is what makes the `[::1]`
/// comparison above a plain string match.
fn host_of(origin: &str) -> &str {
    let rest = origin.split_once("://").map_or(origin, |(_, r)| r);
    if let Some(end) = rest.strip_prefix('[').and_then(|r| r.find(']')) {
        return &rest[..=end + 1];
    }
    let end = rest.find([':', '/']).unwrap_or(rest.len());
    &rest[..end]
}

/// Pull the token out of an `Authorization` header value.
pub fn bearer(header: Option<&str>) -> Option<&str> {
    let value = header?.trim();
    let (scheme, token) = value.split_once(' ')?;
    if scheme.eq_ignore_ascii_case("bearer") {
        let token = token.trim();
        (!token.is_empty()).then_some(token)
    } else {
        None
    }
}

/// Resolve a token to the room it may act on.
pub fn authenticate(
    db: &Database,
    token: Option<&str>,
    harness_hint: Option<&str>,
) -> Result<Caller, AuthError> {
    let Some(token) = token else {
        return Err(AuthError::Missing);
    };
    let room_id = match db.room_for_token(token).map_err(AuthError::Corrupt)? {
        TokenLookup::Unknown => return Err(AuthError::Unknown),
        TokenLookup::Revoked => return Err(AuthError::Revoked),
        TokenLookup::Active { room_id } => room_id,
    };
    let room = db
        .room_by_id(&room_id)
        .map_err(AuthError::Corrupt)?
        .ok_or(AuthError::NoRoom)?;
    if room.archived.is_some() {
        return Err(AuthError::Archived);
    }
    let (harness_id, harness_label) = identify_harness(&room, harness_hint);
    Ok(Caller {
        room_id,
        cwd: room.cwd,
        harness_id,
        harness_label,
    })
}

/// Match the `X-Skein-Harness` value against the room's own harness
/// list. An id the room does not contain is kept as attribution but
/// gets no label — better a byline we cannot pretty-print than one we
/// invented.
fn identify_harness(room: &Room, hint: Option<&str>) -> (Option<String>, Option<String>) {
    let hint = hint.map(str::trim).filter(|h| !h.is_empty());
    let Some(hint) = hint else {
        return (None, None);
    };
    let label = room
        .harnesses
        .iter()
        .find(|h| h.id == hint)
        .map(|h| format!("{} · {}", h.kind, h.name));
    (Some(hint.to_owned()), label)
}

/// First 8 characters of a token, for logs. The rest never leaves the
/// database.
pub fn token_prefix(token: &str) -> String {
    token.chars().take(8).collect()
}
