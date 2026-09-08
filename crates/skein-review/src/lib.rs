//! Baseline review model — issue #211, epic #52 decisions D3 and D4.
//!
//! The Diff card used to derive its tabs from *every* patch row a room
//! had ever seen, so nothing ever cleared: not on commit, not on a new
//! harness, not ever. The answer to "how long does a diff live" is
//! **until it is reviewed**, and that needs an episode boundary. This
//! crate is that boundary.
//!
//! # The model
//!
//! Every file a harness touches gets a **baseline**: the content the
//! user has already reviewed. The pending diff is `baseline → disk`.
//! Two asymmetric verbs act on it:
//!
//! * **accept** advances the baseline toward the working tree and
//!   **never writes to disk** — the change stops being pending, the
//!   bytes on disk are untouched.
//! * **reject** writes the working tree back toward the baseline and
//!   **never moves the baseline** — the change is undone, so it stops
//!   being pending for the same reason.
//!
//! Both are available per hunk and per file; the caller composes
//! "accept all" from the per-file form.
//!
//! # Why splicing, not patching
//!
//! Accept and reject both take a line range from *one* of the two texts
//! and splice it into the other, using line slices that carry their own
//! terminators. Nothing is reconstructed from the rendered hunk, so a
//! CRLF file stays CRLF, a file with no trailing newline keeps not
//! having one, and mixed endings survive. The hunk the caller sends
//! back is used only to *identify* what to splice.
//!
//! # Hunk identity
//!
//! There are no persistent hunk ids. A caller echoes back the hunk it
//! rendered, and the operation recomputes the diff and requires an
//! exact structural match before touching anything; no match means the
//! file moved under the user and the call fails with
//! [`ReviewError::Stale`] rather than applying something they did not
//! see.
//!
//! # Comment anchoring
//!
//! Review threads have the opposite requirement — they must survive a
//! recompute rather than refuse one — so they do not use hunk identity
//! at all. [`anchor`] anchors a thread on the *text* it was written
//! against and re-matches it on every refresh, placing it exactly,
//! placing it with a visible warning, or marking it outdated. Never
//! moving it silently and never dropping it is the whole contract
//! (#212, epic #52 D6).

// As in skein-git and skein-harness: the error enum is the
// documentation. Every fallible entry point here has its failure modes
// named in its own type.
#![allow(clippy::missing_errors_doc)]

mod anchor;
mod content;
mod hunks;

pub use anchor::{
    Anchor, MIN_CONFIDENCE, Placement, Reanchorer, Side, capture_lines, hash_lines, reanchor,
};
pub use content::{
    FileState, MAX_FILE_BYTES, classify_bytes, content_hash, decode, encode, read_state,
    remove_file, write_text,
};
pub use hunks::{
    CONTEXT_RADIUS, Hunk, HunkLine, LineKind, ReviewError, accept, diff_lines, reject, split_lines,
};
