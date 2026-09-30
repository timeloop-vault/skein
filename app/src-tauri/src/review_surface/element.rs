//! Element-anchored threads (#434, recon §3).
//!
//! A comment on a rendered element instead of a line. The thread row is
//! an ordinary `review_threads` row (`scope = "element"`, `file_path` =
//! the entry HTML, every line column `NULL`); what it is anchored to
//! lives in the sibling `review_element_anchors` table.
//!
//! Two halves, kept apart on purpose:
//!
//! * `anchor_json` is the picker's evidence, validated here and written
//!   once — the same rule `anchor_lines` follows for line threads.
//! * `last_seen_json` is what the design pane last computed (it alone
//!   has the DOM). Rust cannot re-anchor an element, so it never claims
//!   a position of its own: it reports the pane's last answer only while
//!   a **content stamp** over the files that answer was computed from
//!   still matches the disk, and `unknown` otherwise. An edit made while
//!   the pane was closed therefore can never leave `anchored` claimed.
//!
//! Everything the frontend or an agent can send is host-validated here;
//! the webview is not trusted to have clipped or shaped anything.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};
use skein_review::Placement;

use super::anchoring::{addressed_by_thread, apply_addressed, comments_by_thread, to_thread_dto};
use super::dto::ThreadDto;
use super::git::norm;
use super::thread_scope;
use crate::db::{Database, ReviewElementAnchorRow};
use crate::design::{MAX_SERVED, resolve_under};
use crate::review::now_ms;

const MAX_PATH_CHARS: usize = 500;
const MAX_SELECTOR_CHARS: usize = 1000;
const MAX_TAG_CHARS: usize = 64;
const MAX_TEXT_CHARS: usize = 500;
const MAX_OD_ID_CHARS: usize = 200;
const MAX_ATTRS: usize = 16;
const MAX_ATTR_KEY_CHARS: usize = 64;
const MAX_ATTR_VALUE_CHARS: usize = 300;
const MAX_SOURCE_POS: u32 = 10_000_000;
const MAX_RECT_ABS: f64 = 1e6;
const MAX_SEEN_FILES: usize = 200;

/// The states the design pane may report, best first.
const SEEN_STATES: [&str; 4] = ["anchored", "reanchored", "stale", "lost"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ElementRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElementSource {
    pub file: String,
    pub line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
}

/// The picker's evidence for one element (recon §3.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ElementAnchor {
    pub entry: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub od_id: Option<String>,
    pub selector: String,
    pub tag: String,
    pub text: String,
    pub attrs: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ElementSource>,
    pub rect: ElementRect,
}

/// What the pane reports through `review_element_seen`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeenInput {
    pub state: String,
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default)]
    pub rect: Option<ElementRect>,
    pub files: Vec<String>,
}

/// `last_seen_json` as stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSeen {
    state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rect: Option<ElementRect>,
    files: Vec<String>,
    stamp: String,
    seen_ms: i64,
}

/// The pane's last answer, without the stamp plumbing.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LastSeenDto {
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rect: Option<ElementRect>,
    pub seen_ms: i64,
}

/// `ThreadDto.element`: the stored anchor plus where it stands.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ElementDto {
    pub anchor: ElementAnchor,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<LastSeenDto>,
    /// `anchored` | `reanchored` | `stale` | `lost` | `unknown`. Only
    /// ever the pane's last state while its content stamp still holds.
    pub state: String,
}

impl ElementDto {
    /// `anchored` and `reanchored` are the only states that do not
    /// render as a guess.
    pub fn is_outdated(&self) -> bool {
        !matches!(self.state.as_str(), "anchored" | "reanchored")
    }
}

// ── validation ────────────────────────────────────────────────────

fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// A worktree-relative, `/`-separated path: nothing that could name a
/// place outside the room's folder.
fn check_rel_path(p: &str, what: &str) -> Result<(), String> {
    if p.is_empty() {
        return Err(format!("{what} is empty"));
    }
    if p.chars().count() > MAX_PATH_CHARS {
        return Err(format!("{what} is longer than {MAX_PATH_CHARS} characters"));
    }
    if p.contains('\0') || p.contains('\\') || p.contains(':') || p.starts_with('/') {
        return Err(format!("{what} must be a relative, '/'-separated path"));
    }
    if p.split('/').any(|s| s.is_empty() || s == "." || s == "..") {
        return Err(format!("{what} must not contain empty, '.' or '..' parts"));
    }
    Ok(())
}

