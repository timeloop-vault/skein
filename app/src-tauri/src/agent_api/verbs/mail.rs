//! The mailbox verbs (#327): send, read and page back through messages
//! between harnesses.

use serde::{Deserialize, Serialize};

use super::mail_routing::{find_room, record_mail_actions, resolve_mail_target};
use super::shared::MAX_MESSAGE_BYTES;
use super::{VerbError, VerbResult, internal};
use crate::agent_api::auth::Caller;
use crate::db::{Database, HarnessMessageRow, Room};
use crate::review::now_ms;

/// How many messages one room may send in a rolling minute (#327), and
/// the window it is measured over — a runaway loop has to hit a wall
/// before it can flood a sibling harness.
const SEND_RATE_LIMIT: i64 = 30;
const SEND_RATE_WINDOW_MS: i64 = 60_000;

/// How many unread messages one harness may accumulate before a sender
/// is refused (#327) — an inbox nobody is reading is not a queue, it is
/// a leak.
const MAX_UNREAD_MESSAGES: i64 = 100;

/// `message_history`'s (#364) default and cap on how many rows one call
/// returns. A director paging back through a thread after a compaction
/// reads pages, not a firehose; `limit: 0` is treated as "use the
/// default" rather than refused — an agent-supplied 0 almost always
/// means "no opinion", not "give me nothing".
const DEFAULT_HISTORY_LIMIT: u32 = 100;
const MAX_HISTORY_LIMIT: u32 = 500;

/// Whether the named agent, run as `kind` in `cwd`, would see Skein's
/// review MCP tools at all — and, by extension, this mailbox's own
/// tools, since they ride the same connection. A plain function
/// pointer rather than a boxed closure: the real implementation
/// (`crate::agents::agent_sees_mcp`) needs no captures, so the mail
/// verbs pay no lifetime parameter for a seam that exists purely so
/// tests can drive both answers without agent files on disk.
pub type AgentSeesMcp = fn(kind: &str, agent: &str, cwd: &str) -> bool;

/// The parts of the user's spawn settings the mailbox verbs need to
/// decide anything, passed in rather than read from live state (#327).
/// A plain `Copy` value keeps `send_message`/`read_messages` testable
/// with no settings file and no Tauri runtime; the MCP and HTTP call
/// sites build one from the live `SpawnSettings` on every request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailPolicy {
    pub messaging_enabled: bool,
    pub claude_injected: bool,
    pub opencode_injected: bool,
}

impl MailPolicy {
    /// Every reachable kind, messaging on — a fresh install's defaults,
    /// and what a test reaches for when the case under test isn't about
    /// the policy at all. Production builds one from the live
    /// `SpawnSettings` instead (`http::mail_context`), so this is
    /// test-only.
    #[cfg(test)]
    pub fn permissive() -> Self {
        Self {
            messaging_enabled: true,
            claude_injected: true,
            opencode_injected: true,
        }
    }
}

/// [`MailPolicy`] plus the disk-lookup seam, bundled for the MCP and
/// HTTP layers that carry both from one request to `call_tool`.
///
/// `app` (#329) is the emitter for the `message_in`/`message_out`
/// `harness_actions` rows `send_message` writes — `None` in a unit
/// test (no Tauri runtime) or when the agent API's own `AgentApiState`
/// has none (`for_test`), in which case the rows still get written,
/// only the live broadcast is skipped, same as every other emit in
/// this codebase. No `Copy`: an `AppHandle` is a handle, not a value.
#[derive(Clone)]
pub struct MailContext {
    pub policy: MailPolicy,
    pub agent_sees_mcp: AgentSeesMcp,
    pub app: Option<tauri::AppHandle>,
}

