//! Line diffing between a baseline and the working tree, and the two
//! splices that resolve a hunk.
//!
//! The DTO shape (`header` + `lines` with `kind` / `content` /
//! `oldLineno` / `newLineno`) deliberately matches `skein-git`'s
//! `FileDiff`, so the Diff card renders a pending hunk with the same
//! component it already uses for a git diff.

use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};

/// Context lines kept either side of a change — the unified-diff default.
pub const CONTEXT_RADIUS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineKind {
    Context,
    Add,
    Delete,
}

/// One rendered diff line. `content` has its terminator stripped: it is
/// for display and for identity, never for reconstructing the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HunkLine {
    pub kind: LineKind,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_lineno: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_lineno: Option<usize>,
}

/// One pending hunk. `old_*` are baseline coordinates, `new_*` working
/// tree, both 1-based, both covering the context lines as well as the
/// changed ones — that is the range accept and reject splice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    pub header: String,
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub lines: Vec<HunkLine>,
}

impl Hunk {
    /// Added lines, for the tab's `+n`.
    pub fn additions(&self) -> usize {
        self.lines
            .iter()
            .filter(|l| l.kind == LineKind::Add)
            .count()
    }

    /// Deleted lines, for the tab's `−n`.
    pub fn deletions(&self) -> usize {
        self.lines
            .iter()
            .filter(|l| l.kind == LineKind::Delete)
            .count()
    }