fn check_rect(r: &ElementRect, what: &str) -> Result<(), String> {
    for v in [r.x, r.y, r.w, r.h] {
        if !v.is_finite() || v.abs() > MAX_RECT_ABS {
            return Err(format!("{what} has a coordinate out of range"));
        }
    }
    Ok(())
}

fn check_selector(s: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err("the element selector is empty".into());
    }
    if s.chars().count() > MAX_SELECTOR_CHARS {
        return Err(format!(
            "the element selector is longer than {MAX_SELECTOR_CHARS} characters"
        ));
    }
    Ok(())
}

/// Check and normalise an anchor. `file_path` is the thread's own
/// (already normalised) path, which the entry must equal. The result is
/// what gets stored — never the raw input.
pub fn validate_anchor(a: ElementAnchor, file_path: &str) -> Result<ElementAnchor, String> {
    check_rel_path(&a.entry, "the element entry")?;
    let html = Path::new(&a.entry)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"));
    if !html {
        return Err("the element entry must be an .html or .htm file".into());
    }
    if a.entry != file_path {
        return Err("the element entry must be the thread's file".into());
    }
    check_selector(&a.selector)?;
    if a.tag.is_empty()
        || a.tag.chars().count() > MAX_TAG_CHARS
        || !a
            .tag
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err("the element tag must be 1-64 characters of a-z, 0-9 and '-'".into());
    }
    let od_id = match a.od_id {
        Some(id) if id.chars().count() > MAX_OD_ID_CHARS => {
            return Err(format!(
                "the element id is longer than {MAX_OD_ID_CHARS} characters"
            ));
        }
        Some(id) if id.is_empty() => None,
        other => other,
    };
    if a.attrs.len() > MAX_ATTRS {
        return Err(format!(
            "an element may carry at most {MAX_ATTRS} attributes"
        ));
    }
    let mut attrs = BTreeMap::new();
    for (k, v) in a.attrs {
        if k.is_empty()
            || k.chars().count() > MAX_ATTR_KEY_CHARS
            || !k
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '-'))
        {
            return Err(format!("attribute name {k:?} is not allowed"));
        }
        attrs.insert(k, clip(&v, MAX_ATTR_VALUE_CHARS));
    }
    if let Some(s) = &a.source {
        check_rel_path(&s.file, "the source file")?;
        let ok = |n: u32| (1..=MAX_SOURCE_POS).contains(&n);
        if !ok(s.line) || !s.column.is_none_or(ok) {
            return Err("the source position is out of range".into());
        }
    }
    check_rect(&a.rect, "the element rect")?;
    Ok(ElementAnchor {
        entry: a.entry,
        od_id,
        selector: a.selector,
        tag: a.tag,
        text: clip(&a.text, MAX_TEXT_CHARS),
        attrs,
        source: a.source,
        rect: a.rect,
    })
}

/// Check a report and return the file list worth stamping. A bad state,
/// selector or rect refuses the call; a bad or surplus file path is
/// skipped instead, because one odd URL the pane saw must not cost it
/// the whole placement. (A skipped file is simply not watched.)
pub fn validate_seen(s: &SeenInput) -> Result<Vec<String>, String> {
    if !SEEN_STATES.contains(&s.state.as_str()) {
        return Err(format!("unknown element state: {}", s.state));
    }
    if let Some(sel) = &s.selector {
        check_selector(sel)?;
    }
    if let Some(r) = &s.rect {
        check_rect(r, "the reported rect")?;
    }
    Ok(s.files
        .iter()
        .filter(|f| check_rel_path(f, "a reported file").is_ok())
        .take(MAX_SEEN_FILES)
        .cloned()
        .collect())
}

// ── the content stamp ─────────────────────────────────────────────

/// FNV-1a, 64-bit. A change detector, not a security boundary; the same
/// family `skein-review` hashes content with.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn update(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

/// The digest of a file's raw bytes. The preview server records this
/// for what it serves and the freshness check recomputes it from disk,
/// so the two must stay the same form.
pub fn digest_bytes(bytes: &[u8]) -> String {
    let mut h = Fnv::new();
    h.update(bytes);
    format!("{:016x}", h.0)
}

/// One file's digest, or `missing`. Streamed so a large asset is never
/// held in memory, and never read at all past what the preview would
/// serve: those are told apart by size and mtime instead.
fn file_digest(root: &Path, rel: &str) -> String {
    let Ok(full) = resolve_under(root, rel) else {
        return "missing".into();
    };
    let Ok(mut file) = std::fs::File::open(full) else {
        return "missing".into();
    };
    if let Ok(meta) = file.metadata() {
        if meta.len() > MAX_SERVED {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis());
            return format!("toolarge:{}:{mtime}", meta.len());
        }
    }
    let mut h = Fnv::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => h.update(&buf[..n]),
            Err(_) => return "missing".into(),
        }
    }
    format!("{:016x}", h.0)
}

