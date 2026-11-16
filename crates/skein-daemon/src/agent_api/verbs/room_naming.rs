//! Room naming: the pure helpers that name a new room's branch, worktree
//! folder and room. A one-to-one port of the TypeScript in
//! `app/src/{branchName,roomName}.ts` (#734), so the daemon can create a
//! room without the webview. Only the built-in default branch template is
//! ported; per-folder and app-level templates live in the webview's
//! localStorage and are deliberately not.
//!
//! `derive_prompt_first_line` is ported here too (the Rust `room_create`
//! has no equivalent).

/// The template every folder starts with until it remembers its own.
pub(super) const DEFAULT_BRANCH_TEMPLATE: &str = "skein/{slug}";

/// Cap on `Room.createdBy.promptFirstLine`, in UTF-16 code units like JS.
pub(super) const PROMPT_FIRST_LINE_MAX: usize = 200;

/// JavaScript's `String.prototype.trim` whitespace set: Unicode
/// `White_Space` minus U+0085 (which Rust includes), plus U+FEFF.
fn js_ws(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

fn js_trim(s: &str) -> &str {
    s.trim_matches(js_ws)
}

/// Lowercase, collapse every run of non `[a-z0-9]` into one `-`, strip a
/// leading and a trailing `-`. Shared by `task_slug` and `worktree_leaf`.
fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in js_trim(s).to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c);
        } else {
            pending_dash = true;
        }
    }
    out
}

/// Slug from free-form task text, capped at 28 chars, `"task"` when it
/// collapses to nothing.
pub(super) fn task_slug(task: &str) -> String {
    let mut slug = slugify(task);
    // ASCII only by now, so byte truncation is safe. Truncation can leave a
    // trailing `-`; the TS does the same (it strips before slicing).
    slug.truncate(28);
    if slug.is_empty() {
        "task".to_string()
    } else {
        slug
    }
}

/// Fill `{slug}` into a branch template; a template without the
/// placeholder gets the slug appended, a blank one is just the slug.
pub(super) fn apply_branch_template(template: &str, slug: &str) -> String {
    let trimmed = js_trim(template);
    if trimmed.is_empty() {
        return slug.to_string();
    }
    if trimmed.contains("{slug}") {
        return trimmed.replace("{slug}", slug);
    }
    format!("{trimmed}{slug}")
}

/// The template a successful branch name is remembered as: the prefix up
/// to and including its last `/`, plus `{slug}`.
#[cfg(test)]
pub(super) fn template_from_branch(branch: &str) -> String {
    match branch.rfind('/') {
        Some(idx) => format!("{}{{slug}}", &branch[..=idx]),
        None => "{slug}".to_string(),
    }
}

/// Last `/`-segment of a branch, sanitized like `task_slug` with no length
/// cap. This is the leaf passed to `git_propose_worktree_path`.
pub(super) fn worktree_leaf(branch: &str) -> String {
    let last = branch.rfind('/').map_or(branch, |i| &branch[i + 1..]);
    let slug = slugify(last);
    if slug.is_empty() {
        "task".to_string()
    } else {
        slug
    }
}

/// A rough `git check-ref-format --branch` for a new branch name. The
/// message texts are the webview's, character for character.
pub(super) fn branch_name_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("can't be empty");
    }
    if name == "@" {
        return Some("can't be just \"@\"");
    }
    if name.starts_with('-') {
        return Some("can't start with -");
    }
    if name.starts_with('/') || name.ends_with('/') {
        return Some("can't start or end with /");
    }
    if name.contains("//") {
        return Some("can't contain //");
    }
    if name.contains("..") {
        return Some("can't contain ..");
    }
    if name.contains("@{") {
        return Some("can't contain @{");
    }
    if name.ends_with('.') {
        return Some("can't end with .");
    }
    let special = |c: char| {
        c <= '\u{1f}'
            || c == '\u{7f}'
            || matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\')
    };
    if name.chars().any(special) {
        return Some("contains a space or a special character");
    }
    for part in name.split('/') {
        if part.is_empty() {
            return Some("can't contain //");
        }
        if part.starts_with('.') {
            return Some("no part can start with .");
        }
        // Git's rule is case-sensitive, and this is not a file extension.
        #[allow(clippy::case_sensitive_file_extension_comparisons)]
        let locked = part.ends_with(".lock");
        if locked {
            return Some("can't end with .lock");
        }
    }
    None
}

/// Exact match against the repo's local branch names.
pub(super) fn branch_collision(name: &str, existing: &[String]) -> bool {
    existing.iter().any(|b| b == name)
}

/// The one problem to report for a branch name, in priority order: the
/// name itself, a collision with an existing branch, a worktree folder
/// already sitting at the proposed path.
pub(super) fn branch_field_problem(
    name: &str,
    existing_branches: &[String],
    worktree_folder_exists: bool,
) -> Option<&'static str> {
    if let Some(problem) = branch_name_problem(name) {
        return Some(problem);
    }
    if branch_collision(name, existing_branches) {
        return Some("branch already exists");
    }
    if worktree_folder_exists {
        return Some("worktree folder already exists");
    }
    None
}

/// The trailing path component of `cwd` (either separator, trailing
/// separators ignored): a new room's default name and `Room.repo`. Falls
/// back to `cwd` itself when there is none.
pub(super) fn default_room_name(cwd: &str) -> String {
    let sep = |c: char| c == '/' || c == '\\';
    let last = cwd.trim_end_matches(sep).rsplit(sep).next().unwrap_or("");
    if last.is_empty() { cwd } else { last }.to_string()
}

/// `Room.createdBy.promptFirstLine`: the explicit value when not blank,
/// else the first non-blank line of `prompt`, trimmed and capped at
/// [`PROMPT_FIRST_LINE_MAX`] UTF-16 units. `None` when neither yields text.
pub(super) fn derive_prompt_first_line(
    explicit: Option<&str>,
    prompt: Option<&str>,
) -> Option<String> {
    if let Some(e) = explicit.map(js_trim).filter(|e| !e.is_empty()) {
        return Some(cap_utf16(e, PROMPT_FIRST_LINE_MAX));
    }
    // The TS splits on `\r?\n`; splitting on `\n` and trimming drops the
    // stray `\r` the same way.
    let line = prompt?.split('\n').find(|l| !js_trim(l).is_empty())?;
    Some(cap_utf16(js_trim(line), PROMPT_FIRST_LINE_MAX))
}

/// `slice(0, max)` on UTF-16 units, never splitting a surrogate pair (JS
/// would leave a lone surrogate, which a Rust `String` cannot hold).
fn cap_utf16(s: &str, max: usize) -> String {
    let mut units = 0;
    let mut out = String::new();
    for c in s.chars() {
        units += c.len_utf16();
        if units > max {
            break;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests;
