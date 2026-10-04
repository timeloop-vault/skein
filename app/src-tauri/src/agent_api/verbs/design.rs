//! Driving a room's design pane (#512, epic #513): list the design
//! harnesses, read one pane's state, switch its entry, and highlight an
//! element.
//!
//! Every verb is scoped to the CALLER's room by the bearer token — there
//! is no room argument. `harness` is optional when the room has exactly
//! one design harness. A harness id that is not a design harness of the
//! caller's room is `NotFound` whether or not it exists elsewhere, so a
//! token cannot probe other rooms. No verb takes focus or switches the
//! visible room or harness.
//!
//! The pane's state is React-local, so the live facts come from the
//! webview through `AgentApiState::request_frontend` with these kinds:
//!
//! | kind | args | answer |
//! |---|---|---|
//! | `design.panes` | `{roomId}` | `{panes: [{harnessId, mounted, ready}]}` |
//! | `design.state` | `{roomId, harnessId}` | `{entry, device, ready, loadFailed, errors[], selected, scroll}` |
//! | `design.open_entry` | `{roomId, harnessId, entry}` | `{entry, previous}` |
//! | `design.show_element` | `{roomId, harnessId, selector?, anchor?}` | `{tier, highlighted, element, count?, score?}` |
//!
//! A frontend error string is prefixed with a code (`not_mounted: …`,
//! `not_ready: …`) and surfaces as a refusal carrying that text.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::shared::frontend_error;
use super::{VerbError, VerbResult, internal};
use crate::agent_api::auth::Caller;
use crate::agent_api::state::AgentApiState;
use crate::db::{Harness, Room};
use crate::design::commands::list_entries_for_room;

/// Rate cap for the two write verbs, combined, per calling room. Same
/// numbers as `open_harness`/`close_harness`, its own bucket: driving a
/// pane and changing which harnesses exist are different budgets.
const DESIGN_CONTROL_BUCKET: &str = "design_control";
const DESIGN_CONTROL_RATE_LIMIT: usize = 10;
const DESIGN_CONTROL_WINDOW: Duration = Duration::from_secs(60);

/// `list_design_harnesses` degrades to nulls rather than waiting long.
const PANES_TIMEOUT: Duration = Duration::from_secs(3);
const STATE_TIMEOUT: Duration = Duration::from_secs(5);
const OPEN_ENTRY_TIMEOUT: Duration = Duration::from_secs(10);
const SHOW_ELEMENT_TIMEOUT: Duration = Duration::from_secs(10);

