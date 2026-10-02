use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;

use tempfile::TempDir;

use super::*;
use crate::review_surface::element::{ElementRect, ElementSource};

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

fn anchor(entry: &str, od_id: Option<&str>, text: &str, src: Option<(&str, u32)>) -> ElementAnchor {
    ElementAnchor {
        entry: entry.into(),
        od_id: od_id.map(Into::into),
        selector: "div".into(),
        tag: "div".into(),
        text: text.into(),
        attrs: BTreeMap::new(),
        source: src.map(|(f, line)| ElementSource {
            file: f.into(),
            line,
            column: None,
            line_text: None,
        }),
        rect: ElementRect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        },
    }
}

#[test]
fn valid_anchor_source() {
    let d = TempDir::new().unwrap();
    write(d.path(), "a.html", "one\ntwo\nthree\n");
    let g = SourceIndex::new(d.path())
        .locate(&anchor("a.html", None, "", Some(("a.html", 2))))
        .unwrap();
    assert_eq!(
        (g.file.as_str(), g.line_start, g.line_end),
        ("a.html", 2, 2)
    );
    assert_eq!(g.via, "anchor_source");
}

#[test]
fn source_beyond_eof_falls_back_to_od_id() {
    let d = TempDir::new().unwrap();
    write(d.path(), "a.html", "x\n<div id=\"hero\">\n");
    let g = SourceIndex::new(d.path())
        .locate(&anchor("a.html", Some("hero"), "", Some(("a.html", 99))))
        .unwrap();
    assert_eq!((g.line_start, g.via), (2, "od_id"));
}

#[test]
fn od_id_needs_quotes() {
    let d = TempDir::new().unwrap();
    write(d.path(), "a.html", "class=heroic\nlet x = 'hero';\n");
    let mut idx = SourceIndex::new(d.path());
    let g = idx
        .locate(&anchor("a.html", Some("hero"), "", None))
        .unwrap();
    assert_eq!(g.line_start, 2);
    let d2 = TempDir::new().unwrap();
    write(d2.path(), "b.html", "heroic hero\n");
    let none = SourceIndex::new(d2.path()).locate(&anchor("b.html", Some("hero"), "", None));
    assert_eq!(none, None);
}

#[test]
fn text_fallback_and_too_short() {
    let d = TempDir::new().unwrap();
    write(d.path(), "a.html", "<p>\nWelcome back\n</p>\n");
    let mut idx = SourceIndex::new(d.path());
    let g = idx
        .locate(&anchor("a.html", None, "\n  Welcome back  \nmore", None))
        .unwrap();
    assert_eq!((g.line_start, g.via), (2, "text"));
    assert_eq!(idx.locate(&anchor("a.html", None, " a b ", None)), None);
}

#[test]
fn entry_directory_searched_first() {
    let d = TempDir::new().unwrap();
    write(d.path(), "a/other.html", "'dup'\n");
    write(d.path(), "z/deep/page.html", "x\n");
    write(d.path(), "z/deep/sib.html", "'dup'\n");
    let g = SourceIndex::new(d.path())
        .locate(&anchor("z/deep/page.html", Some("dup"), "", None))
        .unwrap();
    assert_eq!(g.file, "z/deep/sib.html");
}

#[test]
fn skipped_dirs_are_not_searched() {
    let d = TempDir::new().unwrap();
    write(d.path(), "a.html", "nothing\n");
    write(d.path(), "node_modules/m/x.js", "\"gone\"\n");
    write(d.path(), ".git/x.html", "\"gone\"\n");
    let none = SourceIndex::new(d.path()).locate(&anchor("a.html", Some("gone"), "", None));
    assert_eq!(none, None);
}

#[test]
fn escaping_source_is_not_read() {
    let d = TempDir::new().unwrap();
    let root = d.path().join("root");
    fs::create_dir_all(&root).unwrap();
    fs::write(d.path().join("x.html"), "a\nb\nc\n").unwrap();
    write(&root, "a.html", "nothing\n");
    let none = SourceIndex::new(&root).locate(&anchor("a.html", None, "", Some(("../x.html", 2))));
    assert_eq!(none, None);
}

#[test]
fn walk_deeper_than_max_depth_stops() {
    let d = TempDir::new().unwrap();
    let mut rel = String::new();
    for i in 0..MAX_DEPTH + 5 {
        write!(rel, "d{i}/").unwrap();
    }
    write(
        d.path(),
        &format!("{rel}deep.html"),
        "'deep'
",
    );
    write(
        d.path(),
        "top.html",
        "nothing
",
    );
    let walked = walk_root(d.path());
    assert_eq!(walked, vec!["top.html".to_string()]);
    let none = SourceIndex::new(d.path()).locate(&anchor("top.html", Some("deep"), "", None));
    assert_eq!(none, None);
}

#[test]
fn absolute_source_is_not_read() {
    let d = TempDir::new().unwrap();
    write(
        d.path(),
        "a.html",
        "nothing
",
    );
    let abs = d.path().join("a.html").to_string_lossy().into_owned();
    let none = SourceIndex::new(d.path()).locate(&anchor("a.html", None, "", Some((&abs, 1))));
    assert_eq!(none, None);
}