    /// Identity for accept/reject: everything but the cosmetic header.
    fn same_change_as(&self, other: &Hunk) -> bool {
        self.old_start == other.old_start
            && self.old_lines == other.old_lines
            && self.new_start == other.new_start
            && self.new_lines == other.new_lines
            && self.lines.len() == other.lines.len()
            && self
                .lines
                .iter()
                .zip(&other.lines)
                .all(|(a, b)| a.kind == b.kind && a.content == b.content)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReviewError {
    /// A submitted hunk is not in the current baseline→disk diff. The
    /// file moved between render and click; the caller must refresh
    /// rather than have us guess which hunk was meant.
    #[error("the file changed since these hunks were rendered — refresh and try again")]
    Stale,
    /// Nothing to do: no hunk was submitted.
    #[error("no hunks selected")]
    Empty,
}

/// Split into lines that keep their own terminators, so
/// `split_lines(s).concat() == s` for every input. Both the diff and
/// the splices work on these slices, which is what makes accept and
/// reject byte-exact.
pub fn split_lines(s: &str) -> Vec<&str> {
    s.split_inclusive('\n').collect()
}

fn strip_terminator(s: &str) -> &str {
    let s = s.strip_suffix('\n').unwrap_or(s);
    s.strip_suffix('\r').unwrap_or(s)
}

/// The hunks between a baseline and the working tree.
///
/// An empty result means the file is fully reviewed — that, and only
/// that, is what clears a tab from the Diff card.
pub fn diff_lines(baseline: &str, current: &str) -> Vec<Hunk> {
    let old = split_lines(baseline);
    let new = split_lines(current);
    let diff = TextDiff::from_slices(&old, &new);

    let mut out = Vec::new();
    for group in diff.grouped_ops(CONTEXT_RADIUS) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        let old_from = first.old_range().start;
        let old_to = last.old_range().end;
        let new_from = first.new_range().start;
        let new_to = last.new_range().end;

        let mut lines = Vec::new();
        for op in &group {
            for change in diff.iter_changes(op) {
                let kind = match change.tag() {
                    ChangeTag::Equal => LineKind::Context,
                    ChangeTag::Delete => LineKind::Delete,
                    ChangeTag::Insert => LineKind::Add,
                };
                lines.push(HunkLine {
                    kind,
                    content: strip_terminator(change.value()).to_string(),
                    old_lineno: change.old_index().map(|i| i + 1),
                    new_lineno: change.new_index().map(|i| i + 1),
                });
            }
        }

        let old_lines = old_to - old_from;
        let new_lines = new_to - new_from;
        // git's convention: a zero-length side names the line the
        // change sits after, not a line that exists.
        let old_disp = if old_lines == 0 {
            old_from
        } else {
            old_from + 1
        };
        let new_disp = if new_lines == 0 {
            new_from
        } else {
            new_from + 1
        };
        out.push(Hunk {
            header: format!("@@ -{old_disp},{old_lines} +{new_disp},{new_lines} @@"),
            old_start: old_from + 1,
            old_lines,
            new_start: new_from + 1,
            new_lines,
            lines,
        });
    }
    out
}

#[derive(Clone, Copy)]
enum Direction {
    /// Baseline takes the working tree's lines. Disk untouched.
    Accept,
    /// Working tree takes the baseline's lines. Baseline untouched.
    Reject,
}

/// Advance `baseline` over `selected`, returning the new baseline text.
/// Never touches the working tree — that is the whole point of accept.
pub fn accept(baseline: &str, current: &str, selected: &[Hunk]) -> Result<String, ReviewError> {
    splice(baseline, current, selected, Direction::Accept)
}

/// Undo `selected` in the working tree, returning the text to write.
/// Never moves the baseline.
pub fn reject(baseline: &str, current: &str, selected: &[Hunk]) -> Result<String, ReviewError> {
    splice(baseline, current, selected, Direction::Reject)
}

fn splice(
    baseline: &str,
    current: &str,
    selected: &[Hunk],
    dir: Direction,
) -> Result<String, ReviewError> {
    if selected.is_empty() {
        return Err(ReviewError::Empty);
    }
    // Recompute and require an exact match. A submitted hunk that is
    // not in the live diff means the file moved underneath the user.
    let pending = diff_lines(baseline, current);
    let mut picked: Vec<usize> = Vec::with_capacity(selected.len());
    for want in selected {
        let Some(i) = pending.iter().position(|h| h.same_change_as(want)) else {
            return Err(ReviewError::Stale);
        };
        if !picked.contains(&i) {
            picked.push(i);
        }
    }
    // Bottom-up, so a splice never shifts the coordinates of one that
    // has not been applied yet.
    picked.sort_unstable_by(|a, b| b.cmp(a));

    let old = split_lines(baseline);
    let new = split_lines(current);
    let mut out: Vec<&str> = match dir {
        Direction::Accept => old.clone(),
        Direction::Reject => new.clone(),
    };
    for i in picked {
        let h = &pending[i];
        let (dst, src) = match dir {
            Direction::Accept => (
                (h.old_start - 1)..(h.old_start - 1 + h.old_lines),
                &new[(h.new_start - 1)..(h.new_start - 1 + h.new_lines)],
            ),
            Direction::Reject => (
                (h.new_start - 1)..(h.new_start - 1 + h.new_lines),
                &old[(h.old_start - 1)..(h.old_start - 1 + h.old_lines)],
            ),
        };
        out.splice(dst, src.iter().copied());
    }
    Ok(out.concat())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_change() -> (String, String) {
        let baseline = "a\nb\nc\nd\ne\nf\ng\nh\ni\n".to_string();
        let current = "a\nb\nc\nd\nE\nf\ng\nh\ni\n".to_string();
        (baseline, current)
    }

    #[test]
    fn split_lines_round_trips_every_shape() {
        for s in [
            "",
            "a",
            "a\n",
            "a\nb",
            "a\r\nb\r\n",
            "\n\n\n",
            "no trailing newline",
        ] {
            assert_eq!(split_lines(s).concat(), s, "round trip failed for {s:?}");
        }
    }

    #[test]
    fn identical_texts_have_no_pending_hunks() {
        assert!(diff_lines("a\nb\n", "a\nb\n").is_empty());
        assert!(diff_lines("", "").is_empty());
    }

    #[test]
    fn accept_advances_the_baseline_to_the_working_tree() {
        let (baseline, current) = one_change();
        let hunks = diff_lines(&baseline, &current);
        assert_eq!(hunks.len(), 1);
        let next = accept(&baseline, &current, &hunks).unwrap();
        assert_eq!(next, current);
        // And the file is now fully reviewed.
        assert!(diff_lines(&next, &current).is_empty());
    }

    #[test]
    fn reject_restores_the_baseline_on_disk() {
        let (baseline, current) = one_change();
        let hunks = diff_lines(&baseline, &current);
        let disk = reject(&baseline, &current, &hunks).unwrap();
        assert_eq!(disk, baseline);
        assert!(diff_lines(&baseline, &disk).is_empty());
    }

    #[test]
    fn accepting_one_hunk_of_several_leaves_the_rest_pending() {
        let baseline = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n14\n15\n16\n";
        let current = "1\n2\nTHREE\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\nFOURTEEN\n15\n16\n";
        let hunks = diff_lines(baseline, current);
        assert_eq!(hunks.len(), 2, "context radius should keep these apart");

        let next = accept(baseline, current, &hunks[..1]).unwrap();
        assert!(next.contains("THREE"));
        assert!(!next.contains("FOURTEEN"));
        let still = diff_lines(&next, current);
        assert_eq!(still.len(), 1);
        assert!(still[0].lines.iter().any(|l| l.content == "FOURTEEN"));
    }

    #[test]
    fn rejecting_one_hunk_of_several_leaves_the_rest_on_disk() {
        let baseline = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n14\n15\n16\n";
        let current = "1\n2\nTHREE\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\nFOURTEEN\n15\n16\n";
        let hunks = diff_lines(baseline, current);
        let disk = reject(baseline, current, &hunks[1..]).unwrap();
        assert!(disk.contains("THREE"), "the unrejected hunk must survive");
        assert!(!disk.contains("FOURTEEN"));
    }

    #[test]
    fn multi_hunk_accept_applies_bottom_up_without_drift() {
        // Hunks of different sizes so a top-down application would
        // visibly shift the later ones.
        let mut lines: Vec<String> = (1..=40).map(|i| format!("{i}\n")).collect();
        let baseline: String = lines.concat();
        lines[2] = "three\nthree-and-a-half\n".to_string();
        lines[20] = String::new();
        lines[35] = "thirty-six!\n".to_string();
        let current: String = lines.concat();

        let hunks = diff_lines(&baseline, &current);
        assert!(hunks.len() >= 3);
        let next = accept(&baseline, &current, &hunks).unwrap();
        assert_eq!(next, current);
    }

    #[test]
    fn crlf_and_missing_trailing_newline_survive_a_splice() {
        let baseline = "a\r\nb\r\nc";
        let current = "a\r\nB\r\nc";
        let hunks = diff_lines(baseline, current);
        assert_eq!(accept(baseline, current, &hunks).unwrap(), current);
        assert_eq!(reject(baseline, current, &hunks).unwrap(), baseline);
        // The rendered content is terminator-free even though the
        // splice is not.
        assert!(hunks[0].lines.iter().all(|l| !l.content.contains('\r')));
    }

    #[test]
    fn a_new_file_is_one_pure_insertion() {
        let hunks = diff_lines("", "hello\nworld\n");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old_lines, 0);
        assert_eq!(hunks[0].additions(), 2);
        assert_eq!(hunks[0].header, "@@ -0,0 +1,2 @@");
        assert_eq!(
            accept("", "hello\nworld\n", &hunks).unwrap(),
            "hello\nworld\n"
        );
        assert_eq!(reject("", "hello\nworld\n", &hunks).unwrap(), "");
    }

