//! Comment anchoring — where a review thread lives after the code
//! under it has moved (#212, epic #52 D6).
//!
//! The rule the epic states, and the one this module exists to keep:
//!
//! > **Never silently move a comment. Never silently drop one.**
//!
//! In a Skein room that is not a nicety. The agent commits repeatedly to
//! address feedback, so every anchor moves several times per review, and
//! a squash-merge orphans anything anchored to a sha. A comment that
//! quietly slides onto unrelated code is worse than one marked
//! outdated: the user reads it as being about the code it now sits on.
//!
//! # What an anchor is
//!
//! The **text**, not a line number. [`Anchor`] carries the lines the
//! comment was written against, verbatim, plus the line it started on.
//! The line number is only a tie-breaker — when the same text appears
//! several times in a file, the occurrence nearest where the user left
//! the comment is the one they meant.
//!
//! # How re-matching works
//!
//! Three tiers, most confident first, exactly as D6 orders them:
//!
//! 1. **Exact, unmoved** — the anchor text is still at its original
//!    line. The overwhelmingly common case, and the cheap one.
//! 2. **Exact, moved** — the anchor text appears verbatim elsewhere;
//!    the occurrence closest to the original line wins. This is what
//!    survives an agent inserting a function above yours.
//! 3. **Maximum overlap** — no verbatim match, so the best partial one,
//!    scored on *trimmed* lines so a re-indent does not orphan a thread.
//!    Below [`MIN_CONFIDENCE`] nothing is placed.
//!
//! Anything that reaches the bottom is [`Placement::Outdated`], and the
//! caller renders it against [`Anchor::lines`] — the code as the comment
//! saw it — rather than against whatever occupies those line numbers now.
//!
//! # Why blank lines are excluded from scoring
//!
//! A blank line matches everywhere. Counting them would let a thread on
//! a sparse block score 0.6 against any other sparse block in the file,
//! which is precisely the silent move the rule forbids. They still
//! occupy their place in the anchor; they just never vote.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::content::fnv1a;

/// The fraction of an anchor's significant lines that must match for a
/// partial placement to count. Below this the thread is outdated.
///
/// Half is deliberately permissive: the failure mode of being too
/// strict is a thread marked outdated while its code is plainly still
/// there, which trains the user to ignore the marker. The failure mode
/// of being too loose is a silently moved comment, which the tiers
/// above already guard — anything reaching the overlap pass has no
/// verbatim match anywhere, so a half-match really is the best evidence
/// available.
pub const MIN_CONFIDENCE: f32 = 0.5;

/// Which side of a diff a line anchor refers to.
///
/// A comment on a deleted line belongs to the old text and can never be
/// found in the new; keeping the side means such a thread is anchored
/// against the right content instead of being permanently outdated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    /// The pre-change text — the diff's `-` and context lines.
    Old,
    /// The post-change text — the diff's `+` and context lines.
    New,
}

/// Where a thread was attached when it was written.
///
/// `lines` is the anchor. `start` and `hash` are accelerators: the line
/// number disambiguates repeats, the hash lets an unchanged file skip
/// the text comparison entirely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Anchor {
    pub side: Side,
    /// 1-based, inclusive — the line `lines[0]` was on.
    pub start: usize,
    /// The anchored lines, terminators stripped.
    pub lines: Vec<String>,
    /// Digest of `lines`. Persisted so a stored anchor can be compared
    /// without rehydrating its text.
    pub hash: String,
}

impl Anchor {
    /// Build an anchor from the lines a comment was written against.
    pub fn new(side: Side, start: usize, lines: Vec<String>) -> Self {
        let hash = hash_lines(&lines);
        Self {
            side,
            start,
            lines,
            hash,
        }
    }

    /// 1-based inclusive end line, in the coordinates the anchor was
    /// written in.
    pub fn end(&self) -> usize {
        self.start + self.lines.len().saturating_sub(1)
    }

    /// Lines that carry positional information — non-blank ones. An
    /// anchor with none of these can only ever match exactly.
    fn significant(&self) -> usize {
        self.lines.iter().filter(|l| !l.trim().is_empty()).count()
    }
}

