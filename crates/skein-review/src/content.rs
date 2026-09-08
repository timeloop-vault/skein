//! What is actually on disk at a path, classified before anything tries
//! to diff it.
//!
//! The classification order is load-bearing and matches the reference
//! design in `docs/grok-build-recon-2026-07-16.md` §3: symlink first
//! (an lstat, so we never follow one out of the worktree), then the
//! size cap *before* allocating, then a NUL sniff, then UTF-8. An
//! unreadable file is its own state rather than collapsing into
//! `Missing` — treating a permission error as "the agent deleted it"
//! would let a reject write a deletion the user never asked for.

use std::fs;
use std::io;
use std::path::Path;

/// Files above this read as [`FileState::TooLarge`] and are reported
/// but never diffed. The review surface is for code review; a 1 MiB
/// source file is already past the point where a line diff helps, and
/// the cap keeps a runaway generated file from stalling the pane.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// The reviewable state of one path. `Text` is the only variant a diff
/// can be computed against; everything else is reported to the user as
/// itself rather than being silently skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileState {
    /// No such path (or the baseline predates the file's creation).
    Missing,
    /// A symlink. Not followed — following one can leave the worktree.
    Symlink,
    /// Contains NUL or is not UTF-8. Carries a digest rather than a
    /// length so that accepting a binary file actually clears it: two
    /// different images of the same size must not compare equal, or the
    /// tab would be stuck pending forever.
    Binary { digest: u64 },
    /// Larger than [`MAX_FILE_BYTES`], so never read. Compared by
    /// length alone: an edit that preserves the byte count of an
    /// oversize file will not re-open the tab. The alternative is
    /// hashing megabytes on every watcher tick.
    TooLarge { len: u64 },
    /// Exists and could not be read (permissions, a locked file on
    /// Windows, an I/O error). Deliberately not `Missing`.
    Unreadable,
    /// Reviewable content.
    Text(String),
}

impl FileState {
    /// The text, when this state has any.
    pub fn text(&self) -> Option<&str> {
        match self {
            FileState::Text(s) => Some(s),
            _ => None,
        }
    }

    /// Whether a diff can be computed against this state. `Missing`
    /// counts: an added or deleted file diffs against the empty text.
    pub fn is_diffable(&self) -> bool {
        matches!(self, FileState::Text(_) | FileState::Missing)
    }

    /// The text to diff against, treating a missing file as empty.
    pub fn as_diff_text(&self) -> &str {
        match self {
            FileState::Text(s) => s,
            _ => "",
        }
    }
}

/// Classify `path` without following symlinks.
pub fn read_state(path: &Path) -> FileState {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return FileState::Missing;
    };
    let ft = meta.file_type();
    if ft.is_symlink() {
        return FileState::Symlink;
    }
    if ft.is_dir() {
        // A directory at a path we expected to be a file is not
        // reviewable, and it is certainly not "deleted".
        return FileState::Unreadable;
    }
    let len = meta.len();
    if len > MAX_FILE_BYTES {
        return FileState::TooLarge { len };
    }
    let Ok(bytes) = fs::read(path) else {
        return FileState::Unreadable;
    };
    classify_bytes(&bytes)
}

/// Classify bytes already in hand — a git blob, say — by the same rules
/// [`read_state`] applies to a file. This is what makes a baseline
/// taken from HEAD directly comparable with the working tree.
pub fn classify_bytes(bytes: &[u8]) -> FileState {
    let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if len > MAX_FILE_BYTES {
        return FileState::TooLarge { len };
    }
    if bytes.contains(&0) {
        return FileState::Binary {
            digest: fnv1a(bytes),
        };
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => FileState::Text(s.to_string()),
        Err(_) => FileState::Binary {
            digest: fnv1a(bytes),
        },
    }
}

/// How a state is stored in `review_baselines`: a `kind` discriminator
/// and the one string that reconstitutes it. Text keeps its content;
/// binary keeps a digest; too-large keeps a length; the rest need
/// nothing.
pub fn encode(state: &FileState) -> (&'static str, Option<String>) {
    match state {
        FileState::Missing => ("missing", None),
        FileState::Symlink => ("symlink", None),
        FileState::Unreadable => ("unreadable", None),
        FileState::Binary { digest } => ("binary", Some(format!("{digest:x}"))),
        FileState::TooLarge { len } => ("toolarge", Some(len.to_string())),
        FileState::Text(s) => ("text", Some(s.clone())),
    }
}

/// Inverse of [`encode`]. An unrecognised `kind`, or one whose payload
/// will not parse, decodes to [`FileState::Unreadable`] — never to
/// `Missing`, which a reject would act on by deleting a file.
pub fn decode(kind: &str, content: Option<&str>) -> FileState {
    match kind {
        "missing" => FileState::Missing,
        "symlink" => FileState::Symlink,
        "text" => FileState::Text(content.unwrap_or_default().to_string()),
        "binary" => content
            .and_then(|c| u64::from_str_radix(c, 16).ok())
            .map_or(FileState::Unreadable, |digest| FileState::Binary { digest }),
        "toolarge" => content
            .and_then(|c| c.parse::<u64>().ok())
            .map_or(FileState::Unreadable, |len| FileState::TooLarge { len }),
        _ => FileState::Unreadable,
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(PRIME);
    }
    h
}