    #[test]
    fn a_deleted_file_is_one_pure_deletion() {
        let hunks = diff_lines("gone\n", "");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].new_lines, 0);
        assert_eq!(hunks[0].deletions(), 1);
        assert_eq!(reject("gone\n", "", &hunks).unwrap(), "gone\n");
    }

    #[test]
    fn a_hunk_that_no_longer_matches_is_stale_not_applied() {
        let (baseline, current) = one_change();
        let hunks = diff_lines(&baseline, &current);
        // The agent wrote again between render and click.
        let moved = "a\nb\nc\nd\nSOMETHING ELSE\nf\ng\nh\ni\n";
        assert_eq!(
            accept(&baseline, moved, &hunks),
            Err(ReviewError::Stale),
            "accept must refuse a hunk the user cannot have seen"
        );
        assert_eq!(reject(&baseline, moved, &hunks), Err(ReviewError::Stale));
    }

    #[test]
    fn a_hunk_submitted_twice_is_applied_once() {
        let (baseline, current) = one_change();
        let hunks = diff_lines(&baseline, &current);
        let doubled = vec![hunks[0].clone(), hunks[0].clone()];
        assert_eq!(accept(&baseline, &current, &doubled).unwrap(), current);
    }

    #[test]
    fn an_empty_selection_is_rejected_rather_than_silently_doing_nothing() {
        let (baseline, current) = one_change();
        assert_eq!(accept(&baseline, &current, &[]), Err(ReviewError::Empty));
    }

    #[test]
    fn a_cosmetic_header_difference_does_not_break_identity() {
        let (baseline, current) = one_change();
        let mut hunks = diff_lines(&baseline, &current);
        hunks[0].header = "@@ whatever @@".to_string();
        assert_eq!(accept(&baseline, &current, &hunks).unwrap(), current);
    }

    #[test]
    fn additions_and_deletions_count_only_changed_lines() {
        let hunks = diff_lines("a\nb\nc\n", "a\nB\nB2\nc\n");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].additions(), 2);
        assert_eq!(hunks[0].deletions(), 1);
    }
}
