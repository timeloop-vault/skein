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
    /// The content stamp the pane's report was made against.
    pub stamp: String,
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
    if let Ok(meta) = file.metadata()
        && meta.len() > MAX_SERVED
    {
        let mtime = meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis());
        return format!("toolarge:{}:{mtime}", meta.len());
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
            stamp: s.stamp,
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
mod tests;
