//! Ported case for case from `branchName.test.ts` and `roomName.test.ts`,
//! plus the `derivePromptFirstLine` contract.

use super::*;

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn task_slug_table() {
    let long = "a".repeat(40);
    let cases: [(&str, String); 8] = [
        ("Fix the thing", "fix-the-thing".into()),
        ("  spaced out  ", "spaced-out".into()),
        ("UPPER-CASE_stuff", "upper-case-stuff".into()),
        ("", "task".into()),
        ("   ", "task".into()),
        ("!!!", "task".into()),
        (&long, "a".repeat(28)),
        // Non-ASCII letters are not [a-z0-9], so they collapse like JS.
        ("日本語 task", "task".into()),
    ];
    for (input, expected) in cases {
        assert_eq!(task_slug(input), expected, "{input:?}");
    }
}

#[test]
fn default_template() {
    assert_eq!(DEFAULT_BRANCH_TEMPLATE, "skein/{slug}");
}

#[test]
fn apply_branch_template_table() {
    let cases = [
        ("skein/{slug}", "fix-x", "skein/fix-x"),
        ("{slug}/{slug}", "x", "x/x"),
        ("feat/", "fix-x", "feat/fix-x"),
        ("feat", "fix-x", "featfix-x"),
        ("", "fix-x", "fix-x"),
        ("   ", "fix-x", "fix-x"),
        ("  skein/{slug}  ", "fix-x", "skein/fix-x"),
    ];
    for (template, slug, expected) in cases {
        assert_eq!(
            apply_branch_template(template, slug),
            expected,
            "{template:?}"
        );
    }
}

#[test]
fn template_from_branch_table() {
    let cases = [
        ("feat/213-x", "feat/{slug}"),
        ("skein/fix-thing", "skein/{slug}"),
        ("a/b/c-thing", "a/b/{slug}"),
        ("foo", "{slug}"),
    ];
    for (branch, expected) in cases {
        assert_eq!(template_from_branch(branch), expected, "{branch:?}");
    }
}

#[test]
fn worktree_leaf_table() {
    let long = "x".repeat(60);
    assert_eq!(worktree_leaf("feat/My Thing"), "my-thing");
    assert_eq!(worktree_leaf("solo-branch"), "solo-branch");
    // No length cap, unlike task_slug.
    assert_eq!(worktree_leaf(&format!("feat/{long}")), long);
    assert_eq!(worktree_leaf("feat/"), "task");
    assert_eq!(worktree_leaf("feat/!!!"), "task");
}

#[test]
fn branch_name_problem_accepts() {
    for name in ["skein/fix-thing", "feat/213-x", "main", "feat/x@y"] {
        assert_eq!(branch_name_problem(name), None, "{name:?}");
    }
}

#[test]
fn branch_name_problem_rejects_with_messages() {
    let special = "contains a space or a special character";
    let cases = [
        ("", "can't be empty"),
        ("@", "can't be just \"@\""),
        ("-feat", "can't start with -"),
        ("/feat", "can't start or end with /"),
        ("feat/", "can't start or end with /"),
        ("a//b", "can't contain //"),
        ("a..b", "can't contain .."),
        ("feat/a..b", "can't contain .."),
        ("feat/x@{y", "can't contain @{"),
        ("feat.", "can't end with ."),
        ("feat/my thing", special),
        ("feat/x\ty", special),
        ("feat/x\ny", special),
        ("feat/x\u{7f}y", special),
        (".feat", "no part can start with ."),
        ("feat/.hidden", "no part can start with ."),
        ("x.lock", "can't end with .lock"),
        ("feat/x.lock", "can't end with .lock"),
    ];
    for (name, expected) in cases {
        assert_eq!(branch_name_problem(name), Some(expected), "{name:?}");
    }
    for ch in ['~', '^', ':', '?', '*', '[', '\\'] {
        let name = format!("feat/x{ch}y");
        assert_eq!(branch_name_problem(&name), Some(special), "{name:?}");
    }
}

#[test]
fn branch_collision_table() {
    assert!(branch_collision("main", &strs(&["main", "dev"])));
    assert!(!branch_collision("feat/x", &strs(&["main", "dev"])));
    assert!(!branch_collision("main2", &strs(&["main"])));
    assert!(!branch_collision("main", &[]));
}

#[test]
fn branch_field_problem_priority() {
    let main = strs(&["main"]);
    assert_eq!(branch_field_problem("feat/x", &main, false), None);
    assert_eq!(
        branch_field_problem("", &main, true),
        Some("can't be empty")
    );
    assert_eq!(
        branch_field_problem("main", &strs(&["main", "dev"]), false),
        Some("branch already exists")
    );
    assert_eq!(
        branch_field_problem("feat/x", &main, true),
        Some("worktree folder already exists")
    );
}

#[test]
fn default_room_name_table() {
    let cases = [
        ("D:\\code\\skein", "skein"),
        ("/a/skein-wt/skein-241/", "skein-241"),
        ("D:\\code\\skein\\", "skein"),
        ("/a/skein-wt\\skein-241", "skein-241"),
        ("/", "/"),
        ("C:\\", "C:"),
    ];
    for (cwd, expected) in cases {
        assert_eq!(default_room_name(cwd), expected, "{cwd:?}");
    }
}

#[test]
fn derive_prompt_first_line_table() {
    let d = derive_prompt_first_line;
    assert_eq!(d(Some("  Title "), Some("x")), Some("Title".into()));
    assert_eq!(
        d(Some("  "), Some("\r\n \r\n  Do it  \r\nmore")),
        Some("Do it".into())
    );
    assert_eq!(d(None, Some("a\nb")), Some("a".into()));
    assert_eq!(d(None, Some(" \n\n")), None);
    assert_eq!(d(None, None), None);
    // Capped at 200 UTF-16 units; astral chars count as two.
    let long = "y".repeat(300);
    assert_eq!(d(Some(&long), None).map(|s| s.len()), Some(200));
    let astral = "😀".repeat(150);
    assert_eq!(d(None, Some(&astral)).map(|s| s.chars().count()), Some(100));
}

#[test]
fn js_trim_quirks() {
    // JS trims U+FEFF but not U+0085.
    assert_eq!(task_slug("\u{feff}ab\u{feff}"), "ab");
    assert_eq!(apply_branch_template("\u{feff}", "s"), "s");
    assert_eq!(js_trim("\u{85}x"), "\u{85}x");
}