/// Memoised per-file digests, so a pass over many threads that share an
/// entry reads each served file once.
#[derive(Default)]
pub struct DigestCache(HashMap<String, String>);

impl DigestCache {
    /// Start from what the preview server actually served, so a file is
    /// stamped as the bytes the pane loaded, not as whatever is on disk
    /// by the time the report arrives.
    fn seeded(served: &HashMap<String, String>) -> Self {
        Self(served.clone())
    }

    fn digest(&mut self, root: &Path, rel: &str) -> String {
        self.0
            .entry(rel.to_owned())
            .or_insert_with(|| file_digest(root, rel))
            .clone()
    }
}

/// The stamp over a set of files under `root`: sorted, de-duplicated,
/// each as (path, digest-or-`missing`).
pub fn content_stamp(root: &Path, files: &[String], cache: &mut DigestCache) -> String {
    let set: BTreeSet<&str> = files.iter().map(String::as_str).collect();
    let mut h = Fnv::new();
    for f in set {
        h.update(f.as_bytes());
        h.update(&[0]);
        h.update(cache.digest(root, f).as_bytes());
        h.update(b"\n");
    }
    format!("{:016x}", h.0)
}

// ── reading ───────────────────────────────────────────────────────

/// Build the DTO block for one stored anchor. `None` when the stored
/// anchor cannot be parsed (a hand-edited or corrupt row).
pub fn element_dto(
    row: &ReviewElementAnchorRow,
    root: Option<&Path>,
    cache: &mut DigestCache,
) -> Option<ElementDto> {
    let anchor: ElementAnchor = serde_json::from_str(&row.anchor_json).ok()?;
    let stored: Option<StoredSeen> = row
        .last_seen_json
        .as_deref()
        .and_then(|j| serde_json::from_str(j).ok());
    let state = match (&stored, root) {
        (Some(s), Some(root))
            if SEEN_STATES.contains(&s.state.as_str())
                && content_stamp(root, &s.files, cache) == s.stamp =>
        {
            s.state.clone()
        }
        _ => "unknown".to_owned(),
    };
    Some(ElementDto {
        anchor,
        last_seen: stored.map(|s| LastSeenDto {
            state: s.state,
            selector: s.selector,
            rect: s.rect,
            seen_ms: s.seen_ms,
        }),
        state,
    })
}

/// Every anchor row in the room, keyed by thread.
pub(super) fn element_rows_by_thread(
    db: &Database,
    room_id: &str,
) -> Result<HashMap<String, ReviewElementAnchorRow>, String> {
    Ok(db
        .review_element_anchors_for_room(room_id)?
        .into_iter()
        .map(|r| (r.thread_id.clone(), r))
        .collect())
}

/// Stamp the element block onto element threads, and let it decide
/// `outdated`: [`to_thread_dto`] starts them as outdated, and only a
/// state that still holds clears that.
///
/// Applied after the fact, like [`apply_addressed`], because it reads
/// the disk and nothing about line placement wants that.
pub(super) fn apply_element(
    threads: &mut [ThreadDto],
    rows: &HashMap<String, ReviewElementAnchorRow>,
    cwd: Option<&str>,
) {
    let root = cwd.filter(|c| !c.is_empty()).map(Path::new);
    let mut cache = DigestCache::default();
    for t in threads.iter_mut() {
        if t.scope != thread_scope::ELEMENT {
            continue;
        }
        t.element = rows
            .get(&t.id)
            .and_then(|r| element_dto(r, root, &mut cache));
        if let Some(e) = &t.element {
            t.outdated = e.is_outdated();
        }
    }
}

/// Every element thread on `entry`, resolved included, with comments
/// and the addressed/element blocks — the design pane's fetch. Needs no
/// git: nothing here touches a repo.
pub(crate) fn element_threads_impl(
    db: &Database,
    room_id: &str,
    root: &Path,
    entry: &str,
) -> Result<Vec<ThreadDto>, String> {
    let entry = norm(entry);
    let comments = comments_by_thread(db, room_id)?;
    let mut threads: Vec<ThreadDto> = db
        .review_threads_for_room(room_id)?
        .into_iter()
        .filter(|t| {
            t.scope == thread_scope::ELEMENT
                && t.file_path.as_deref().map(norm).as_deref() == Some(entry.as_str())
        })
        .map(|t| to_thread_dto(t, Vec::new(), Placement::Outdated, &comments))
        .collect();
    apply_addressed(&mut threads, &addressed_by_thread(db, room_id)?);
    apply_element(
        &mut threads,
        &element_rows_by_thread(db, room_id)?,
        root.to_str(),
    );
    Ok(threads)
}

