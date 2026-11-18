use serde_json::{Value, json};

use super::user_prompt_text;

fn human(content: impl Into<Value>) -> Value {
    json!({"type":"user","origin":{"kind":"human"},"promptId":"p","uuid":"u",
        "timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":content.into()}})
}

fn bare(content: &str) -> Value {
    json!({"type":"user","message":{"role":"user","content":content}})
}

fn with(mut row: Value, key: &str, v: Value) -> Value {
    row[key] = v;
    row
}

#[test]
fn string_human_prompt() {
    assert_eq!(
        user_prompt_text(&human(json!("fix the bug"))).as_deref(),
        Some("fix the bug")
    );
}

#[test]
fn list_content_joins_text_blocks_and_skips_images() {
    let row = human(json!([
        {"type":"text","text":"look at this"},
        {"type":"image","source":{"type":"base64","data":"x"}},
        {"type":"text","text":"and fix it"}
    ]));
    assert_eq!(
        user_prompt_text(&row).as_deref(),
        Some("look at this\nand fix it")
    );
}

#[test]
fn image_only_is_not_a_prompt() {
    let row = human(json!([{"type":"image","source":{}}]));
    assert_eq!(user_prompt_text(&row), None);
}

#[test]
fn cross_session_message_counts() {
    let row = bare("Another Claude session sent a message: hello");
    assert_eq!(
        user_prompt_text(&row).as_deref(),
        Some("Another Claude session sent a message: hello")
    );
}

#[test]
fn truncates_to_200_chars() {
    let long = "a".repeat(300);
    let got = user_prompt_text(&human(json!(long))).unwrap();
    assert_eq!(got.chars().count(), 201);
    assert!(got.ends_with('…'));
    assert!(got.starts_with(&"a".repeat(200)));
}

#[test]
fn exactly_200_chars_is_untouched() {
    let s = "b".repeat(200);
    assert_eq!(user_prompt_text(&human(json!(s.clone()))), Some(s));
}

#[test]
fn truncates_multibyte_on_a_char_boundary() {
    let long = "å".repeat(250);
    let got = user_prompt_text(&human(json!(long))).unwrap();
    assert_eq!(got.chars().count(), 201);
    assert!(got.starts_with(&"å".repeat(200)));
    assert!(got.ends_with('…'));
}

#[test]
fn not_prompts() {
    let cases: Vec<(&str, Value)> = vec![
        ("isMeta", with(human(json!("x")), "isMeta", json!(true))),
        (
            "task-notification origin",
            json!({"type":"user","origin":{"kind":"task-notification"},
                "message":{"content":"done"}}),
        ),
        (
            "peer origin",
            json!({"type":"user","origin":{"kind":"peer"},"message":{"content":"hi"}}),
        ),
        (
            "toolUseResult",
            with(human(json!("x")), "toolUseResult", json!({"stdout":""})),
        ),
        (
            "tool_result block",
            human(json!([{"type":"tool_result","tool_use_id":"t","content":"ok"}])),
        ),
        (
            "queueTranscriptOnly",
            with(human(json!("x")), "queueTranscriptOnly", json!(true)),
        ),
        (
            "isCompactSummary",
            with(human(json!("x")), "isCompactSummary", json!(true)),
        ),
        ("command-name", bare("<command-name>/model</command-name>")),
        (
            "local stdout",
            bare("<local-command-stdout>x</local-command-stdout>"),
        ),
        (
            "local caveat",
            bare("<local-command-caveat>x</local-command-caveat>"),
        ),
        ("bash-input", bare("<bash-input>ls</bash-input>")),
        ("bash-stdout", bare("<bash-stdout>x</bash-stdout>")),
        ("indented task-notification", bare("  <task-notification>x")),
        (
            "sidechain",
            with(human(json!("x")), "isSidechain", json!(true)),
        ),
        ("empty", human(json!(""))),
        ("whitespace", human(json!("  \n\t"))),
        (
            "assistant row",
            json!({"type":"assistant","message":{"content":"x"}}),
        ),
    ];
    for (name, row) in cases {
        assert_eq!(user_prompt_text(&row), None, "{name}");
    }
}

#[test]
fn human_prompt_may_start_with_angle_bracket() {
    let row = human(json!("<div> is broken, fix it"));
    assert_eq!(
        user_prompt_text(&row).as_deref(),
        Some("<div> is broken, fix it")
    );
}