/// Where a thread ended up on this refresh.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Placement {
    /// The anchor text is still exactly where it was.
    Unmoved { start: usize },
    /// The anchor text was found verbatim, at a different line.
    Moved { start: usize },
    /// No verbatim match; this is the best partial one.
    Shifted { start: usize, confidence: f32 },
    /// Nothing matched well enough. Render against [`Anchor::lines`].
    Outdated,
}

impl Placement {
    /// The 1-based line this thread now starts on, when it is placed at
    /// all.
    pub fn start(&self) -> Option<usize> {
        match *self {
            Placement::Unmoved { start }
            | Placement::Moved { start }
            | Placement::Shifted { start, .. } => Some(start),
            Placement::Outdated => None,
        }
    }

    /// Whether the UI must mark this thread as no longer trustworthy.
    ///
    /// `Shifted` counts. A partial match is a guess, and D6's rule is
    /// that a guess is shown as a guess — the thread renders where it
    /// best fits *and* says it may have drifted.
    pub fn is_outdated(&self) -> bool {
        matches!(self, Placement::Outdated | Placement::Shifted { .. })
    }
}

/// Digest of a run of lines — the value stored in `Anchor::hash`.
///
/// Hex FNV-1a over the lines joined with `\n`, matching how the rest of
/// the crate hashes. Not cryptographic: it detects change, not attack.
pub fn hash_lines(lines: &[String]) -> String {
    let joined = lines.join("\n");
    format!("{:x}", fnv1a(joined.as_bytes()))
}

/// A file indexed for re-anchoring.
///
/// Built once per file per refresh and reused across every thread on
/// that file — a busy file has dozens of threads, and re-splitting and
/// re-hashing it for each one is the difference between a refresh that
/// is free and one the user feels.
pub struct Reanchorer<'a> {
    lines: Vec<&'a str>,
    /// Exact line hash → every 1-based line it occurs on.
    exact: HashMap<u64, Vec<usize>>,
    /// Trimmed line hash → every 1-based line it occurs on. Blank lines
    /// are absent by construction, which is what keeps them from voting.
    fuzzy: HashMap<u64, Vec<usize>>,
}