// ── writing what the pane saw ─────────────────────────────────────

/// Record the pane's placement of one element thread. Touches
/// `last_seen_json` only — never the anchor.
///
/// `served` is what the preview server last sent for this room, by
/// worktree-relative path. The pane computed its state from those bytes,
/// so a listed file that was served is stamped as served, not as disk
/// is now: an edit landing between the load and this report then reads
/// `unknown` instead of being stamped as the new content with the old
/// state. Only a file never served falls back to disk.
pub(crate) fn seen_impl(
    db: &Database,
    room_id: &str,
    root: &Path,
    thread_id: &str,
    seen: &SeenInput,
    served: &HashMap<String, String>,
) -> Result<(), String> {
    let reported = validate_seen(seen)?;
    let thread = db
        .review_thread(thread_id)?
        .filter(|t| t.room_id == room_id)
        .ok_or("that thread no longer exists")?;
    if thread.scope != thread_scope::ELEMENT || db.review_element_anchor(thread_id)?.is_none() {
        return Err("only an element thread has a placement".into());
    }
    // The entry itself is always part of what the placement was
    // computed from, whether or not the pane listed it.
    let mut files: BTreeSet<String> = reported.into_iter().collect();
    if let Some(entry) = thread.file_path.as_deref() {
        files.insert(entry.to_owned());
    }
    let files: Vec<String> = files.into_iter().collect();
    let stored = StoredSeen {
        state: seen.state.clone(),
        selector: seen.selector.clone(),
        rect: seen.rect.clone(),
        stamp: content_stamp(root, &files, &mut DigestCache::seeded(served)),
        files,
        seen_ms: now_ms(),
    };
    let json = serde_json::to_string(&stored).map_err(|e| e.to_string())?;
    if db.set_review_element_last_seen(thread_id, &json, stored.seen_ms)? {
        Ok(())
    } else {
        Err("that thread has no element anchor".into())
    }
}

#[cfg(test)]
mod tests {
    use super::super::dto::NewThread;
    use super::super::write::add_thread_impl;
    use super::*;

    fn anchor() -> ElementAnchor {
        ElementAnchor {
            entry: "proto/index.html".into(),
            od_id: Some("hero".into()),
            selector: "#root > div:nth-of-type(1)".into(),
            tag: "button".into(),
            text: "First run".into(),
            attrs: BTreeMap::from([("class".to_owned(), "rgroup-h".to_owned())]),
            source: Some(ElementSource {
                file: "proto/shell.jsx".into(),
                line: 16,
                column: Some(9),
            }),
            rect: ElementRect {
                x: 41.0,
                y: 197.0,
                w: 235.0,
                h: 27.0,
            },
        }
    }

    fn check(mutate: impl FnOnce(&mut ElementAnchor)) -> Result<ElementAnchor, String> {
        let mut a = anchor();
        mutate(&mut a);
        validate_anchor(a, "proto/index.html")
    }

    #[test]
    fn a_well_formed_anchor_round_trips() {
        assert_eq!(check(|_| {}).unwrap(), anchor());
    }

    #[test]
    fn entry_must_be_a_safe_html_path_equal_to_the_threads_file() {
        for bad in [
            "",
            "/abs/index.html",
            "../index.html",
            "proto/../index.html",
            "proto\\index.html",
            "C:/index.html",
            "proto/index.html:stream",
            "proto//index.html",
            "proto/./index.html",
            "proto/index.txt",
            "proto/index.html\0",
        ] {
            assert!(check(|a| a.entry = bad.into()).is_err(), "{bad:?}");
        }
        assert!(check(|a| a.entry = "other.html".into()).is_err());
        let long = format!("{}.html", "a".repeat(500));
        assert!(
            validate_anchor(
                ElementAnchor {
                    entry: long.clone(),
                    ..anchor()
                },
                &long
            )
            .is_err()
        );
        assert!(
            validate_anchor(
                ElementAnchor {
                    entry: "P/I.HTM".into(),
                    ..anchor()
                },
                "P/I.HTM"
            )
            .is_ok()
        );
    }

