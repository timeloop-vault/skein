//! An element thread seen from its JSX source file (#467).
//!
//! The thread is one row, filed under its entry HTML. When the picker
//! also captured where the element was written (`source`), the same
//! thread is *mirrored* into that source file's view, so the reviewer
//! reading `shell.jsx` meets the comment beside the code it is about.
//!
//! Two rules keep a mirror honest:
//!
//! * **The line is vouched for by text, or not at all.** The anchor holds
//!   the source line's text as served at pick time; the mirror re-matches
//!   it against the file as this scope sees it and claims a line only on
//!   a verbatim hit. A partial match, an old thread with no captured
//!   text is `outdated`, with no line. A line past the end is clamped
//!   to the last line first, so the text is still found if it moved up.
//! * **A mirror writes nothing.** No position goes back to the database
//!   (the thread has no line columns), and it is never counted in the
//!   source file's `thread_count`: sign-off sees the thread once.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use skein_review::{Anchor, Placement, Reanchorer, Side};

use super::anchoring::{PlaceCtx, apply_addressed, to_thread_dto};
use super::dto::{AddressedDto, CommentDto, ReviewFileDto, ThreadDto};
use super::element::{DigestCache, ElementAnchor, ElementSource, element_dto};
use super::git::norm;
use super::thread_scope;
use crate::db::{ReviewElementAnchorRow, ReviewThreadRow};

/// One element thread and where its anchor says it was written.
pub(super) struct Mirror {
    row: ReviewThreadRow,
    source: ElementSource,
}

/// Mirrors keyed by the normalised source file they appear in.
pub(super) type SourceIndex = HashMap<String, Vec<Mirror>>;

/// Index the room's element threads by their anchor's source file. A
/// thread whose source is its own entry, or whose anchor cannot be
/// parsed, has no mirror.
pub(super) fn index_sources(
    threads: &[ReviewThreadRow],
    elements: &HashMap<String, ReviewElementAnchorRow>,
) -> SourceIndex {
    let mut index = SourceIndex::new();
    for t in threads {
        let Some(own) = t.file_path.as_deref().map(norm) else {
            continue;
        };
        if t.scope != thread_scope::ELEMENT {
            continue;
        }
        let Some(source) = elements
            .get(&t.id)
            .and_then(|r| serde_json::from_str::<ElementAnchor>(&r.anchor_json).ok())
            .and_then(|a| a.source)
        else {
            continue;
        };
        let file = norm(&source.file);
        if file == own {
            continue;
        }
        index.entry(file).or_default().push(Mirror {
            row: t.clone(),
            source,
        });
    }
    index
}

/// Per source file: (all mirrors, unresolved mirrors).
pub(super) fn source_counts(index: &SourceIndex) -> HashMap<String, (usize, usize)> {
    index
        .iter()
        .map(|(file, ms)| {
            let open = ms.iter().filter(|m| m.row.resolved_ms.is_none()).count();
            (file.clone(), (ms.len(), open))
        })
        .collect()
}

/// Stamp the source counts onto every listed file.
pub(super) fn apply_source_counts(
    files: &mut [ReviewFileDto],
    counts: &HashMap<String, (usize, usize)>,
) {
    for f in files {
        let (all, open) = counts.get(&f.path).copied().unwrap_or((0, 0));
        f.source_thread_count = all;
        f.source_unresolved_count = open;
    }
}

/// List a source file that carries unresolved mirrored threads but has no
/// row yet, as `unchanged` with zero diff counts — the sibling of
/// `push_element_only_files`, for the same reason: sign-off counts the
/// thread, so the reviewer must be able to open the file it points into.
pub(super) fn push_source_only_files(
    files: &mut Vec<ReviewFileDto>,
    index: &SourceIndex,
    per_file: &HashMap<String, (usize, usize)>,
    viewed: &HashMap<String, String>,
    pending_paths: &[String],
    hash_of: impl Fn(&str) -> String,
) {
    let listed: HashSet<String> = files.iter().map(|f| f.path.clone()).collect();
    let counts = source_counts(index);
    let wanted: BTreeSet<&String> = index
        .keys()
        .filter(|p| !listed.contains(*p) && counts.get(*p).is_some_and(|c| c.1 > 0))
        .collect();
    for path in wanted {
        let (thread_count, unresolved_count) = per_file.get(path).copied().unwrap_or((0, 0));
        let (source_thread_count, source_unresolved_count) =
            counts.get(path).copied().unwrap_or((0, 0));
        let hash = hash_of(path);
        let marked = viewed.get(path);
        files.push(ReviewFileDto {
            name: path.rsplit('/').next().unwrap_or(path).to_owned(),
            change: "unchanged",
            additions: 0,
            deletions: 0,
            binary: false,
            viewed: marked.is_some_and(|h| *h == hash),
            changed_since_viewed: marked.is_some_and(|h| *h != hash),
            content_hash: hash,
            thread_count,
            unresolved_count,
            source_thread_count,
            source_unresolved_count,
            has_pending: pending_paths.contains(path),
            harness_id: None,
            path: path.clone(),
        });
    }
}

/// The mirrors that belong in `key`'s view, placed against the file as
/// `ctx`'s scope sees it. Resolved ones are included, like any thread.
pub(super) fn mirror_threads(
    index: &SourceIndex,
    key: &str,
    ctx: &PlaceCtx<'_>,
    comments: &HashMap<String, Vec<CommentDto>>,
    addressed: &HashMap<String, AddressedDto>,
    elements: &HashMap<String, ReviewElementAnchorRow>,
    cwd: Option<&str>,
) -> Vec<ThreadDto> {
    let Some(mirrors) = index.get(key) else {
        return Vec::new();
    };
    let text = ctx.anchor_text(Side::New, key);
    let lines = Reanchorer::new(&text);
    let root = cwd.filter(|c| !c.is_empty()).map(Path::new);
    let mut cache = DigestCache::default();

    let mut out: Vec<ThreadDto> = mirrors
        .iter()
        .map(|m| {
            let anchor_lines: Vec<String> = m.source.line_text.iter().cloned().collect();
            let mut dto = to_thread_dto(m.row.clone(), anchor_lines, Placement::Outdated, comments);
            dto.element = elements
                .get(&m.row.id)
                .and_then(|r| element_dto(r, root, &mut cache));
            dto.via_source = true;
            // The text match alone vouches for the line; the element's
            // own pane state says nothing about this file.
            let placed = place(&lines, &m.source);
            dto.placement = placed.map_or("outdated", |(name, _)| name);
            dto.line_start = placed.map(|(_, line)| line);
            dto.line_end = dto.line_start;
            dto.outdated = placed.is_none();
            dto.confidence = None;
            dto
        })
        .collect();
    apply_addressed(&mut out, addressed);
    out
}

/// A verbatim hit for the captured line, or `None`.
fn place(lines: &Reanchorer<'_>, source: &ElementSource) -> Option<(&'static str, usize)> {
    let text = source.line_text.as_ref()?;
    let line = usize::try_from(source.line).ok()?;
    if line == 0 || lines.is_empty() {
        return None;
    }
    // A start past the end is clamped, so a verbatim hit higher up can
    // still be found (as `moved`).
    let line = line.min(lines.len());
    match lines.place(&Anchor::new(Side::New, line, vec![text.clone()])) {
        Placement::Unmoved { start } => Some(("unmoved", start)),
        Placement::Moved { start } => Some(("moved", start)),
        Placement::Shifted { .. } | Placement::Outdated => None,
    }
}

#[cfg(test)]
mod tests;
