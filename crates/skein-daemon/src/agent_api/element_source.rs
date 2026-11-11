//! A guess at which source lines a design-pane element maps to (#435).
//!
//! The recon (docs/design-surface-recon.md) says the lines come from the
//! anchor's `source` when present, else from a Rust-side search of the
//! worktree for the `odId` or the text snippet. It is a guess, and the
//! caller labels it as one: `via` says which evidence found it.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::design::{MAX_SERVED, resolve_under};
use crate::review_surface::element::ElementAnchor;

const EXTENSIONS: &[&str] = &[
    "html", "htm", "jsx", "tsx", "js", "ts", "mjs", "cjs", "vue", "svelte",
];
const SKIP_DIRS: &[&str] = &["node_modules", "target", "dist", "build"];
const MAX_FILES: usize = 5000;
/// Directory entries of every kind the walk may look at, so a huge tree of
/// non-matching files cannot stall a verb call.
const MAX_VISITED_ENTRIES: usize = 50_000;
const MAX_DEPTH: usize = 32;
const MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
const MAX_NEEDLE_CHARS: usize = 80;
const MIN_NEEDLE_CHARS: usize = 3;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct SourceGuess {
    /// Worktree-relative, `/`-separated.
    pub file: String,
    /// 1-based, inclusive.
    pub line_start: usize,
    pub line_end: usize,
    /// `anchor_source`, `od_id` or `text`: which evidence found it.
    pub via: &'static str,
}

/// Lazily read text files under one root, shared across many anchors in
/// one verb call. The walk happens once; file contents are cached.
pub struct SourceIndex {
    root: PathBuf,
    /// Every searchable file, relative, in walk order. `None` until the
    /// first search.
    walk: Option<Vec<String>>,
    /// `None` value = unreadable, too large, or over the byte budget.
    cache: HashMap<String, Option<Vec<String>>>,
    bytes_read: u64,
}

impl SourceIndex {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            walk: None,
            cache: HashMap::new(),
            bytes_read: 0,
        }
    }

    pub fn locate(&mut self, anchor: &ElementAnchor) -> Option<SourceGuess> {
        if let Some(src) = &anchor.source {
            let rel = normalize(&src.file);
            let line = src.line as usize;
            if line >= 1 && self.lines(&rel).is_some_and(|lines| line <= lines.len()) {
                return Some(SourceGuess {
                    file: rel,
                    line_start: line,
                    line_end: line,
                    via: "anchor_source",
                });
            }
        }
        let order = self.search_order(&anchor.entry);
        if let Some(id) = anchor.od_id.as_deref().filter(|s| !s.is_empty()) {
            let quoted = [format!("\"{id}\""), format!("'{id}'"), format!("`{id}`")];
            if let Some(g) = self.find(&order, "od_id", |l| quoted.iter().any(|q| l.contains(q))) {
                return Some(g);
            }
        }
        let needle = text_needle(&anchor.text)?;
        self.find(&order, "text", |l| l.contains(&needle))
    }

    fn find(
        &mut self,
        order: &[String],
        via: &'static str,
        hit: impl Fn(&str) -> bool,
    ) -> Option<SourceGuess> {
        for rel in order {
            let Some(lines) = self.lines(rel) else {
                continue;
            };
            if let Some(i) = lines.iter().position(|l| hit(l)) {
                return Some(SourceGuess {
                    file: rel.clone(),
                    line_start: i + 1,
                    line_end: i + 1,
                    via,
                });
            }
        }
        None
    }

    /// The entry file, then the rest of its directory tree, then the rest
    /// of the worktree; no file twice.
    fn search_order(&mut self, entry: &str) -> Vec<String> {
        let entry = normalize(entry);
        let walk = self.walk.get_or_insert_with(|| walk_root(&self.root));
        let dir = entry.rsplit_once('/').map_or("", |(d, _)| d);
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        let mut order = vec![entry.clone()];
        let (near, far): (Vec<&String>, Vec<&String>) = walk
            .iter()
            .filter(|f| **f != entry)
            .partition(|f| f.starts_with(&prefix));
        order.extend(near.into_iter().cloned());
        order.extend(far.into_iter().cloned());
        order
    }

    fn lines(&mut self, rel: &str) -> Option<&Vec<String>> {
        if !self.cache.contains_key(rel) {
            let loaded = self.load(rel);
            self.cache.insert(rel.to_string(), loaded);
        }
        self.cache.get(rel)?.as_ref()
    }

    fn load(&mut self, rel: &str) -> Option<Vec<String>> {
        let path = resolve_under(&self.root, rel).ok()?;
        let len = fs::metadata(&path).ok()?.len();
        if len > MAX_SERVED || self.bytes_read + len > MAX_TOTAL_BYTES {
            return None;
        }
        let bytes = fs::read(&path).ok()?;
        self.bytes_read += bytes.len() as u64;
        Some(
            String::from_utf8_lossy(&bytes)
                .lines()
                .map(str::to_string)
                .collect(),
        )
    }
}

fn normalize(rel: &str) -> String {
    let s = rel.replace('\\', "/");
    s.strip_prefix("./").unwrap_or(&s).to_string()
}

/// First non-empty line, trimmed, cut to 80 chars; `None` if too short.
fn text_needle(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let cut: String = line.chars().take(MAX_NEEDLE_CHARS).collect();
    let cut = cut.trim().to_string();
    (cut.chars().filter(|c| !c.is_whitespace()).count() >= MIN_NEEDLE_CHARS).then_some(cut)
}

/// Deterministic walk: entries sorted by name, symlinks not followed.
fn walk_root(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut visited = 0;
    walk_dir(root, "", 0, &mut visited, &mut out);
    out
}

/// Stops quietly at `MAX_FILES` matches, `MAX_VISITED_ENTRIES` entries of
/// any kind, or `MAX_DEPTH` levels.
fn walk_dir(dir: &Path, rel: &str, depth: usize, visited: &mut usize, out: &mut Vec<String>) {
    if depth >= MAX_DEPTH {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.filter_map(Result::ok).collect();
    entries.sort_by_key(fs::DirEntry::file_name);
    for e in entries {
        if out.len() >= MAX_FILES || *visited >= MAX_VISITED_ENTRIES {
            return;
        }
        *visited += 1;
        let Ok(ft) = e.file_type() else { continue };
        // std's `is_symlink` on Windows is true for any reparse point whose
        // tag is a name surrogate, so directory junctions are skipped too.
        if ft.is_symlink() {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        let child = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if ft.is_dir() {
            if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk_dir(&e.path(), &child, depth + 1, visited, out);
        } else if ft.is_file()
            && Path::new(&name)
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| EXTENSIONS.contains(&x.to_ascii_lowercase().as_str()))
        {
            out.push(child);
        }
    }
}

#[cfg(test)]
mod tests;