    #[test]
    fn selector_tag_and_id_are_capped() {
        assert!(check(|a| a.selector = String::new()).is_err());
        assert!(check(|a| a.selector = "a".repeat(1001)).is_err());
        assert!(check(|a| a.selector = "a".repeat(1000)).is_ok());
        assert!(check(|a| a.tag = String::new()).is_err());
        assert!(check(|a| a.tag = "Button".into()).is_err());
        assert!(check(|a| a.tag = "my_tag".into()).is_err());
        assert!(check(|a| a.tag = "x".repeat(65)).is_err());
        assert!(check(|a| a.tag = "my-tag2".into()).is_ok());
        assert!(check(|a| a.od_id = Some("x".repeat(201))).is_err());
        assert_eq!(
            check(|a| a.od_id = Some(String::new())).unwrap().od_id,
            None
        );
    }

    #[test]
    fn text_is_clipped_on_a_char_boundary() {
        let got = check(|a| a.text = "å".repeat(800)).unwrap();
        assert_eq!(got.text.chars().count(), 500);
    }

    #[test]
    fn attrs_are_counted_named_and_clipped() {
        let many: BTreeMap<String, String> =
            (0..17).map(|i| (format!("k{i}"), "v".to_owned())).collect();
        assert!(check(|a| a.attrs = many).is_err());
        let sixteen: BTreeMap<String, String> =
            (0..16).map(|i| (format!("k{i}"), "v".to_owned())).collect();
        assert!(check(|a| a.attrs = sixteen).is_ok());
        for bad in ["", "a b", "a=b", "é", &"k".repeat(65)] {
            let attrs = BTreeMap::from([(bad.to_owned(), "v".to_owned())]);
            assert!(check(|a| a.attrs = attrs).is_err(), "{bad:?}");
        }
        let attrs = BTreeMap::from([("data-x:y_z".to_owned(), "v".repeat(400))]);
        let got = check(|a| a.attrs = attrs).unwrap();
        assert_eq!(got.attrs["data-x:y_z"].len(), 300);
    }

    #[test]
    fn source_and_rect_are_range_checked() {
        let src = |file: &str, line, column| {
            Some(ElementSource {
                file: file.into(),
                line,
                column,
            })
        };
        assert!(check(|a| a.source = src("../x.jsx", 1, None)).is_err());
        assert!(check(|a| a.source = src("/x.jsx", 1, None)).is_err());
        assert!(check(|a| a.source = src("x.jsx", 0, None)).is_err());
        assert!(check(|a| a.source = src("x.jsx", 10_000_001, None)).is_err());
        assert!(check(|a| a.source = src("x.jsx", 1, Some(0))).is_err());
        assert!(check(|a| a.source = src("x.jsx", 10_000_000, Some(10_000_000))).is_ok());
        assert!(check(|a| a.source = None).is_ok());
        assert!(check(|a| a.rect.w = f64::NAN).is_err());
        assert!(check(|a| a.rect.x = f64::INFINITY).is_err());
        assert!(check(|a| a.rect.y = 1_000_001.0).is_err());
        assert!(check(|a| a.rect.y = -1_000_000.0).is_ok());
    }

    fn seen(state: &str, files: &[&str]) -> SeenInput {
        SeenInput {
            state: state.into(),
            selector: Some("#a".into()),
            rect: Some(anchor().rect),
            files: files.iter().map(|f| (*f).to_owned()).collect(),
        }
    }

    #[test]
    fn a_seen_report_is_validated() {
        assert_eq!(
            validate_seen(&seen("anchored", &["proto/a.jsx"])).unwrap(),
            ["proto/a.jsx"]
        );
        assert!(validate_seen(&seen("guessing", &[])).is_err());
        // A bad or surplus file is skipped, never a reason to refuse.
        assert_eq!(
            validate_seen(&seen("lost", &["../x", "ok.js", "a\\b", "http://x/y"])).unwrap(),
            ["ok.js"]
        );
        let many: Vec<String> = (0..201).map(|i| format!("f{i}.js")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(validate_seen(&seen("lost", &refs)).unwrap().len(), 200);
        let mut s = seen("lost", &[]);
        s.selector = Some(String::new());
        assert!(validate_seen(&s).is_err());
        let mut s = seen("lost", &[]);
        s.rect = Some(ElementRect {
            x: 1e7,
            ..anchor().rect
        });
        assert!(validate_seen(&s).is_err());
    }

    // ── against a database and a folder ──

    /// `seen_impl` with nothing served: every file stamps from disk.
    fn seen_impl(
        db: &Database,
        room_id: &str,
        root: &Path,
        thread_id: &str,
        seen: &SeenInput,
    ) -> Result<(), String> {
        super::seen_impl(db, room_id, root, thread_id, seen, &HashMap::new())
    }

    struct Fixture {
        db: Database,
        tmp: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::TempDir::new().unwrap();
            let db = Database::open(&tmp.path().join("skein.db")).unwrap();
            std::fs::create_dir_all(tmp.path().join("proto")).unwrap();
            std::fs::write(tmp.path().join("proto/index.html"), "<p>one</p>").unwrap();
            std::fs::write(tmp.path().join("proto/shell.jsx"), "v1").unwrap();
            Self { db, tmp }
        }

        fn cwd(&self) -> &str {
            self.tmp.path().to_str().unwrap()
        }

        fn new_thread(scope: &str, element: Option<ElementAnchor>) -> NewThread {
            NewThread {
                scope: scope.into(),
                file_path: Some("proto/index.html".into()),
                commit_sha: None,
                side: None,
                line_start: None,
                line_end: None,
                anchor_lines: Vec::new(),
                element,
                body: "make it bigger".into(),
            }
        }

        fn add_element(&self) -> ThreadDto {
            add_thread_impl(
                &self.db,
                "r1",
                self.cwd(),
                &Self::new_thread("element", Some(anchor())),
            )
            .unwrap()
        }
    }