impl<'a> Reanchorer<'a> {
    pub fn new(text: &'a str) -> Self {
        // `lines()` rather than the crate's `split_lines`: an anchor is
        // display text with terminators already stripped, so the two
        // sides must be compared the same way.
        let lines: Vec<&str> = text.lines().collect();
        let mut exact: HashMap<u64, Vec<usize>> = HashMap::new();
        let mut fuzzy: HashMap<u64, Vec<usize>> = HashMap::new();
        for (i, line) in lines.iter().enumerate() {
            let lineno = i + 1;
            exact
                .entry(fnv1a(line.as_bytes()))
                .or_default()
                .push(lineno);
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                fuzzy
                    .entry(fnv1a(trimmed.as_bytes()))
                    .or_default()
                    .push(lineno);
            }
        }
        Self {
            lines,
            exact,
            fuzzy,
        }
    }

    /// Total lines in the indexed text.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Re-match `anchor` against this file.
    pub fn place(&self, anchor: &Anchor) -> Placement {
        if anchor.lines.is_empty() || self.lines.is_empty() {
            // A file-level or review-level thread has no line anchor and
            // must never be reported as outdated on that basis; the
            // caller does not route those here. An empty file genuinely
            // has nowhere to put a line thread.
            return Placement::Outdated;
        }
        if let Some(start) = self.find_exact(anchor) {
            return if start == anchor.start {
                Placement::Unmoved { start }
            } else {
                Placement::Moved { start }
            };
        }
        self.find_overlap(anchor)
    }

    /// The verbatim occurrence closest to where the comment was left.
    fn find_exact(&self, anchor: &Anchor) -> Option<usize> {
        // Seed on the anchor's rarest line so a long anchor in a big
        // file does not walk every candidate start. A line the file
        // repeats a thousand times is a poor seed; the rarest one is
        // usually unique.
        let (seed_idx, seed_positions) = anchor
            .lines
            .iter()
            .enumerate()
            .filter_map(|(i, line)| {
                self.exact
                    .get(&fnv1a(line.as_bytes()))
                    .map(|positions| (i, positions))
            })
            .min_by_key(|(_, positions)| positions.len())?;

        let mut best: Option<usize> = None;
        for &seed_line in seed_positions {
            // Where the anchor would start if this occurrence is its
            // `seed_idx`-th line.
            let Some(start) = seed_line.checked_sub(seed_idx) else {
                continue;
            };
            if start == 0 || !self.matches_at(anchor, start) {
                continue;
            }
            let closer = best.is_none_or(|b| {
                start.abs_diff(anchor.start) < b.abs_diff(anchor.start)
                    // A genuine tie takes the *later* occurrence. Code
                    // grows above a line far more often than it shrinks,
                    // so when an anchor sits equidistant between two
                    // copies of its text, the one that drifted downward
                    // is the one the user commented on. It also makes
                    // the result independent of hash iteration order.
                    || (start.abs_diff(anchor.start) == b.abs_diff(anchor.start) && start > b)
            });
            if closer {
                best = Some(start);
            }
        }
        best
    }

    /// Does the anchor sit verbatim at 1-based line `start`?
    fn matches_at(&self, anchor: &Anchor, start: usize) -> bool {
        let end = start + anchor.lines.len() - 1;
        if end > self.lines.len() {
            return false;
        }
        anchor
            .lines
            .iter()
            .enumerate()
            .all(|(i, want)| self.lines.get(start - 1 + i).is_some_and(|got| got == want))
    }

    /// Best partial match, scored on trimmed lines.
    ///
    /// Every significant anchor line votes for the offset that would
    /// put it where it was found; the winning offset is scored properly
    /// by counting how many anchor lines actually land inside the window
    /// it implies. Voting narrows the candidates; the window count is
    /// what decides, so an inserted line inside the range costs one
    /// vote rather than invalidating the whole match.
    fn find_overlap(&self, anchor: &Anchor) -> Placement {
        let significant = anchor.significant();
        if significant == 0 {
            // Nothing but blank lines. There is no evidence to match on,
            // and guessing here is exactly the silent move D6 forbids.
            return Placement::Outdated;
        }

        let mut votes: HashMap<usize, usize> = HashMap::new();
        for (i, line) in anchor.lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Some(positions) = self.fuzzy.get(&fnv1a(trimmed.as_bytes())) else {
                continue;
            };
            for &lineno in positions {
                if let Some(start) = lineno.checked_sub(i) {
                    if start > 0 {
                        *votes.entry(start).or_default() += 1;
                    }
                }
            }
        }
        if votes.is_empty() {
            return Placement::Outdated;
        }

        let mut best: Option<(usize, usize)> = None; // (start, overlap)
        for &start in votes.keys() {
            let overlap = self.window_overlap(anchor, start);
            let better = best.is_none_or(|(b_start, b_overlap)| {
                overlap > b_overlap
                    || (overlap == b_overlap
                        && start.abs_diff(anchor.start) < b_start.abs_diff(anchor.start))
                    || (overlap == b_overlap
                        && start.abs_diff(anchor.start) == b_start.abs_diff(anchor.start)
                        && start < b_start)
            });
            if better {
                best = Some((start, overlap));
            }
        }

        let Some((start, overlap)) = best else {
            return Placement::Outdated;
        };
        #[allow(clippy::cast_precision_loss)]
        let confidence = overlap as f32 / significant as f32;
        if overlap == 0 || confidence < MIN_CONFIDENCE {
            return Placement::Outdated;
        }
        Placement::Shifted { start, confidence }
    }

    /// How many of the anchor's significant lines appear anywhere inside
    /// the window of the same length starting at 1-based `start`.
    ///
    /// Multiset, not positional: a line that merely shifted within the
    /// range still counts, which is the "maximum overlap" D6 asks for
    /// rather than a rigid equality test.
    fn window_overlap(&self, anchor: &Anchor, start: usize) -> usize {
        let mut available: HashMap<u64, usize> = HashMap::new();
        for offset in 0..anchor.lines.len() {
            let Some(line) = self.lines.get(start - 1 + offset) else {
                break;
            };
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                *available.entry(fnv1a(trimmed.as_bytes())).or_default() += 1;
            }
        }
        let mut hits = 0;
        for line in &anchor.lines {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(count) = available.get_mut(&fnv1a(trimmed.as_bytes())) {
                if *count > 0 {
                    *count -= 1;
                    hits += 1;
                }
            }
        }
        hits
    }
}

