//! The context-limit nudge (#689): the text and the pure decision. No
//! db, no environment; the caller reads the mark and the newest context
//! size, calls `decide`, and stores the returned mark itself.

/// The text sent when the user set none.
pub const DEFAULT_BODY: &str = "Your context is at {tokens} tokens, over the limit of {limit} set in Skein. Tell the user it is time to hand over or start a fresh session. Do not compact or restart on your own.";

/// The nudge text. A `None` or blank `body` means `DEFAULT_BODY`; every
/// `{tokens}` and `{limit}` becomes a plain decimal integer.
#[must_use]
pub fn render(body: Option<&str>, tokens: u64, limit: u64) -> String {
    let body = body
        .filter(|b| !b.trim().is_empty())
        .unwrap_or(DEFAULT_BODY);
    body.replace("{tokens}", &tokens.to_string())
        .replace("{limit}", &limit.to_string())
}

/// What the last check recorded for one harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    /// The newest `turn_cost` row id seen.
    pub last_row_id: Option<i64>,
    /// True when the next crossing of the limit may nudge.
    pub armed: bool,
}

/// The outcome of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Nothing to store, no nudge.
    Nothing,
    /// Store this mark now; no nudge.
    Store(Mark),
    /// Send a nudge, then store `after`. Storing it before the send
    /// succeeds would lose the nudge, so an undelivered one is decided
    /// due again on the next check.
    Due {
        tokens: u64,
        limit: u64,
        after: Mark,
    },
}

/// Decide from the stored mark, the newest `(row id, context tokens)`
/// and the configured limit. A size equal to the limit counts as over.
#[must_use]
pub fn decide(mark: Option<Mark>, newest: Option<(i64, u64)>, limit: Option<u64>) -> Decision {
    let Some(limit) = limit else {
        // No limit: never due, but keep the mark current and armed, so a
        // limit set later judges only rows that land after it.
        return match (mark, newest) {
            (None, _) => Decision::Store(Mark {
                last_row_id: newest.map(|(id, _)| id),
                armed: true,
            }),
            (Some(m), Some((id, _))) if m.last_row_id != Some(id) => Decision::Store(Mark {
                last_row_id: Some(id),
                armed: true,
            }),
            _ => Decision::Nothing,
        };
    };
    let Some(mark) = mark else {
        // First sight: never nudge a harness that is already over.
        return Decision::Store(Mark {
            last_row_id: newest.map(|(id, _)| id),
            armed: newest.is_none_or(|(_, t)| t < limit),
        });
    };
    let Some((id, tokens)) = newest else {
        return Decision::Nothing;
    };
    if mark.last_row_id == Some(id) {
        return Decision::Nothing;
    }
    if tokens < limit {
        return Decision::Store(Mark {
            last_row_id: Some(id),
            armed: true,
        });
    }
    let after = Mark {
        last_row_id: Some(id),
        armed: false,
    };
    if mark.armed {
        Decision::Due {
            tokens,
            limit,
            after,
        }
    } else {
        Decision::Store(after)
    }
}

#[cfg(test)]
mod tests;