/// How many entries an `unknown_entry` refusal lists.
const UNKNOWN_ENTRY_LIST_CAP: usize = 20;
/// Cap on a `show_element` selector, so a runaway string never reaches
/// the iframe.
const MAX_SELECTOR_CHARS: usize = 2000;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesignTargetArgs {
    /// Optional when the room has exactly one design harness.
    #[serde(default)]
    pub harness: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenDesignEntryArgs {
    #[serde(default)]
    pub harness: Option<String>,
    pub entry: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShowElementArgs {
    #[serde(default)]
    pub harness: Option<String>,
    /// A CSS selector. Exactly one of `selector` / `anchor`.
    #[serde(default)]
    pub selector: Option<String>,
    /// The #434 element-anchor shape, passed through as JSON.
    #[serde(default)]
    pub anchor: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignHarnessOut {
    pub harness_id: String,
    pub name: String,
    pub entry: Option<String>,
    /// As stored on the harness; `null` when the user never set one.
    pub device: Option<Value>,
    pub entries: Vec<String>,
    /// `null` when the webview could not answer.
    pub mounted: Option<bool>,
    pub ready: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ListDesignHarnessesOut {
    pub harnesses: Vec<DesignHarnessOut>,
}

fn log_outcome(verb: &str, caller_room: &str, harness: &str, outcome: &str) {
    tracing::info!(
        caller_room = %caller_room,
        harness = %harness,
        outcome = %outcome,
        "agent_api: {verb}"
    );
}

/// The caller's own room, refused when archived.
fn caller_room(state: &AgentApiState, caller: &Caller) -> VerbResult<Room> {
    let room = state
        .db
        .room_by_id(&caller.room_id)
        .map_err(internal)?
        .ok_or_else(|| VerbError::Unavailable("the calling room no longer exists".into()))?;
    if room.archived.is_some() {
        return Err(VerbError::Refused(format!(
            "archived: {} is archived",
            room.name
        )));
    }
    Ok(room)
}

fn is_design(h: &Harness) -> bool {
    h.kind == "design"
}

/// Pick the harness a verb acts on, per the module doc's rules.
fn pick_harness(room: &Room, requested: Option<&str>) -> VerbResult<Harness> {
    let designs: Vec<&Harness> = room.harnesses.iter().filter(|h| is_design(h)).collect();
    if let Some(id) = requested.map(str::trim).filter(|s| !s.is_empty()) {
        return designs
            .into_iter()
            .find(|h| h.id == id)
            .cloned()
            .ok_or_else(|| {
                VerbError::NotFound(format!("not_found: no such design harness: {id:?}"))
            });
    }
    match designs.as_slice() {
        [] => Err(VerbError::Refused(
            "no_design_harness: this room has no design harness; open one with \
             open_harness (kind \"design\")"
                .into(),
        )),
        [only] => Ok((*only).clone()),
        many => {
            let ids: Vec<&str> = many.iter().map(|h| h.id.as_str()).collect();
            Err(VerbError::Refused(format!(
                "harness_required: this room has {} design harnesses — pass `harness` \
                 as one of {}",
                ids.len(),
                ids.join(", ")
            )))
        }
    }
}

/// Entries the room's folder offers; blocking walk kept off the runtime.
async fn entries_for(state: &AgentApiState, room_id: &str) -> VerbResult<Vec<String>> {
    let db = state.db.clone();
    let room_id = room_id.to_owned();
    tokio::task::spawn_blocking(move || list_entries_for_room(&db, &room_id))
        .await
        .map_err(internal)?
        .map_err(VerbError::Unavailable)
}

/// Kill switch then rate cap, shared by the two write verbs.
fn guard_write(
    state: &AgentApiState,
    caller: &Caller,
    verb: &str,
    harness_control_enabled: bool,
) -> VerbResult<()> {
    if !harness_control_enabled {
        log_outcome(verb, &caller.room_id, "", "disabled");
        return Err(VerbError::Refused(
            "disabled: agent harness control is turned off in Settings".into(),
        ));
    }
    if !state.check_verb_rate(
        DESIGN_CONTROL_BUCKET,
        &caller.room_id,
        DESIGN_CONTROL_WINDOW,
        DESIGN_CONTROL_RATE_LIMIT,
    ) {
        log_outcome(verb, &caller.room_id, "", "rate_limited");
        return Err(VerbError::Refused(format!(
            "rate_limited: this room has attempted {DESIGN_CONTROL_RATE_LIMIT} \
             open_design_entry/show_element calls in the last minute"
        )));
    }
    Ok(())
}

/// Ask the frontend, mapping its error string the way `open_harness` does.
async fn ask(
    state: &AgentApiState,
    verb: &str,
    caller: &Caller,
    harness_id: &str,
    kind: &str,
    args: Value,
    timeout: Duration,
) -> VerbResult<Value> {
    match state.request_frontend(kind, args, timeout).await {
        Ok(v) => {
            log_outcome(verb, &caller.room_id, harness_id, "ok");
            Ok(v)
        }
        Err(e) => {
            let err = frontend_error(&e);
            log_outcome(verb, &caller.room_id, harness_id, err.message());
            Err(err)
        }
    }
}

/// The frontend's object with `harnessId` added, so a caller that relied
/// on the single-harness default learns which one answered.
fn with_harness_id(mut answer: Value, harness_id: &str) -> VerbResult<Value> {
    match answer.as_object_mut() {
        Some(obj) => {
            obj.entry("harnessId").or_insert_with(|| json!(harness_id));
            Ok(answer)
        }
        None => Err(VerbError::Unavailable(
            "the app returned an unexpected (non-object) answer".into(),
        )),
    }
}

/// Mounted/ready per harness, or `None` when the webview can't say.
async fn pane_status(state: &AgentApiState, room_id: &str) -> Option<Vec<(String, bool, bool)>> {
    let v = state
        .request_frontend("design.panes", json!({ "roomId": room_id }), PANES_TIMEOUT)
        .await
        .ok()?;
    let panes = v.get("panes")?.as_array()?;
    Some(
        panes
            .iter()
            .filter_map(|p| {
                Some((
                    p.get("harnessId")?.as_str()?.to_owned(),
                    p.get("mounted").and_then(Value::as_bool).unwrap_or(false),
                    p.get("ready").and_then(Value::as_bool).unwrap_or(false),
                ))
            })
            .collect(),
    )
}

/// Every design harness in the caller's room. Read-only, no kill switch.
/// `mounted`/`ready` are `null` when the webview is unavailable.
pub async fn list_design_harnesses(
    state: &AgentApiState,
    caller: &Caller,
) -> VerbResult<ListDesignHarnessesOut> {
    let room = caller_room(state, caller)?;
    let designs: Vec<&Harness> = room.harnesses.iter().filter(|h| is_design(h)).collect();
    if designs.is_empty() {
        return Ok(ListDesignHarnessesOut { harnesses: vec![] });
    }
    let entries = entries_for(state, &room.id).await?;
    let panes = pane_status(state, &room.id).await;
    let harnesses = designs
        .into_iter()
        .map(|h| {
            let pane = panes
                .as_ref()
                .map(|p| p.iter().find(|(id, _, _)| *id == h.id));
            DesignHarnessOut {
                harness_id: h.id.clone(),
                name: h.name.clone(),
                entry: h.design_entry.clone(),
                device: h.design_device.clone(),
                entries: entries.clone(),
                mounted: pane.map(|p| p.is_some_and(|(_, m, _)| *m)),
                ready: pane.map(|p| p.is_some_and(|(_, _, r)| *r)),
            }
        })
        .collect();
    Ok(ListDesignHarnessesOut { harnesses })
}

/// One pane's live state, as the frontend reports it. Read-only.
pub async fn get_design_state(
    state: &AgentApiState,
    caller: &Caller,
    args: &DesignTargetArgs,
) -> VerbResult<Value> {
    let room = caller_room(state, caller)?;
    let harness = pick_harness(&room, args.harness.as_deref())?;
    let answer = ask(
        state,
        "get_design_state",
        caller,
        &harness.id,
        "design.state",
        json!({ "roomId": room.id, "harnessId": harness.id }),
        STATE_TIMEOUT,
    )
    .await?;
    with_harness_id(answer, &harness.id)
}

/// Switch a design harness's entry file (persisted, like the toolbar
/// picker). The entry must be exactly one of the listed entries.
pub async fn open_design_entry(
    state: &AgentApiState,
    caller: &Caller,
    args: &OpenDesignEntryArgs,
    harness_control_enabled: bool,
) -> VerbResult<Value> {
    guard_write(state, caller, "open_design_entry", harness_control_enabled)?;
    let room = caller_room(state, caller)?;
    let harness = pick_harness(&room, args.harness.as_deref())?;
    let entries = entries_for(state, &room.id).await?;
    if !entries.contains(&args.entry) {
        log_outcome(
            "open_design_entry",
            &caller.room_id,
            &harness.id,
            "unknown_entry",
        );
        let shown: Vec<&str> = entries
            .iter()
            .take(UNKNOWN_ENTRY_LIST_CAP)
            .map(String::as_str)
            .collect();
        let more = entries.len().saturating_sub(shown.len());
        let tail = if more > 0 {
            format!(" (and {more} more)")
        } else {
            String::new()
        };
        return Err(VerbError::Refused(format!(
            "unknown_entry: {:?} is not one of this room's entries; available: [{}]{tail}",
            args.entry,
            shown.join(", ")
        )));
    }
    let answer = ask(
        state,
        "open_design_entry",
        caller,
        &harness.id,
        "design.open_entry",
        json!({ "roomId": room.id, "harnessId": harness.id, "entry": args.entry }),
        OPEN_ENTRY_TIMEOUT,
    )
    .await?;
    with_harness_id(answer, &harness.id)
}

fn bad_arguments(msg: &str) -> VerbError {
    VerbError::Refused(format!("bad_arguments: {msg}"))
}

/// Validate the selector/anchor pair before anything else runs.
fn check_target(args: &ShowElementArgs) -> VerbResult<()> {
    match (&args.selector, &args.anchor) {
        (Some(_), Some(_)) => Err(bad_arguments(
            "pass either `selector` or `anchor`, not both",
        )),
        (None, None) => Err(bad_arguments("pass one of `selector` or `anchor`")),
        (Some(s), None) => {
            if s.trim().is_empty() {
                Err(bad_arguments("`selector` is empty"))
            } else if s.chars().count() > MAX_SELECTOR_CHARS {
                Err(bad_arguments(&format!(
                    "`selector` is capped at {MAX_SELECTOR_CHARS} characters"
                )))
            } else {
                Ok(())
            }
        }
        (None, Some(a)) => {
            let Some(obj) = a.as_object() else {
                return Err(bad_arguments("`anchor` must be an object"));
            };
            let has = |k: &str| {
                obj.get(k)
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty())
            };
            if has("selector") || has("tag") {
                Ok(())
            } else {
                Err(bad_arguments(
                    "`anchor` needs a `selector` or a `tag` string",
                ))
            }
        }
    }
}

/// Scroll to and highlight an element, transiently — no thread is
/// created. The frontend answers with the match tier; it never
/// highlights a guess.
pub async fn show_element(
    state: &AgentApiState,
    caller: &Caller,
    args: &ShowElementArgs,
    harness_control_enabled: bool,
) -> VerbResult<Value> {
    check_target(args)?;
    guard_write(state, caller, "show_element", harness_control_enabled)?;
    let room = caller_room(state, caller)?;
    let harness = pick_harness(&room, args.harness.as_deref())?;
    let mut request = json!({ "roomId": room.id, "harnessId": harness.id });
    if let Some(obj) = request.as_object_mut() {
        if let Some(s) = &args.selector {
            obj.insert("selector".into(), json!(s));
        }
        if let Some(a) = &args.anchor {
            obj.insert("anchor".into(), a.clone());
        }
    }
    let answer = ask(
        state,
        "show_element",
        caller,
        &harness.id,
        "design.show_element",
        request,
        SHOW_ELEMENT_TIMEOUT,
    )
    .await?;
    with_harness_id(answer, &harness.id)
}