impl MailContext {
    /// The same defaults as [`MailPolicy::permissive`], for tests that
    /// have no opinion on messaging at all.
    #[cfg(test)]
    pub fn permissive() -> Self {
        Self {
            policy: MailPolicy::permissive(),
            agent_sees_mcp: |_, _, _| true,
            app: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SendMessageArgs {
    /// A harness id (searched across every room) or a room id, resolved
    /// at send time to that room's lead harness.
    pub to: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageOut {
    pub message_id: String,
    pub to_room_id: String,
    pub to_harness_id: String,
    pub to_room_name: String,
    pub to_harness_name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ReadMessagesArgs {
    /// Return the whole history, oldest first, instead of only what is
    /// unread. Still marks anything unread as read.
    #[serde(default)]
    pub include_read: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessage {
    pub id: String,
    pub from_room_id: String,
    /// `None` when the sending room no longer exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_room_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_harness_id: Option<String>,
    /// `None` when the sender had no `X-Skein-Harness`, or its room or
    /// that harness no longer exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_harness_name: Option<String>,
    pub body: String,
    pub created_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadMessagesOut {
    pub messages: Vec<AgentMessage>,
    /// How many of `messages` were unread before this call and just got
    /// marked — the signal the MCP/HTTP layer uses to decide whether to
    /// fire `skein://mail-changed`, without re-deriving it from the
    /// list (a message already read before `include_read` asked for
    /// history must not count again).
    pub newly_marked_read: usize,
}

/// `message_history`'s (#364) paging cursor: a JSON number is a
/// millisecond `created_ms` (exclusive), a JSON string is a message id
/// (strictly after that message in `(created_ms, rowid)` order — see
/// [`Database::harness_message_history`]). Never ambiguous: message ids
/// are UUIDs, never numeric, so `untagged` always picks the right arm.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum HistorySince {
    Ms(i64),
    MessageId(String),
}

/// `message_history`'s (#364) `direction` argument. Defaults to `In` —
/// the same scope `read_messages` has always had — so a caller that
/// never heard of this argument still gets the inbox it expects.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HistoryDirection {
    #[default]
    In,
    Out,
    Both,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MessageHistoryArgs {
    /// A room id or harness id — only messages exchanged with that
    /// counterpart. Optional — default: no filter.
    #[serde(default)]
    pub with: Option<String>,
    #[serde(default)]
    pub since: Option<HistorySince>,
    /// Optional — default and cap: [`DEFAULT_HISTORY_LIMIT`] /
    /// [`MAX_HISTORY_LIMIT`].
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub direction: Option<HistoryDirection>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryMessage {
    pub id: String,
    /// `"inbound"` or `"outbound"` — never both, even for the one row
    /// shape that could read as either (a sibling harness in the
    /// caller's own room mailing the caller): see
    /// [`Database::harness_message_history`] for how that is resolved.
    pub direction: &'static str,
    pub from_room_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_room_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_harness_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_harness_name: Option<String>,
    pub to_room_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_room_name: Option<String>,
    pub to_harness_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_harness_name: Option<String>,
    pub body: String,
    pub created_ms: i64,
    /// When the recipient read this message — for an outbound row, the
    /// signal that the counterpart has seen it; for an inbound row,
    /// whatever an earlier `read_messages` call already left behind.
    /// This call never sets it: `message_history` never marks anything
    /// read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageHistoryOut {
    pub messages: Vec<HistoryMessage>,
    /// Whether more rows exist after the last one returned here — the
    /// signal for forward paging with `since` set to that row's id.
    pub has_more: bool,
}

/// Send a message to another harness's mailbox.
///
/// `X-Skein-Harness` is attribution here exactly as it is everywhere
/// else in this API (#213): addressing a harness inside a room is
/// routing, not a security boundary, so any harness in any room may
/// send to any other. The lead harness is resolved *now*, not read
/// later, so the stored row names a concrete recipient rather than a
/// room whose lead harness might change before anyone reads it.
///
/// On success, also records one `message_in` `harness_actions` row for
/// the recipient and one `message_out` row for the sender (#329) — the
/// Live Context feed's only way to show who talked to whom, since a
/// mailbox write touches nothing a harness's own transcript tail would
/// ever see. Neither carries the message body. This is the one place
/// both `mcp.rs` and `http.rs` route a send through, so it is the only
/// place that needs to write them. A failure to record either row is
/// logged and swallowed — the send itself already succeeded.
pub fn send_message(
    db: &Database,
    caller: &Caller,
    args: &SendMessageArgs,
    policy: MailPolicy,
    agent_sees_mcp: AgentSeesMcp,
    app: Option<&tauri::AppHandle>,
) -> VerbResult<SendMessageOut> {
    if !policy.messaging_enabled {
        return Err(VerbError::Refused(
            "agent messaging is turned off in Settings".into(),
        ));
    }
    if args.body.is_empty() {
        return Err(VerbError::Refused("a message needs a body".into()));
    }
    if args.body.len() > MAX_MESSAGE_BYTES {
        return Err(VerbError::Refused(format!(
            "a message body is capped at {MAX_MESSAGE_BYTES} bytes; this one is {} bytes",
            args.body.len()
        )));
    }
    let to = args.to.trim();
    if to.is_empty() {
        return Err(VerbError::Refused("send_message needs a to".into()));
    }

    let rooms = db.all_rooms().map_err(internal)?;
    let (room, harness) = resolve_mail_target(&rooms, to, policy, agent_sees_mcp)?;

    // The rate check, the unread check and the insert below are three
    // separate lock acquisitions, so concurrent sends can push a count
    // slightly past its cap. Accepted: these are anti-runaway caps, not
    // a security boundary.
    let now = now_ms();
    let sent = db
        .harness_messages_sent_since(&caller.room_id, now - SEND_RATE_WINDOW_MS)
        .map_err(internal)?;
    if sent >= SEND_RATE_LIMIT {
        return Err(VerbError::Refused(format!(
            "rate limit: this room has sent {sent} messages in the last minute \
             (cap {SEND_RATE_LIMIT})"
        )));
    }
    let unread = db
        .unread_harness_message_count(&room.id, &harness.id)
        .map_err(internal)?;
    if unread >= MAX_UNREAD_MESSAGES {
        return Err(VerbError::Refused(format!(
            "{} already has {unread} unread messages (cap {MAX_UNREAD_MESSAGES}); \
             it needs to read before it can receive more",
            harness.name
        )));
    }

    let row = HarnessMessageRow {
        id: uuid::Uuid::new_v4().to_string(),
        room_id: room.id.clone(),
        harness_id: harness.id.clone(),
        from_room_id: caller.room_id.clone(),
        from_harness_id: caller.harness_id.clone(),
        body: args.body.clone(),
        created_ms: now,
        read_ms: None,
    };
    db.insert_harness_message(&row).map_err(internal)?;

    record_mail_actions(db, app, &rooms, caller, &room, &harness, &row);

    Ok(SendMessageOut {
        message_id: row.id,
        to_room_id: room.id,
        to_harness_id: harness.id,
        to_room_name: room.name,
        to_harness_name: harness.name,
    })
}

/// Every unread message for the calling harness, oldest first — or, with
/// `include_read`, the whole history. Marks the unread ones read.
pub fn read_messages(
    db: &Database,
    caller: &Caller,
    args: &ReadMessagesArgs,
    policy: MailPolicy,
) -> VerbResult<ReadMessagesOut> {
    if !policy.messaging_enabled {
        return Err(VerbError::Refused(
            "agent messaging is turned off in Settings".into(),
        ));
    }
    let Some(harness_id) = caller.harness_id.as_deref() else {
        return Err(VerbError::Refused(
            "read_messages needs X-Skein-Harness to say which harness is asking".into(),
        ));
    };
    let include_read = args.include_read.unwrap_or(false);
    let rows = if include_read {
        db.all_harness_messages(&caller.room_id, harness_id)
    } else {
        db.unread_harness_messages(&caller.room_id, harness_id)
    }
    .map_err(internal)?;

    let now = now_ms();
    let mut to_mark = Vec::new();
    let rooms = db.all_rooms().map_err(internal)?;
    let messages: Vec<AgentMessage> = rows
        .into_iter()
        .map(|m| {
            let read_ms = if let Some(r) = m.read_ms {
                Some(r)
            } else {
                to_mark.push(m.id.clone());
                Some(now)
            };
            let sender_room = find_room(&rooms, &m.from_room_id);
            let from_room_name = sender_room.map(|r| r.name.clone());
            let from_harness_name = m.from_harness_id.as_deref().and_then(|hid| {
                sender_room
                    .and_then(|r| r.harnesses.iter().find(|h| h.id == hid))
                    .map(|h| format!("{} · {}", h.kind, h.name))
            });
            AgentMessage {
                id: m.id,
                from_room_id: m.from_room_id,
                from_room_name,
                from_harness_id: m.from_harness_id,
                from_harness_name,
                body: m.body,
                created_ms: m.created_ms,
                read_ms,
            }
        })
        .collect();

    if !to_mark.is_empty() {
        db.mark_harness_messages_read(&to_mark, now)
            .map_err(internal)?;
    }

    Ok(ReadMessagesOut {
        newly_marked_read: to_mark.len(),
        messages,
    })
}

/// Whether `m` sits in the caller's own inbox — the same scope
/// `read_messages` reads (`room_id`/`harness_id`, the recipient
/// columns). `false` with no harness on the caller, same as
/// `read_messages`'s outright refusal in that case, but this function
/// itself never refuses — [`message_history`] decides what to do with
/// that.
fn is_inbound(
    m: &HarnessMessageRow,
    caller_room_id: &str,
    caller_harness_id: Option<&str>,
) -> bool {
    caller_harness_id.is_some_and(|h| m.room_id == caller_room_id && m.harness_id == h)
}

/// Whether `m` sits in the caller's ROOM's outbox (`from_room_id`, not
/// `from_harness_id` — #364's brief is explicit that outbound scope is
/// per room, covering every harness in it, not only the caller's own).
/// Excludes anything [`is_inbound`] already claims, which is what keeps
/// a sibling harness's message to the caller classified inbound rather
/// than counted twice.
fn is_outbound(
    m: &HarnessMessageRow,
    caller_room_id: &str,
    caller_harness_id: Option<&str>,
) -> bool {
    m.from_room_id == caller_room_id && !is_inbound(m, caller_room_id, caller_harness_id)
}

/// [`HarnessMessageRow`] → [`HistoryMessage`], enriching both ends the
/// way [`read_messages`] already enriches the sender — `rooms` is one
/// `all_rooms` read shared across a whole page, not refetched per row.
fn render_history_message(
    m: &HarnessMessageRow,
    rooms: &[Room],
    caller_room_id: &str,
    caller_harness_id: Option<&str>,
) -> HistoryMessage {
    let direction = if is_inbound(m, caller_room_id, caller_harness_id) {
        "inbound"
    } else {
        "outbound"
    };

    let sender_room = find_room(rooms, &m.from_room_id);
    let from_room_name = sender_room.map(|r| r.name.clone());
    let from_harness_name = m.from_harness_id.as_deref().and_then(|hid| {
        sender_room
            .and_then(|r| r.harnesses.iter().find(|h| h.id == hid))
            .map(|h| format!("{} · {}", h.kind, h.name))
    });

    let recipient_room = find_room(rooms, &m.room_id);
    let to_room_name = recipient_room.map(|r| r.name.clone());
    let to_harness_name = recipient_room
        .and_then(|r| r.harnesses.iter().find(|h| h.id == m.harness_id))
        .map(|h| format!("{} · {}", h.kind, h.name));

    HistoryMessage {
        id: m.id.clone(),
        direction,
        from_room_id: m.from_room_id.clone(),
        from_room_name,
        from_harness_id: m.from_harness_id.clone(),
        from_harness_name,
        to_room_id: m.room_id.clone(),
        to_room_name,
        to_harness_id: m.harness_id.clone(),
        to_harness_name,
        body: m.body.clone(),
        created_ms: m.created_ms,
        read_ms: m.read_ms,
    }
}

/// A harness's (or, with `direction: "out"`/`"both"`, its whole room's)
/// mail history — filtered by counterpart, bounded by `since`/`limit`,
/// oldest first (#364). Unlike [`read_messages`], this **never** marks
/// anything read: it exists so a director recovering after its own
/// context is compacted can page back through exactly the thread it
/// needs without touching the unread badge that call maintains.
pub fn message_history(
    db: &Database,
    caller: &Caller,
    args: &MessageHistoryArgs,
    policy: MailPolicy,
) -> VerbResult<MessageHistoryOut> {
    if !policy.messaging_enabled {
        return Err(VerbError::Refused(
            "agent messaging is turned off in Settings".into(),
        ));
    }
    let direction = args.direction.unwrap_or_default();
    let include_inbound = matches!(direction, HistoryDirection::In | HistoryDirection::Both);
    let include_outbound = matches!(direction, HistoryDirection::Out | HistoryDirection::Both);

    if include_inbound && caller.harness_id.is_none() {
        return Err(VerbError::Refused(
            "message_history needs X-Skein-Harness to say which harness's inbox to \
             read — pass direction: \"out\" for the room's own outbox without one"
                .into(),
        ));
    }

    let limit = match args.limit {
        None | Some(0) => DEFAULT_HISTORY_LIMIT,
        Some(n) => n.min(MAX_HISTORY_LIMIT),
    };

    let (since_ms, since_message_id) = match &args.since {
        None => (None, None),
        Some(HistorySince::Ms(ms)) => (Some(*ms), None),
        Some(HistorySince::MessageId(id)) => {
            let row = db.harness_message_by_id(id).map_err(internal)?;
            let visible = row.is_some_and(|m| {
                is_inbound(&m, &caller.room_id, caller.harness_id.as_deref())
                    || is_outbound(&m, &caller.room_id, caller.harness_id.as_deref())
            });
            if !visible {
                return Err(VerbError::Refused(
                    "since does not name a message in this room's own mail history".into(),
                ));
            }
            (None, Some(id.as_str()))
        }
    };

    let rows = db
        .harness_message_history(
            &caller.room_id,
            caller.harness_id.as_deref(),
            include_inbound,
            include_outbound,
            args.with.as_deref(),
            since_ms,
            since_message_id,
            limit + 1,
        )
        .map_err(internal)?;

    let has_more = rows.len() > limit as usize;
    let rooms = db.all_rooms().map_err(internal)?;
    let messages = rows
        .into_iter()
        .take(limit as usize)
        .map(|m| render_history_message(&m, &rooms, &caller.room_id, caller.harness_id.as_deref()))
        .collect();

    Ok(MessageHistoryOut { messages, has_more })
}

/// A harness's unread-mail summary (#329) — how many, and which rooms
/// they're from, for a badge that has to answer both without reading
/// (`read_messages` marks read, which this must never do).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailUnread {
    pub count: i64,
    /// Distinct sender room names, first-seen order (oldest unread
    /// message first). A room that no longer exists falls back to its
    /// id rather than being dropped — the same "over-report rather than
    /// hide" call review baselines make (#221).
    pub from_room_names: Vec<String>,
}

/// Pure derivation over already-fetched rows, so it's testable with no
/// `Database` and reusable by both the Tauri command (`mail_unread`,
/// #329) and, if ever needed, a verb. Never marks anything read — the
/// caller must fetch `messages` with `unread_harness_messages`, not
/// `all_harness_messages`.
pub fn unread_mail(messages: &[HarnessMessageRow], rooms: &[Room]) -> MailUnread {
    let mut from_room_names = Vec::new();
    for m in messages {
        let name = find_room(rooms, &m.from_room_id)
            .map_or_else(|| m.from_room_id.clone(), |r| r.name.clone());
        if !from_room_names.contains(&name) {
            from_room_names.push(name);
        }
    }
    MailUnread {
        count: i64::try_from(messages.len()).unwrap_or(i64::MAX),
        from_room_names,
    }
}