    #[test]
    fn an_element_thread_stores_null_lines_and_an_anchor_row() {
        let f = Fixture::new();
        let dto = f.add_element();
        assert_eq!(dto.scope, "element");
        assert_eq!(dto.file_path.as_deref(), Some("proto/index.html"));
        assert_eq!((dto.line_start, dto.line_end), (None, None));

        let row = f.db.review_thread(&dto.id).unwrap().unwrap();
        assert_eq!(row.file_path.as_deref(), Some("proto/index.html"));
        assert_eq!((row.side, row.line_start, row.line_end), (None, None, None));
        assert_eq!((row.anchor_hash, row.anchor_lines), (None, None));

        let a = f.db.review_element_anchor(&dto.id).unwrap().unwrap();
        assert_eq!(a.file_path, "proto/index.html");
        assert_eq!(a.room_id, "r1");
        assert_eq!(
            serde_json::from_str::<ElementAnchor>(&a.anchor_json).unwrap(),
            anchor()
        );
        assert_eq!(dto.element.unwrap().anchor, anchor());
    }

    #[test]
    fn the_stored_anchor_is_the_validated_struct_not_the_raw_input() {
        let f = Fixture::new();
        let mut a = anchor();
        a.text = "x".repeat(900);
        let dto = add_thread_impl(
            &f.db,
            "r1",
            f.cwd(),
            &Fixture::new_thread("element", Some(a)),
        )
        .unwrap();
        let stored = f.db.review_element_anchor(&dto.id).unwrap().unwrap();
        let back: ElementAnchor = serde_json::from_str(&stored.anchor_json).unwrap();
        assert_eq!(back.text.chars().count(), 500);
    }

    #[test]
    fn scope_element_needs_an_element_and_nothing_else_may_carry_one() {
        let f = Fixture::new();
        let err = add_thread_impl(&f.db, "r1", f.cwd(), &Fixture::new_thread("element", None));
        assert!(err.is_err());
        let err = add_thread_impl(
            &f.db,
            "r1",
            f.cwd(),
            &Fixture::new_thread("file", Some(anchor())),
        );
        assert!(err.is_err());
        let mut bad = anchor();
        bad.selector = String::new();
        let err = add_thread_impl(
            &f.db,
            "r1",
            f.cwd(),
            &Fixture::new_thread("element", Some(bad)),
        );
        assert!(err.is_err());
        assert!(f.db.review_threads_for_room("r1").unwrap().is_empty());
        let mut other = anchor();
        other.entry = "proto/other.html".into();
        let err = add_thread_impl(
            &f.db,
            "r1",
            f.cwd(),
            &Fixture::new_thread("element", Some(other)),
        );
        assert!(err.is_err(), "the entry must equal the thread's file");
    }

    #[test]
    fn a_new_element_thread_is_never_unmoved_and_starts_unknown() {
        let f = Fixture::new();
        let dto = f.add_element();
        assert_eq!(dto.placement, "element");
        assert!(dto.outdated);
        let e = dto.element.unwrap();
        assert_eq!(e.state, "unknown");
        assert!(e.last_seen.is_none());
    }

    #[test]
    fn set_last_seen_never_changes_the_anchor() {
        let f = Fixture::new();
        let id = f.add_element().id;
        let before = f.db.review_element_anchor(&id).unwrap().unwrap();
        let root = f.tmp.path();
        seen_impl(&f.db, "r1", root, &id, &seen("lost", &["proto/shell.jsx"])).unwrap();
        let after = f.db.review_element_anchor(&id).unwrap().unwrap();
        assert_eq!(after.anchor_json, before.anchor_json);
        assert_ne!(after.last_seen_json, before.last_seen_json);
    }

