//! Ports of the `validateAgent` cases of `app/src/agents.test.ts`.

use crate::agents::{AgentDto, AgentListDto};

use super::{AgentVerdict, validate_agent, with_verdict};

fn reply_json(l: AgentListDto, check: Option<&str>) -> serde_json::Value {
    serde_json::to_value(with_verdict(l, check)).unwrap()
}

#[test]
fn reply_without_check_has_no_verdict_key() {
    let v = reply_json(listing(&["coder"], None), None);
    let obj = v.as_object().unwrap();
    assert!(!obj.contains_key("verdict"));
    assert_eq!(obj["degraded"], serde_json::Value::Null);
    assert_eq!(obj["unsupported"], false);
    assert_eq!(obj["agents"][0]["name"], "coder");
}

#[test]
fn reply_verdicts_serialise_like_the_ts_union() {
    let ok = reply_json(listing(&["coder"], None), Some("coder"));
    assert_eq!(ok["verdict"], serde_json::json!({"kind": "ok"}));
    assert_eq!(ok["agents"][0]["name"], "coder");
    let unk = reply_json(listing(&["coder"], None), Some("gone"));
    assert_eq!(
        unk["verdict"],
        serde_json::json!({"kind": "unknown", "agent": "gone"})
    );
    let unv = reply_json(listing(&[], Some("no cli")), Some("gone"));
    assert_eq!(
        unv["verdict"],
        serde_json::json!({"kind": "unverified", "agent": "gone", "why": "no cli"})
    );
    let blank = reply_json(listing(&["coder"], None), Some("  "));
    assert_eq!(blank["verdict"], serde_json::json!({"kind": "ok"}));
}

fn agent(name: &str) -> AgentDto {
    AgentDto {
        name: name.to_owned(),
        description: None,
        tools: None,
        model: None,
        source: "user".to_owned(),
        plugin: None,
        allows_review_tools: true,
    }
}

fn listing(names: &[&str], degraded: Option<&str>) -> AgentListDto {
    AgentListDto {
        agents: names.iter().map(|n| agent(n)).collect(),
        degraded: degraded.map(str::to_owned),
        unsupported: false,
    }
}

fn unknown(agent: &str) -> AgentVerdict {
    AgentVerdict::Unknown {
        agent: agent.to_owned(),
    }
}

fn unverified(agent: &str, why: &str) -> AgentVerdict {
    AgentVerdict::Unverified {
        agent: agent.to_owned(),
        why: why.to_owned(),
    }
}

#[test]
fn accepts_no_agent_at_all() {
    let l = listing(&["reviewer"], None);
    assert_eq!(validate_agent(None, &l), AgentVerdict::Ok);
    assert_eq!(validate_agent(Some(""), &l), AgentVerdict::Ok);
    assert_eq!(validate_agent(Some("  "), &l), AgentVerdict::Ok);
}

#[test]
fn accepts_a_name_the_cli_vouched_for() {
    let l = listing(&["coder", "reviewer"], None);
    assert_eq!(validate_agent(Some("reviewer"), &l), AgentVerdict::Ok);
}

#[test]
fn rejects_a_name_an_authoritative_list_does_not_have() {
    let l = listing(&["coder"], None);
    assert_eq!(validate_agent(Some("reviewer"), &l), unknown("reviewer"));
}

#[test]
fn matches_exactly_namespaced_names_are_not_the_same_agent() {
    let l = listing(&["pr-review-toolkit:code-reviewer"], None);
    assert_eq!(
        validate_agent(Some("code-reviewer"), &l),
        unknown("code-reviewer")
    );
}

#[test]
fn is_case_sensitive_because_the_cli_is() {
    let l = listing(&["Explore"], None);
    assert_eq!(validate_agent(Some("explore"), &l), unknown("explore"));
}

#[test]
fn never_rejects_on_a_degraded_list() {
    assert_eq!(
        validate_agent(
            Some("reviewer"),
            &listing(&[], Some("claude: program not found"))
        ),
        unverified("reviewer", "claude: program not found")
    );
    assert_eq!(
        validate_agent(
            Some("reviewer"),
            &listing(&["coder"], Some("output unparseable"))
        ),
        unverified("reviewer", "output unparseable")
    );
}

#[test]
fn treats_a_listing_nobody_fetched_as_unverified_not_as_empty() {
    // The TS `NO_AGENTS`: empty, degraded "not fetched", unsupported.
    let no_agents = AgentListDto {
        agents: Vec::new(),
        degraded: Some("not fetched".to_owned()),
        unsupported: true,
    };
    assert_eq!(
        validate_agent(Some("reviewer"), &no_agents),
        unverified("reviewer", "not fetched")
    );
}

#[test]
fn keeps_the_untrimmed_name_in_the_verdict() {
    let l = listing(&["coder"], None);
    assert_eq!(validate_agent(Some(" x "), &l), unknown(" x "));
}