/// A stable 64-bit digest of a state, used as the staleness guard on
/// whole-file accept and reject: the caller sends back the hash of what
/// it rendered, and the operation refuses to run if disk has moved on.
///
/// FNV-1a — not cryptographic and not meant to be. It guards against a
/// file changing between a render and a click, not against an adversary
/// constructing a collision.
pub fn content_hash(state: &FileState) -> u64 {
    // Tag first so Text("") and Missing never collide.
    let (kind, payload) = encode(state);
    let mut buf = kind.as_bytes().to_vec();
    buf.push(0);
    if let Some(p) = payload {
        buf.extend_from_slice(p.as_bytes());
    }
    fnv1a(&buf)
}

/// Write `text` to `path`, creating parent directories. Used by reject
/// to restore baseline content, including for a file the agent deleted.
pub fn write_text(path: &Path, text: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(path, text.as_bytes())
}

/// Delete `path`. Used by reject when the baseline is [`FileState::Missing`]
/// — the agent created the file and the user does not want it. A path
/// that is already gone is success, not an error.
pub fn remove_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn classifies_text_binary_missing_and_oversize() {
        let dir = TempDir::new().unwrap();
        let text = dir.path().join("a.rs");
        fs::write(&text, "fn main() {}\n").unwrap();
        assert_eq!(
            read_state(&text),
            FileState::Text("fn main() {}\n".to_string())
        );

        let bin = dir.path().join("b.png");
        fs::write(&bin, [0x89, 0x50, 0x00, 0x01]).unwrap();
        assert!(matches!(read_state(&bin), FileState::Binary { .. }));

        assert_eq!(read_state(&dir.path().join("nope")), FileState::Missing);

        let big = dir.path().join("big.txt");
        fs::write(
            &big,
            vec![b'x'; usize::try_from(MAX_FILE_BYTES).unwrap() + 1],
        )
        .unwrap();
        assert!(matches!(read_state(&big), FileState::TooLarge { .. }));
    }

    #[test]
    fn a_directory_is_unreadable_not_missing() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        assert_eq!(read_state(&sub), FileState::Unreadable);
    }

    #[test]
    fn hash_separates_empty_text_from_missing() {
        assert_ne!(
            content_hash(&FileState::Text(String::new())),
            content_hash(&FileState::Missing)
        );
        assert_eq!(
            content_hash(&FileState::Text("a\n".into())),
            content_hash(&FileState::Text("a\n".into()))
        );
        assert_ne!(
            content_hash(&FileState::Text("a\n".into())),
            content_hash(&FileState::Text("b\n".into()))
        );
    }

    #[test]
    fn write_creates_parents_and_remove_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("x/y/z.txt");
        write_text(&nested, "hi\n").unwrap();
        assert_eq!(read_state(&nested), FileState::Text("hi\n".into()));
        remove_file(&nested).unwrap();
        remove_file(&nested).unwrap();
        assert_eq!(read_state(&nested), FileState::Missing);
    }
}

#[cfg(test)]
mod encoding_tests {
    use super::*;

    #[test]
    fn every_state_round_trips_through_the_db_encoding() {
        for state in [
            FileState::Missing,
            FileState::Symlink,
            FileState::Unreadable,
            FileState::Binary {
                digest: 0xdead_beef,
            },
            FileState::TooLarge { len: 5_000_000 },
            FileState::Text("a\r\nb\n".into()),
            FileState::Text(String::new()),
        ] {
            let (kind, payload) = encode(&state);
            assert_eq!(
                decode(kind, payload.as_deref()),
                state,
                "round trip {state:?}"
            );
        }
    }

    #[test]
    fn two_binaries_of_the_same_length_do_not_compare_equal() {
        let dir = tempfile::TempDir::new().unwrap();
        let a = dir.path().join("a.bin");
        let b = dir.path().join("b.bin");
        fs::write(&a, [0x00, 0x01, 0x02, 0x03]).unwrap();
        fs::write(&b, [0x00, 0x01, 0x02, 0x04]).unwrap();
        assert_ne!(
            read_state(&a),
            read_state(&b),
            "an accepted binary would never clear if these matched"
        );
    }

    #[test]
    fn a_corrupt_row_decodes_to_unreadable_never_to_missing() {
        // Missing is the one state a reject acts on by deleting the
        // file, so a row we cannot parse must never land there.
        assert_eq!(decode("binary", Some("not-hex")), FileState::Unreadable);
        assert_eq!(decode("toolarge", None), FileState::Unreadable);
        assert_eq!(decode("from-a-future-version", None), FileState::Unreadable);
    }
}
