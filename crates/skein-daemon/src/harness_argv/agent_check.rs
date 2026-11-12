//! What a named agent is, measured against a listing (port of
//! `validateAgent` in `app/src/agents.ts`).

use serde::Serialize;

use crate::agents::AgentListDto;

/// The agent listing, plus (when the caller asked) the verdict on one
/// stored name measured against that same listing. Without a check there
/// is no `verdict` key, so the shape is the plain listing's.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentListReply {
    #[serde(flatten)]
    pub listing: AgentListDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<AgentVerdict>,
}

/// `check = None` adds no verdict; `Some("")` is "no agent" and so `ok`.
#[must_use]
pub fn with_verdict(listing: AgentListDto, check: Option<&str>) -> AgentListReply {
    let verdict = check.map(|c| validate_agent(Some(c), &listing));
    AgentListReply { listing, verdict }
}

/// - `Ok`: no agent named (the tool's own default), or the list vouches
///   for it.
/// - `Unknown`: the CLI answered and this name was not in the answer.
///   The only verdict that blocks a spawn.
/// - `Unverified`: the list is degraded, so absence proves nothing.
///   Spawning is the right call: refusing here would strand every harness
///   the moment the CLI is briefly unavailable, and the spawn itself
///   fails loudly if the name really is gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AgentVerdict {
    Ok,
    Unknown { agent: String },
    Unverified { agent: String, why: String },
}

/// Matching is exact and case-sensitive, namespace included, because the
/// CLI is. A blank name is "no agent".
#[must_use]
pub fn validate_agent(agent: Option<&str>, listing: &AgentListDto) -> AgentVerdict {
    let Some(agent) = agent.filter(|a| !a.trim().is_empty()) else {
        return AgentVerdict::Ok;
    };
    if let Some(why) = &listing.degraded {
        return AgentVerdict::Unverified {
            agent: agent.to_owned(),
            why: why.clone(),
        };
    }
    if listing.agents.iter().any(|a| a.name == agent) {
        return AgentVerdict::Ok;
    }
    AgentVerdict::Unknown {
        agent: agent.to_owned(),
    }
}
