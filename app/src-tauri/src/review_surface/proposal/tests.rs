use super::*;

fn style(property: &str, from: &str, to: &str) -> Change {
    Change::Style {
        property: property.into(),
        from: from.into(),
        to: to.into(),
        token: None,
    }
}

fn offset(dx: f64, dy: f64) -> Change {
    Change::Offset { dx, dy }
}

fn text(from: &str, to: &str) -> Change {
    Change::Text {
        from: from.into(),
        to: to.into(),
    }
}

fn check(changes: Vec<Change>) -> Result<(), String> {
    validate_proposal(&Proposal { changes })
}

fn mixed() -> Proposal {
    Proposal {
        changes: vec![
            Change::Style {
                property: "padding-left".into(),
                from: "12px".into(),
                to: "16px".into(),
                token: Some("--space-4".into()),
            },
            offset(4.0, 0.0),
            text("Save", "Save changes"),
        ],
    }
}

#[test]
fn a_mixed_proposal_is_valid() {
    validate_proposal(&mixed()).unwrap();
}

#[test]
fn the_count_must_be_one_to_thirty_two() {
    assert!(check(vec![]).is_err());
    let many = |n: usize| -> Vec<Change> {
        PROPERTY_ALLOWLIST
            .iter()
            .cycle()
            .take(n)
            .enumerate()
            .map(|(i, p)| {
                if i < PROPERTY_ALLOWLIST.len() {
                    style(p, "1", "2")
                } else {
                    offset(1.0, 1.0)
                }
            })
            .collect()
    };
    // 23 distinct properties + one offset + one text is as many as the
    // rules allow without repeating, so build the 33 case by hand.
    assert!(check(many(23)).is_ok());
    let mut over = many(23);
    over.extend((0..10).map(|_| offset(1.0, 1.0)));
    assert!(check(over).unwrap_err().contains("at most 32"));
}

#[test]
fn style_rules() {
    assert!(check(vec![style("z-index", "1", "2")]).is_err());
    assert!(check(vec![style("gap", "1", "2"), style("gap", "2", "3")]).is_err());
    assert!(check(vec![style("gap", &"x".repeat(201), "2")]).is_err());
    assert!(check(vec![style("gap", "1", &"x".repeat(201))]).is_err());
    assert!(check(vec![style("gap", &"x".repeat(200), "2")]).is_ok());
    assert!(check(vec![style("gap", "1\n", "2")]).is_err());
    assert!(check(vec![style("gap", "1", "2\u{0}")]).is_err());
    assert!(check(vec![style("gap", "1px", "1px")]).is_err());
}

#[test]
fn tokens_must_be_custom_property_names() {
    let with = |t: &str| {
        check(vec![Change::Style {
            property: "gap".into(),
            from: "1".into(),
            to: "2".into(),
            token: Some(t.into()),
        }])
    };
    assert!(with("--space-4").is_ok());
    assert!(with("--a_B-9").is_ok());
    for bad in [
        "",
        "--",
        "space-4",
        "-space",
        "--a b",
        "--a;b",
        "--a)",
        &format!("--{}", "x".repeat(101)),
    ] {
        assert!(with(bad).is_err(), "{bad:?}");
    }
    assert!(with(&format!("--{}", "x".repeat(100))).is_ok());
}

#[test]
fn offset_rules() {
    assert!(check(vec![offset(1.0, 0.0), offset(2.0, 0.0)]).is_err());
    assert!(check(vec![offset(0.0, 0.0)]).is_err());
    assert!(check(vec![offset(-0.0, 0.0)]).is_err());
    assert!(check(vec![offset(f64::NAN, 1.0)]).is_err());
    assert!(check(vec![offset(1.0, f64::INFINITY)]).is_err());
    assert!(check(vec![offset(100_001.0, 0.0)]).is_err());
    assert!(check(vec![offset(0.0, -100_000.0)]).is_ok());
}

#[test]
fn text_rules() {
    assert!(check(vec![text("a", "b"), text("b", "c")]).is_err());
    assert!(check(vec![text("a", "a")]).is_err());
    assert!(check(vec![text("a", &"x".repeat(2001))]).is_err());
    assert!(check(vec![text(&"x".repeat(2001), "a")]).is_err());
    assert!(check(vec![text("a", &"x".repeat(2000))]).is_ok());
    assert!(check(vec![text("a", "b\0")]).is_err());
    // Newlines are legitimate text.
    assert!(check(vec![text("a", "b\nc")]).is_ok());
}

#[test]
fn an_oversized_proposal_is_rejected() {
    let long = "é".repeat(200);
    let mut changes: Vec<Change> = PROPERTY_ALLOWLIST
        .iter()
        .map(|p| style(p, &long, &"ü".repeat(200)))
        .collect();
    changes.push(text(&"é".repeat(2000), &"ü".repeat(2000)));
    let json = serde_json::to_vec(&Proposal {
        changes: changes.clone(),
    })
    .unwrap();
    assert!(json.len() > 16384, "{}", json.len());
    assert!(check(changes).unwrap_err().contains("bytes"));
}

#[test]
fn unknown_kinds_and_fields_do_not_deserialize() {
    let bad = [
        r#"{"changes":[{"kind":"script","code":"x"}]}"#,
        r#"{"changes":[{"kind":"offset","dx":1,"dy":1,"extra":1}]}"#,
        r#"{"changes":[],"extra":1}"#,
        r#"{"changes":[{"dx":1,"dy":1}]}"#,
    ];
    for b in bad {
        assert!(serde_json::from_str::<Proposal>(b).is_err(), "{b}");
    }
}

#[test]
fn the_wire_shape_round_trips() {
    let json = serde_json::to_string(&mixed()).unwrap();
    assert!(json.contains(r#""kind":"style""#));
    assert!(json.contains(r#""kind":"offset""#));
    assert_eq!(serde_json::from_str::<Proposal>(&json).unwrap(), mixed());
    // `token` is optional on the wire.
    let p: Proposal = serde_json::from_str(
        r#"{"changes":[{"kind":"style","property":"gap","from":"1","to":"2"}]}"#,
    )
    .unwrap();
    assert_eq!(p.changes, vec![style("gap", "1", "2")]);
}

#[test]
fn the_summary_is_exactly_the_spec_format() {
    assert_eq!(
        summary(&mixed()),
        "Proposed edit:\n\
         - padding-left 12px → 16px (token --space-4)\n\
         - move by dx 4px, dy 0px\n\
         - text \"Save\" → \"Save changes\""
    );
    assert_eq!(
        summary(&Proposal {
            changes: vec![style("gap", "1px", "2px"), offset(-0.0, 2.5)]
        }),
        "Proposed edit:\n- gap 1px → 2px\n- move by dx 0px, dy 2.5px"
    );
}
