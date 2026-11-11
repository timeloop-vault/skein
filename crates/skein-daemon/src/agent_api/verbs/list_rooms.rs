//! `list_rooms` (#356, filtering and paging #612): every room Skein
//! holds, newest first, archived and retired ones hidden by default.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::mail::MailContext;
use super::mail_routing::mail_lead_harness;
use super::room_listing::{
    AgentCreatedBy, HarnessSnapshot, KNOWN_PHASES, LastStatus, harness_snapshots,
};
use super::shared::first_non_empty_line;
use super::{VerbError, VerbResult, internal};
use crate::agent_api::auth::Caller;
use crate::agent_api::state::AgentApiState;
use crate::db::Room;

/// `list_rooms`'s default and cap on how many rooms one call returns,
/// the same shape as `message_history`'s: `limit: 0` means "no opinion"
/// and takes the default rather than being refused.
const DEFAULT_ROOMS_LIMIT: u32 = 50;
const MAX_ROOMS_LIMIT: u32 = 500;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ListRoomsArgs {
    /// The only accepted value is `"me"` — every other non-`None` value
    /// is refused rather than silently ignored, since a typo here
    /// (`"mine"`, `"self"`) would otherwise read as "show me
    /// everything" and a director would never notice its filter never
    /// applied.
    #[serde(default)]
    pub created_by: Option<String>,
    /// Archived rooms are left out unless this is true.
    #[serde(default)]
    pub include_archived: bool,
    /// Retired rooms (archived history, #417) are left out unless this
    /// is true; it implies `include_archived`, since retired is a subset.
    #[serde(default)]
    pub include_retired: bool,
    /// Keep only rooms whose `lifecycle` is in the list: `"archived"`,
    /// `"unknown"` or one of the activity phases. Anything else is
    /// refused (same reasoning as `created_by`). Empty = no filter.
    #[serde(default)]
    pub lifecycle: Option<Vec<String>>,
    /// Rooms per page: `None` or 0 → 50, clamped to 500.
    #[serde(default)]
    pub limit: Option<u32>,
    /// A room id cursor, exclusive: the page starts after it.
    #[serde(default)]
    pub since: Option<String>,
    /// `"full"` (default) or `"summary"` (drops the bulky fields).
    #[serde(default)]
    pub fields: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct RoomSummary {
    pub room_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub archived: bool,
    pub retired: bool,
    pub harness_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<AgentCreatedBy>,
    /// The harness `send_message` would actually reach if addressed to
    /// this room id — `None` when nothing in the room could read mail
    /// at all (see `mail_lead_harness`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lead_harness_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_first_line: Option<String>,
    /// `"archived"`, one of [`KNOWN_PHASES`] (the lead harness's own),
    /// or `"unknown"` — never a guess.
    pub lifecycle: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_status: Option<LastStatus>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct ListRoomsOut {
    pub rooms: Vec<RoomSummary>,
    /// More rooms follow this page; pass the last `room_id` as `since`.
    pub has_more: bool,
    /// The count of rooms Skein holds but could not parse (see
    /// `FindRoomsForPathOut::unreadable_rooms`), so a director reading a
    /// short list here knows whether it might be short for that reason
    /// rather than because it truly created no more.
    pub unreadable_rooms: u32,
}

/// The rooms a call may show, in the stable newest-first order: open
/// rooms first in reverse persisted order (the frontend appends new
/// rooms, so the newest-added leads), then archived ones by `archived`
/// descending, ties by id ascending. Archived-not-retired rooms need
/// `include_archived` or `include_retired`; retired ones need
/// `include_retired`. `rooms` must be in persisted order.
fn visible_ordered(rooms: &[Room], include_archived: bool, include_retired: bool) -> Vec<&Room> {
    let mut out: Vec<&Room> = rooms
        .iter()
        .rev()
        .filter(|r| r.archived.is_none())
        .collect();
    let mut archived: Vec<&Room> = rooms
        .iter()
        .filter(|r| match (r.archived, r.retired) {
            (None, _) => false,
            (Some(_), None) => include_archived || include_retired,
            (Some(_), Some(_)) => include_retired,
        })
        .collect();
    archived.sort_by(|a, b| (Reverse(a.archived), &a.id).cmp(&(Reverse(b.archived), &b.id)));
    out.extend(archived);
    out
}

fn effective_limit(limit: Option<u32>) -> usize {
    let n = match limit {
        None | Some(0) => DEFAULT_ROOMS_LIMIT,
        Some(n) => n.min(MAX_ROOMS_LIMIT),
    };
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// One page of `items`: those after the `since` id (exclusive) that
/// pass `keep`, at most `limit`, and whether more follow. The cursor is
/// resolved against all of `items`, before `keep`, so a cursor room that
/// no longer passes the filter still works; an id not among `items` is
/// refused rather than read as "start over".
fn page_after<T>(
    items: Vec<T>,
    id_of: impl Fn(&T) -> &str,
    since: Option<&str>,
    keep: impl Fn(&T) -> bool,
    limit: usize,
) -> VerbResult<(Vec<T>, bool)> {
    let skip = match since {
        None => 0,
        Some(s) => {
            items.iter().position(|i| id_of(i) == s).ok_or_else(|| {
                VerbError::Refused(format!(
                    "since {s:?} names no room in this listing (it may be hidden by the \
                         current filters)"
                ))
            })? + 1
        }
    };
    let mut page: Vec<T> = items
        .into_iter()
        .skip(skip)
        .filter(|i| keep(i))
        .take(limit + 1)
        .collect();
    let has_more = page.len() > limit;
    page.truncate(limit);
    Ok((page, has_more))
}

fn validate_lifecycle(values: &[String]) -> VerbResult<()> {
    for v in values {
        if v != "archived" && v != "unknown" && !KNOWN_PHASES.contains(&v.as_str()) {
            let valid = KNOWN_PHASES
                .iter()
                .map(|p| format!("{p:?}"))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(VerbError::Refused(format!(
                "unknown lifecycle {v:?} — valid values are \"archived\", \"unknown\", {valid}"
            )));
        }
    }
    Ok(())
}

fn lifecycle_of(
    r: &Room,
    mail: &MailContext,
    phases: &BTreeMap<String, HarnessSnapshot>,
) -> String {
    if r.archived.is_some() {
        return "archived".to_owned();
    }
    mail_lead_harness(r, mail.policy, mail.agent_sees_mcp)
        .and_then(|h| phases.get(&h.id))
        .map_or_else(|| "unknown".to_owned(), |s| s.phase.clone())
}

fn lead_ids(rooms: &[&Room], mail: &MailContext) -> Vec<String> {
    rooms
        .iter()
        .filter(|r| r.archived.is_none())
        .filter_map(|r| {
            mail_lead_harness(r, mail.policy, mail.agent_sees_mcp).map(|h| h.id.clone())
        })
        .collect()
}

/// Rooms Skein holds, across every project, newest first and paged.
/// Archived rooms are **hidden by default**: pass `include_archived`
/// to see them (a director checking "did that room land and get
/// archived" after its context was compacted), and `include_retired`
/// for retired history too (it implies archived). `created_by: "me"`
/// narrows to the rooms the *caller's* room created; `lifecycle`
/// keeps only rooms in the named states; `limit`/`since` page (a page
/// reports `has_more`, its last `room_id` is the next `since`);
/// `fields: "summary"` drops the bulky per-room fields.
pub async fn list_rooms(
    state: &AgentApiState,
    caller: &Caller,
    args: &ListRoomsArgs,
    mail: &MailContext,
) -> VerbResult<ListRoomsOut> {
    let db = &state.db;
    if let Some(cb) = args.created_by.as_deref()
        && cb != "me"
    {
        return Err(VerbError::Refused(format!(
            "unknown created_by {cb:?} — the only supported value is \"me\""
        )));
    }
    let lifecycle_filter = args.lifecycle.as_deref().filter(|l| !l.is_empty());
    if let Some(l) = lifecycle_filter {
        validate_lifecycle(l)?;
    }
    let summary = match args.fields.as_deref() {
        None | Some("full") => false,
        Some("summary") => true,
        Some(other) => {
            return Err(VerbError::Refused(format!(
                "unknown fields {other:?} — use \"full\" or \"summary\""
            )));
        }
    };
    let limit = effective_limit(args.limit);

    let rooms = db.all_rooms().map_err(internal)?;
    let unreadable_rooms = db.unreadable_room_count().map_err(internal)?;

    let mut visible = visible_ordered(&rooms, args.include_archived, args.include_retired);
    if args.created_by.is_some() {
        visible.retain(|r| {
            r.created_by
                .as_ref()
                .is_some_and(|c| c.room_id == caller.room_id)
        });
    }

    // A lifecycle filter needs every visible open room's phase before it
    // can page; without one, only the page's rooms are asked about.
    let mut phases = BTreeMap::new();
    if lifecycle_filter.is_some() {
        phases = harness_snapshots(state, &lead_ids(&visible, mail)).await;
    }
    let (page, has_more) = page_after(
        visible,
        |r| r.id.as_str(),
        args.since.as_deref(),
        |r| lifecycle_filter.is_none_or(|w| w.contains(&lifecycle_of(r, mail, &phases))),
        limit,
    )?;
    if lifecycle_filter.is_none() {
        phases = harness_snapshots(state, &lead_ids(&page, mail)).await;
    }

    let mut out = Vec::with_capacity(page.len());
    for r in page {
        let lead = mail_lead_harness(r, mail.policy, mail.agent_sees_mcp);
        let last_status = db
            .latest_message_from_room(&r.id, &caller.room_id)
            .map_err(internal)?
            .map(|m| LastStatus {
                first_line: first_non_empty_line(&m.body),
                created_ms: m.created_ms,
            });
        let mut row = RoomSummary {
            room_id: r.id.clone(),
            name: r.name.clone(),
            cwd: r.cwd.clone(),
            repo_root: r.repo_root.clone(),
            branch: r.branch.clone(),
            archived: r.archived.is_some(),
            retired: r.retired.is_some(),
            harness_count: r.harnesses.len(),
            created_by: r.created_by.as_ref().map(AgentCreatedBy::from),
            lead_harness_id: lead.map(|h| h.id.clone()),
            base_sha: r.created_by.as_ref().and_then(|c| c.base_sha.clone()),
            prompt_first_line: r
                .created_by
                .as_ref()
                .and_then(|c| c.prompt_first_line.clone()),
            lifecycle: lifecycle_of(r, mail, &phases),
            last_status,
        };
        if summary {
            row.cwd = None;
            row.repo_root = None;
            row.created_by = None;
            row.lead_harness_id = None;
            row.base_sha = None;
            row.prompt_first_line = None;
        }
        out.push(row);
    }

    Ok(ListRoomsOut {
        rooms: out,
        has_more,
        unreadable_rooms,
    })
}