/// Re-match one anchor against one file. Convenience over
/// [`Reanchorer`]; prefer the indexed form when placing several threads
/// on the same file.
pub fn reanchor(anchor: &Anchor, current: &str) -> Placement {
    Reanchorer::new(current).place(anchor)
}

/// The lines of `text` covered by the 1-based inclusive range
/// `start..=end`, for capturing an anchor when a comment is written.
///
/// Clamped rather than fallible: the UI can only offer line numbers it
/// rendered, and a range that has drifted by the time the comment is
/// submitted should still capture the text that is there.
pub fn capture_lines(text: &str, start: usize, end: usize) -> Vec<String> {
    if start == 0 {
        return Vec::new();
    }
    text.lines()
        .skip(start - 1)
        .take(end.saturating_sub(start) + 1)
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor_at(start: usize, lines: &[&str]) -> Anchor {
        Anchor::new(
            Side::New,
            start,
            lines.iter().map(|s| (*s).to_owned()).collect(),
        )
    }

    /// A small file with enough shape to move things around inside.
    const FILE: &str = "\
fn main() {
    let total = 0;
    for item in items {
        total += item.value;
    }
    println!(\"{total}\");
}
";

    // ── tier 1: unchanged ─────────────────────────────────────────

    #[test]
    fn an_untouched_line_stays_unmoved() {
        let a = anchor_at(4, &["        total += item.value;"]);
        assert_eq!(reanchor(&a, FILE), Placement::Unmoved { start: 4 });
    }

    #[test]
    fn a_multi_line_range_stays_unmoved() {
        let a = anchor_at(
            3,
            &[
                "    for item in items {",
                "        total += item.value;",
                "    }",
            ],
        );
        assert_eq!(reanchor(&a, FILE), Placement::Unmoved { start: 3 });
        assert!(!reanchor(&a, FILE).is_outdated());
    }

    // ── tier 2: moved verbatim ────────────────────────────────────

    #[test]
    fn an_insertion_above_moves_the_anchor_down() {
        let a = anchor_at(4, &["        total += item.value;"]);
        let after = FILE.replace("fn main() {\n", "fn main() {\n    // added\n");
        assert_eq!(reanchor(&a, &after), Placement::Moved { start: 5 });
    }

    #[test]
    fn a_deletion_above_moves_the_anchor_up() {
        let a = anchor_at(4, &["        total += item.value;"]);
        let after = FILE.replace("    let total = 0;\n", "");
        assert_eq!(reanchor(&a, &after), Placement::Moved { start: 3 });
    }

    #[test]
    fn a_moved_anchor_is_not_outdated() {
        let a = anchor_at(4, &["        total += item.value;"]);
        let after = FILE.replace("fn main() {\n", "fn main() {\n    // added\n");
        assert!(
            !reanchor(&a, &after).is_outdated(),
            "a verbatim match elsewhere is a certainty, not a guess"
        );
    }

    #[test]
    fn repeated_text_resolves_to_the_occurrence_nearest_the_original() {
        // The same line three times: the anchor must land on the one the
        // user actually commented on, not the first in the file.
        let text = "x();\ny();\nx();\nz();\nx();\n";
        let a = anchor_at(3, &["x();"]);
        assert_eq!(reanchor(&a, text), Placement::Unmoved { start: 3 });

        // Shift everything down by one. The commented occurrence is now
        // line 4, and the one above it (line 2) is equally distant from
        // the original 3 — the later-wins tie-break is what keeps the
        // thread on the copy the user actually meant.
        let shifted = format!("// head\n{text}");
        assert_eq!(reanchor(&a, &shifted), Placement::Moved { start: 4 });
    }

    #[test]
    fn a_tie_between_equidistant_occurrences_takes_the_later_one() {
        // Occurrences at 1 and 5, anchored at 3 — exactly 2 away each.
        // Code grows above a line more often than it shrinks, so the
        // later copy is the better guess; and either way the answer must
        // not depend on hash iteration order.
        let text = "dup\na\nb\nc\ndup\n";
        let a = anchor_at(3, &["dup"]);
        assert_eq!(reanchor(&a, text), Placement::Moved { start: 5 });
    }

    // ── tier 3: overlap ───────────────────────────────────────────

    #[test]
    fn a_reindented_block_is_found_but_flagged() {
        let a = anchor_at(
            3,
            &[
                "    for item in items {",
                "        total += item.value;",
                "    }",
            ],
        );
        // The whole body gets an extra indent level.
        let after = FILE
            .replace("    for item", "        for item")
            .replace("        total +=", "            total +=")
            .replace("    }\n    println", "        }\n    println");
        match reanchor(&a, &after) {
            Placement::Shifted { start, confidence } => {
                assert_eq!(start, 3);
                assert!(confidence >= MIN_CONFIDENCE, "confidence {confidence}");
            }
            other => panic!("expected a shifted placement, got {other:?}"),
        }
    }

    #[test]
    fn a_shifted_placement_is_reported_as_outdated() {
        // The rule: a guess renders as a guess. It is placed *and*
        // flagged, never placed silently.
        let a = anchor_at(
            3,
            &["    for item in items {", "        total += item.value;"],
        );
        let after = FILE.replace(
            "        total += item.value;",
            "        total += item.cost;",
        );
        let placement = reanchor(&a, &after);
        assert!(placement.start().is_some(), "still placed");
        assert!(placement.is_outdated(), "and still flagged");
    }

    #[test]
    fn a_partly_rewritten_block_keeps_its_place_above_the_threshold() {
        let a = anchor_at(1, &["one", "two", "three", "four"]);
        // Half the lines survive — exactly at the threshold.
        let after = "one\nCHANGED\nthree\nALSO CHANGED\n";
        match reanchor(&a, after) {
            Placement::Shifted { start, confidence } => {
                assert_eq!(start, 1);
                assert!((confidence - 0.5).abs() < f32::EPSILON);
            }
            other => panic!("expected shifted, got {other:?}"),
        }
    }

    #[test]
    fn a_mostly_rewritten_block_goes_outdated() {
        let a = anchor_at(1, &["one", "two", "three", "four"]);
        let after = "one\nA\nB\nC\n";
        assert_eq!(reanchor(&a, after), Placement::Outdated);
    }

    #[test]
    fn a_line_inside_the_range_costs_one_match_not_the_whole_anchor() {
        let a = anchor_at(1, &["alpha", "beta", "gamma", "delta"]);
        let after = "alpha\nbeta\nINSERTED\ngamma\ndelta\n";
        let placement = reanchor(&a, after);
        assert_eq!(
            placement.start(),
            Some(1),
            "the block is plainly still here"
        );
        assert!(placement.is_outdated(), "but it is not verbatim");
    }

    // ── the never-drop / never-silently-move rule ─────────────────

    #[test]
    fn a_deleted_block_is_outdated_not_relocated() {
        // The code the comment was about is simply gone. The one thing
        // that must not happen is the thread landing on the code that
        // now occupies those line numbers.
        let a = anchor_at(
            3,
            &[
                "    for item in items {",
                "        total += item.value;",
                "    }",
            ],
        );
        let after = "fn main() {\n    let total = 0;\n    println!(\"{total}\");\n}\n";
        assert_eq!(reanchor(&a, after), Placement::Outdated);
    }

    #[test]
    fn an_emptied_file_orphans_every_thread_rather_than_guessing() {
        let a = anchor_at(4, &["        total += item.value;"]);
        assert_eq!(reanchor(&a, ""), Placement::Outdated);
    }

    #[test]
    fn a_blank_only_anchor_never_matches_by_overlap() {
        // Blank lines match everywhere; scoring on them would place this
        // thread on any sparse block in the file.
        let a = anchor_at(2, &["", "   "]);
        let after = "code\n\n\n\nmore code\n\n\n";
        assert_eq!(
            reanchor(&a, after),
            Placement::Outdated,
            "no evidence means outdated, not a guess"
        );
    }

    #[test]
    fn a_blank_only_anchor_still_matches_exactly_when_it_is_still_there() {
        // Exact matching does not need evidence to be *distinctive*,
        // only to be present — the tier-1 path is safe for blank runs.
        let text = "a\n\n\nb\n";
        let a = anchor_at(2, &["", ""]);
        assert_eq!(reanchor(&a, text), Placement::Unmoved { start: 2 });
    }

    #[test]
    fn an_anchor_past_the_end_of_a_shortened_file_does_not_panic() {
        let a = anchor_at(200, &["        total += item.value;"]);
        // Present, but nowhere near line 200.
        assert_eq!(reanchor(&a, FILE), Placement::Moved { start: 4 });

        let gone = anchor_at(200, &["nothing like this"]);
        assert_eq!(reanchor(&gone, FILE), Placement::Outdated);
    }

    // ── plumbing ──────────────────────────────────────────────────

    #[test]
    fn hash_and_end_describe_the_anchor() {
        let a = anchor_at(10, &["one", "two", "three"]);
        assert_eq!(a.end(), 12);
        assert_eq!(a.hash, hash_lines(&a.lines));
        // Different content, different digest.
        let b = anchor_at(10, &["one", "two", "four"]);
        assert_ne!(a.hash, b.hash);
        // Same content at a different line is the same digest — the
        // line number is a tie-breaker, not part of identity.
        let c = anchor_at(99, &["one", "two", "three"]);
        assert_eq!(a.hash, c.hash);
    }

    #[test]
    fn a_single_line_anchor_has_end_equal_to_start() {
        let a = anchor_at(7, &["only"]);
        assert_eq!(a.end(), 7);
    }

    #[test]
    fn capture_lines_takes_the_rendered_range() {
        assert_eq!(capture_lines(FILE, 3, 5).len(), 3);
        assert_eq!(
            capture_lines(FILE, 4, 4),
            vec!["        total += item.value;"]
        );
        // Past the end clamps rather than failing.
        assert_eq!(capture_lines(FILE, 6, 99).len(), 2);
        assert!(capture_lines(FILE, 0, 3).is_empty());
        assert!(capture_lines("", 1, 3).is_empty());
    }

    #[test]
    fn the_indexed_form_agrees_with_the_convenience_one() {
        let idx = Reanchorer::new(FILE);
        assert_eq!(idx.len(), 7);
        assert!(!idx.is_empty());
        let a = anchor_at(4, &["        total += item.value;"]);
        assert_eq!(idx.place(&a), reanchor(&a, FILE));
    }

    #[test]
    fn crlf_text_anchors_the_same_as_lf() {
        // `lines()` strips \r\n, and anchors are stored with terminators
        // already gone — the two sides have to agree or every anchor in
        // a CRLF file would be outdated.
        let crlf = FILE.replace('\n', "\r\n");
        let a = anchor_at(4, &["        total += item.value;"]);
        assert_eq!(reanchor(&a, &crlf), Placement::Unmoved { start: 4 });
    }

    #[test]
    fn side_round_trips_through_serde() {
        let a = anchor_at(1, &["x"]);
        let json = serde_json::to_string(&a).unwrap();
        assert!(json.contains("\"side\":\"new\""), "{json}");
        let back: Anchor = serde_json::from_str(&json).unwrap();
        assert_eq!(back, a);
    }
}