    #[test]
    fn a_reported_state_holds_until_a_served_file_changes() {
        let f = Fixture::new();
        let id = f.add_element().id;
        let root = f.tmp.path();
        seen_impl(
            &f.db,
            "r1",
            root,
            &id,
            &seen("anchored", &["proto/shell.jsx"]),
        )
        .unwrap();

        let state = |f: &Fixture| {
            element_threads_impl(&f.db, "r1", f.tmp.path(), "proto/index.html")
                .unwrap()
                .remove(0)
        };
        let t = state(&f);
        assert_eq!(t.element.as_ref().unwrap().state, "anchored");
        assert!(!t.outdated);
        assert_eq!(t.placement, "element");

        // A file the pane listed changes while the pane is closed.
        std::fs::write(root.join("proto/shell.jsx"), "v2").unwrap();
        let t = state(&f);
        let e = t.element.unwrap();
        assert_eq!(e.state, "unknown");
        assert!(t.outdated);
        assert_eq!(
            e.last_seen.unwrap().state,
            "anchored",
            "the report is kept, not believed"
        );

        // So does the entry, even though the pane never listed it.
        seen_impl(&f.db, "r1", root, &id, &seen("reanchored", &[])).unwrap();
        assert_eq!(state(&f).element.unwrap().state, "reanchored");
        std::fs::write(root.join("proto/index.html"), "<p>two</p>").unwrap();
        assert_eq!(state(&f).element.unwrap().state, "unknown");

        // A listed file that disappears is a change too.
        seen_impl(
            &f.db,
            "r1",
            root,
            &id,
            &seen("anchored", &["proto/shell.jsx"]),
        )
        .unwrap();
        assert_eq!(state(&f).element.unwrap().state, "anchored");
        std::fs::remove_file(root.join("proto/shell.jsx")).unwrap();
        assert_eq!(state(&f).element.unwrap().state, "unknown");
    }

    #[test]
    fn a_file_changed_after_it_was_served_reads_unknown() {
        let f = Fixture::new();
        let id = f.add_element().id;
        let root = f.tmp.path();
        let read = |f: &Fixture| {
            element_threads_impl(&f.db, "r1", f.tmp.path(), "proto/index.html")
                .unwrap()
                .remove(0)
                .element
                .unwrap()
                .state
        };
        let served = |bytes: &[u8]| {
            HashMap::from([
                ("proto/shell.jsx".to_owned(), digest_bytes(bytes)),
                ("proto/index.html".to_owned(), digest_bytes(b"<p>one</p>")),
            ])
        };

        // Served v1, then the file became v2 before the report landed:
        // the stamp is v1's, so the freshness check disagrees.
        std::fs::write(root.join("proto/shell.jsx"), "v2").unwrap();
        super::seen_impl(
            &f.db,
            "r1",
            root,
            &id,
            &seen("anchored", &["proto/shell.jsx"]),
            &served(b"v1"),
        )
        .unwrap();
        assert_eq!(read(&f), "unknown");

        // Served what is still on disk: the state is kept.
        super::seen_impl(
            &f.db,
            "r1",
            root,
            &id,
            &seen("anchored", &["proto/shell.jsx"]),
            &served(b"v2"),
        )
        .unwrap();
        assert_eq!(read(&f), "anchored");
    }

    #[test]
    fn a_file_past_the_serve_cap_is_not_read() {
        let f = Fixture::new();
        let big = f.tmp.path().join("proto/big.bin");
        let file = std::fs::File::create(&big).unwrap();
        file.set_len(MAX_SERVED + 1).unwrap();
        let d = file_digest(f.tmp.path(), "proto/big.bin");
        assert!(
            d.starts_with(&format!("toolarge:{}:", MAX_SERVED + 1)),
            "{d}"
        );
    }

    #[test]
    fn stale_and_lost_render_as_outdated() {
        let f = Fixture::new();
        let id = f.add_element().id;
        for (st, outdated) in [("stale", true), ("lost", true), ("anchored", false)] {
            seen_impl(&f.db, "r1", f.tmp.path(), &id, &seen(st, &[])).unwrap();
            let t = element_threads_impl(&f.db, "r1", f.tmp.path(), "proto/index.html")
                .unwrap()
                .remove(0);
            assert_eq!(t.outdated, outdated, "{st}");
            assert_eq!(t.element.unwrap().state, st);
        }
    }

