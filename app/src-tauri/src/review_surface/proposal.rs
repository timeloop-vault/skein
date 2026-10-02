//! Proposed edits on element threads (#436).
//!
//! The design pane records a structured change request on a new element
//! thread; Skein never writes source for it — the agent applies it and
//! marks the thread addressed. A proposal exists only from the moment its
//! thread is created, so no thread id lets a later message forge one.
//!
//! Everything here comes from the iframe and is untrusted: the webview
//! applies the same caps, but this module is the authority.

use std::collections::HashMap;
use std::fmt::Write;

use serde::{Deserialize, Serialize};

use super::dto::ThreadDto;
use crate::db::Database;

const MAX_CHANGES: usize = 32;
const MAX_PROPOSAL_BYTES: usize = 16384;
const MAX_STYLE_VALUE_CHARS: usize = 200;
const MAX_TEXT_CHARS: usize = 2000;
const MAX_TOKEN_CHARS: usize = 100;
const MAX_OFFSET_ABS: f64 = 100_000.0;

/// The CSS properties a proposal may touch. Keep identical to the
/// allowlists in `app/src/designProposal.ts` (TS) and
/// `app/src-tauri/src/design/editor.js` — three copies, one list.
pub const PROPERTY_ALLOWLIST: &[&str] = &[
    "width",
    "height",
    "margin-top",
    "margin-right",
    "margin-bottom",
    "margin-left",
    "padding-top",
    "padding-right",
    "padding-bottom",
    "padding-left",
    "gap",
    "row-gap",
    "column-gap",
    "color",
    "background-color",
    "border-color",
    "border-radius",
    "font-family",
    "font-size",
    "font-weight",
    "line-height",
    "letter-spacing",
    "opacity",
];

/// A set of changes to one element. Stored as JSON, returned to the
/// frontend and the agent as-is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    pub changes: Vec<Change>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Change {
    /// One CSS property. `token` names a custom property whose value is `to`.
    Style {
        property: String,
        from: String,
        to: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
    },
    /// Cumulative drag/nudge, in CSS px.
    Offset { dx: f64, dy: f64 },
    /// The element's text, before and after.
    Text { from: String, to: String },
}

fn check_style_value(v: &str, what: &str) -> Result<(), String> {
    if v.chars().count() > MAX_STYLE_VALUE_CHARS {
        return Err(format!(
            "a style {what} is longer than {MAX_STYLE_VALUE_CHARS} characters"
        ));
    }
    if v.chars().any(char::is_control) {
        return Err(format!("a style {what} contains a control character"));
    }
    Ok(())
}

fn check_token(t: &str) -> Result<(), String> {
    let name = t.strip_prefix("--").unwrap_or("");
    let ok = !name.is_empty()
        && name.chars().count() <= MAX_TOKEN_CHARS
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "token {t:?} must be a custom property name like --space-4"
        ))
    }
}

fn check_offset(v: f64) -> Result<(), String> {
    if v.is_finite() && v.abs() <= MAX_OFFSET_ABS {
        Ok(())
    } else {
        Err(format!("an offset must be within ±{MAX_OFFSET_ABS} px"))
    }
}

/// Check a proposal against every rule in the #436 spec.
pub fn validate_proposal(p: &Proposal) -> Result<(), String> {
    if p.changes.is_empty() {
        return Err("a proposal needs at least one change".into());
    }
    if p.changes.len() > MAX_CHANGES {
        return Err(format!(
            "a proposal may carry at most {MAX_CHANGES} changes"
        ));
    }
    let mut properties: Vec<&str> = Vec::new();
    let (mut offsets, mut texts) = (0, 0);
    for c in &p.changes {
        match c {
            Change::Style {
                property,
                from,
                to,
                token,
            } => {
                if !PROPERTY_ALLOWLIST.contains(&property.as_str()) {
                    return Err(format!("property {property:?} is not allowed"));
                }
                if properties.contains(&property.as_str()) {
                    return Err(format!("property {property:?} appears twice"));
                }
                properties.push(property);
                check_style_value(from, "from value")?;
                check_style_value(to, "to value")?;
                if from == to {
                    return Err(format!("{property} does not change"));
                }
                if let Some(t) = token {
                    check_token(t)?;
                }
            }
            Change::Offset { dx, dy } => {
                offsets += 1;
                if offsets > 1 {
                    return Err("a proposal may carry at most one offset".into());
                }
                check_offset(*dx)?;
                check_offset(*dy)?;
                if *dx == 0.0 && *dy == 0.0 {
                    return Err("an offset must move the element".into());
                }
            }
            Change::Text { from, to } => {
                texts += 1;
                if texts > 1 {
                    return Err("a proposal may carry at most one text change".into());
                }
                for v in [from, to] {
                    if v.chars().count() > MAX_TEXT_CHARS {
                        return Err(format!(
                            "a text change is longer than {MAX_TEXT_CHARS} characters"
                        ));
                    }
                    if v.contains('\0') {
                        return Err("a text change contains a NUL".into());
                    }
                }
                if from == to {
                    return Err("the text does not change".into());
                }
            }
        }
    }
    let size = serde_json::to_vec(p).map_err(|e| e.to_string())?.len();
    if size > MAX_PROPOSAL_BYTES {
        return Err(format!(
            "a proposal may be at most {MAX_PROPOSAL_BYTES} bytes serialized"
        ));
    }
    Ok(())
}

/// Every stored proposal in the room, keyed by thread. A row that no
/// longer parses is logged and left out — the thread still renders.
pub(super) fn proposals_by_thread(
    db: &Database,
    room_id: &str,
) -> Result<HashMap<String, Proposal>, String> {
    let mut out = HashMap::new();
    for (thread_id, json) in db.review_element_proposals_for_room(room_id)? {
        match serde_json::from_str::<Proposal>(&json) {
            Ok(p) => {
                out.insert(thread_id, p);
            }
            Err(e) => tracing::warn!(thread_id, "unreadable stored proposal: {e}"),
        }
    }
    Ok(out)
}

/// Stamp the proposal onto element threads that have one.
pub(super) fn apply_proposals(threads: &mut [ThreadDto], proposals: &HashMap<String, Proposal>) {
    for t in threads.iter_mut() {
        t.proposal = proposals.get(&t.id).cloned();
    }
}

/// `4.0` prints as `4`; `-0.0` as `0`.
fn px(v: f64) -> String {
    format!("{}px", v + 0.0)
}

/// The text of the first comment on a proposal thread.
pub fn summary(p: &Proposal) -> String {
    let mut out = String::from("Proposed edit:");
    for c in &p.changes {
        out.push_str("\n- ");
        match c {
            Change::Style {
                property,
                from,
                to,
                token,
            } => {
                let _ = write!(out, "{property} {from} → {to}");
                if let Some(t) = token {
                    let _ = write!(out, " (token {t})");
                }
            }
            Change::Offset { dx, dy } => {
                let _ = write!(out, "move by dx {}, dy {}", px(*dx), px(*dy));
            }
            Change::Text { from, to } => {
                let _ = write!(out, "text \"{from}\" → \"{to}\"");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