    #[test]
    fn seen_refuses_other_rooms_other_scopes_and_unknown_threads() {
        let f = Fixture::new();
        let id = f.add_element().id;
        let root = f.tmp.path();
        assert!(seen_impl(&f.db, "r2", root, &id, &seen("lost", &[])).is_err());
        assert!(seen_impl(&f.db, "r1", root, "nope", &seen("lost", &[])).is_err());
        assert!(seen_impl(&f.db, "r1", root, &id, &seen("bogus", &[])).is_err());
        let file =
            add_thread_impl(&f.db, "r1", f.cwd(), &Fixture::new_thread("file", None)).unwrap();
        assert!(seen_impl(&f.db, "r1", root, &file.id, &seen("lost", &[])).is_err());
    }

    #[test]
    fn element_threads_are_scoped_to_the_entry_and_include_resolved() {
        let f = Fixture::new();
        let a = f.add_element().id;
        let mut other = anchor();
        other.entry = "proto/two.html".into();
        let mut nt = Fixture::new_thread("element", Some(other));
        nt.file_path = Some("proto/two.html".into());
        add_thread_impl(&f.db, "r1", f.cwd(), &nt).unwrap();
        f.db.set_review_thread_resolved(&a, Some(5), 5).unwrap();
        let got = element_threads_impl(&f.db, "r1", f.tmp.path(), "proto/index.html").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, a);
        assert_eq!(got[0].resolved_ms, Some(5));
        assert_eq!(got[0].comments.len(), 1);
        assert!(
            element_threads_impl(&f.db, "r2", f.tmp.path(), "proto/index.html")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn the_agent_is_told_the_state_and_never_unmoved() {
        use crate::agent_api::auth::Caller;
        use crate::agent_api::verbs::{GetCommentArgs, ListArgs, get_comment, list_comments};

        let f = Fixture::new();
        let id = f.add_element().id;
        let caller = Caller {
            room_id: "r1".into(),
            cwd: Some(f.cwd().to_owned()),
            harness_id: None,
            harness_label: None,
        };
        let list = |f: &Fixture| {
            list_comments(&f.db, &caller, &ListArgs::default())
                .unwrap()
                .threads
                .remove(0)
        };

        let t = list(&f);
        assert_eq!(t.placement, Some("element"));
        assert!(t.outdated, "never seen, so unknown, so a guess");
        assert_eq!((t.line_start, t.line_end), (None, None));
        assert_eq!(t.element.as_ref().unwrap()["state"], "unknown");

        seen_impl(&f.db, "r1", f.tmp.path(), &id, &seen("anchored", &[])).unwrap();
        let t = list(&f);
        assert!(!t.outdated);
        assert_eq!(t.element.as_ref().unwrap()["state"], "anchored");
        assert_eq!(t.element.as_ref().unwrap()["anchor"]["tag"], "button");

        let detail = get_comment(&f.db, &caller, &GetCommentArgs { thread_id: id }).unwrap();
        assert_eq!(detail.thread.placement, Some("element"));
        assert!(detail.diff_context.is_none() && detail.current_context.is_none());

        std::fs::write(f.tmp.path().join("proto/index.html"), "<p>two</p>").unwrap();
        assert!(list(&f).outdated);
    }

    #[test]
    fn deleting_a_thread_deletes_its_anchor() {
        let f = Fixture::new();
        let id = f.add_element().id;
        assert!(f.db.delete_review_thread(&id).unwrap());
        assert!(f.db.review_element_anchor(&id).unwrap().is_none());

        let id = f.add_element().id;
        let comment = f.db.review_comments_for_room("r1").unwrap().remove(0);
        assert_eq!(
            f.db.delete_review_comment(&comment.id).unwrap(),
            Some(id.clone())
        );
        assert!(f.db.review_element_anchor(&id).unwrap().is_none());
    }

    #[test]
    fn a_corrupt_anchor_row_degrades_instead_of_failing() {
        let f = Fixture::new();
        let id = f.add_element().id;
        rusqlite::Connection::open(f.tmp.path().join("skein.db"))
            .unwrap()
            .execute(
                "UPDATE review_element_anchors SET anchor_json = 'junk' WHERE thread_id = ?1",
                [&id],
            )
            .unwrap();
        let got = element_threads_impl(&f.db, "r1", f.tmp.path(), "proto/index.html").unwrap();
        assert!(got[0].element.is_none());
        assert_eq!(got[0].placement, "element");
        assert!(got[0].outdated);
    }
}
