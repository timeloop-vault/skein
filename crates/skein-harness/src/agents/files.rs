//! Agent definition files on disk, and their frontmatter.
//!
//! Shared between both harnesses on purpose: Claude and opencode both
//! keep agents as `---`-fenced markdown with the same handful of keys,
//! so the reader is one piece of code and only the directories differ.

use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use super::def::{AgentDef, AgentSource};

/// Markdown agent definitions in one directory, recursively.
///
/// Claude allows subdirectories under `agents/`, and the name comes
/// from the frontmatter when it has one and the file stem otherwise —
/// so a nested file is named by its stem, not its path. `namespace`
/// prefixes plugin agents the way the CLI does (`plugin:agent`).
pub(super) fn read_agent_dir(
    dir: &Path,
    source: &AgentSource,
    namespace: Option<&str>,
) -> Vec<AgentDef> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(read_agent_dir(&path, source, namespace));
            continue;
        }
        if path.extension().and_then(OsStr::to_str) != Some("md") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let fm = parse_frontmatter(&text);
        let stem = path
            .file_stem()
            .and_then(OsStr::to_str)
            .unwrap_or_default()
            .to_owned();
        let bare = fm.as_ref().and_then(|f| f.name.clone()).unwrap_or(stem);
        if bare.is_empty() {
            continue;
        }
        let name = match namespace {
            Some(ns) => format!("{ns}:{bare}"),
            None => bare,
        };
        out.push(AgentDef {
            name,
            description: fm.as_ref().and_then(|f| f.description.clone()),
            tools: fm.as_ref().and_then(|f| f.tools.clone()),
            model: fm.as_ref().and_then(|f| f.model.clone()),
            source: source.clone(),
        });
    }
    out
}

/// The fields we read out of an agent file's YAML frontmatter.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FrontMatter {
    pub name: Option<String>,
    pub description: Option<String>,
    pub tools: Option<Vec<String>>,
    pub model: Option<String>,
}

/// Parse the `---`-fenced frontmatter of an agent definition.
///
/// Deliberately not a YAML parser: agent frontmatter is a handful of
/// scalars, and the shapes seen in the wild are a one-line scalar
/// (`model: sonnet`), an inline comma list (`tools: Read, Glob`), an
/// inline flow sequence (`tools: [Read, Glob]`), a block sequence, and
/// a block scalar (`description: >-`). Pulling in a YAML dependency to
/// read four keys would cost more than it explains. Anything unknown
/// is skipped, never guessed at.
pub fn parse_frontmatter(md: &str) -> Option<FrontMatter> {
    let body = frontmatter_block(md)?;
    let lines: Vec<&str> = body.lines().collect();
    let mut fm = FrontMatter::default();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        // Only top-level keys; anything indented belongs to the value
        // above and is consumed there.
        if line.trim().is_empty() || line.starts_with(char::is_whitespace) {
            continue;
        }
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let rest = rest.trim();
        // Gather any block that follows: indented continuation lines.
        let start = i;
        while i < lines.len() && (lines[i].starts_with(char::is_whitespace) || lines[i].is_empty())
        {
            i += 1;
        }
        let block = &lines[start..i];
        match key {
            "name" => fm.name = scalar(rest, block),
            "description" => fm.description = scalar(rest, block),
            "model" => fm.model = scalar(rest, block),
            "tools" => fm.tools = Some(string_list(rest, block)),
            _ => {}
        }
    }
    Some(fm)
}

/// The text between the opening and closing `---`, or `None` when the
/// file has no frontmatter.
fn frontmatter_block(md: &str) -> Option<&str> {
    // A BOM survives `read_to_string` on Windows-authored files and
    // would hide the opening fence.
    let text = md.strip_prefix('\u{feff}').unwrap_or(md);
    let rest = text.strip_prefix("---")?;
    let rest = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))?;
    let end = rest
        .split_inclusive('\n')
        .scan(0usize, |at, line| {
            let here = *at;
            *at += line.len();
            Some((here, line))
        })
        .find(|(_, line)| matches!(line.trim_end(), "---" | "..."))
        .map(|(at, _)| at)?;
    Some(&rest[..end])
}

/// A scalar value: inline when there is one, otherwise the block
/// underneath folded into a single line.
fn scalar(inline: &str, block: &[&str]) -> Option<String> {
    let inline = inline.trim();
    // `>` / `|` (with any chomping indicator) say the value is the
    // block below; a bare empty value means the same thing.
    let is_block_marker = inline.is_empty() || inline.starts_with('>') || inline.starts_with('|');
    if !is_block_marker {
        return Some(unquote(inline));
    }
    let literal = inline.starts_with('|');
    let joined: Vec<&str> = block
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    if joined.is_empty() {
        return None;
    }
    Some(joined.join(if literal { "\n" } else { " " }))
}

/// A list value: an inline flow sequence, an inline comma list, or a
/// block sequence of `- item` lines.
fn string_list(inline: &str, block: &[&str]) -> Vec<String> {
    let inline = inline.trim();
    if !inline.is_empty() {
        let inner = inline
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .unwrap_or(inline);
        return inner
            .split(',')
            .map(|s| unquote(s.trim()))
            .filter(|s| !s.is_empty())
            .collect();
    }
    block
        .iter()
        .filter_map(|l| l.trim().strip_prefix("- "))
        .map(|s| unquote(s.trim()))
        .filter(|s| !s.is_empty())
        .collect()
}

/// Strip one layer of matching quotes.
fn unquote(s: &str) -> String {
    let s = s.trim();
    for q in ['"', '\''] {
        if s.len() >= 2 && s.starts_with(q) && s.ends_with(q) {
            return s[1..s.len() - 1].to_owned();
        }
    }
    s.to_owned()
}

#[cfg(test)]
mod tests {
    use super::super::def::allows_mcp_tools;
    use super::*;

    #[test]
    fn parses_an_inline_comma_tools_list() {
        // The shape `~/.claude/agents/coder.md` actually uses.
        let md = "---\nname: coder\ndescription: Use proactively for implementation work.\ntools: Read, Glob, Grep, Edit, Write, Bash, PowerShell\nmodel: sonnet\ncolor: green\n---\n\nYou are an implementation subagent.\n";
        let fm = parse_frontmatter(md).expect("has frontmatter");
        assert_eq!(fm.name.as_deref(), Some("coder"));
        assert_eq!(
            fm.description.as_deref(),
            Some("Use proactively for implementation work.")
        );
        assert_eq!(fm.model.as_deref(), Some("sonnet"));
        let expected = [
            "Read",
            "Glob",
            "Grep",
            "Edit",
            "Write",
            "Bash",
            "PowerShell",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(fm.tools, Some(expected));
    }

    #[test]
    fn parses_a_flow_sequence_and_a_block_sequence() {
        let flow = parse_frontmatter("---\ntools: [Read, \"Glob\"]\n---\n").expect("frontmatter");
        assert_eq!(flow.tools, Some(vec!["Read".to_owned(), "Glob".to_owned()]));
        let block =
            parse_frontmatter("---\ntools:\n  - Read\n  - Glob\n---\n").expect("frontmatter");
        assert_eq!(
            block.tools,
            Some(vec!["Read".to_owned(), "Glob".to_owned()])
        );
    }

    #[test]
    fn folds_a_block_scalar_description() {
        let md = "---\nname: x\ndescription: >-\n  first line\n  second line\n---\n";
        let fm = parse_frontmatter(md).expect("frontmatter");
        assert_eq!(fm.description.as_deref(), Some("first line second line"));
    }

    #[test]
    fn no_tools_key_means_unrestricted_not_empty() {
        let fm = parse_frontmatter("---\nname: x\n---\n").expect("frontmatter");
        // The distinction the picker's warning depends on.
        assert!(fm.tools.is_none());
        assert!(allows_mcp_tools(fm.tools.as_deref()));
    }

    #[test]
    fn a_file_without_frontmatter_is_not_an_agent() {
        assert!(parse_frontmatter("# just markdown\n").is_none());
        assert!(
            parse_frontmatter("---\nname: x\n").is_none(),
            "unterminated fence"
        );
    }

    #[test]
    fn survives_a_bom_and_crlf() {
        let md = "\u{feff}---\r\nname: x\r\ndescription: hi\r\n---\r\n";
        let fm = parse_frontmatter(md).expect("frontmatter");
        assert_eq!(fm.name.as_deref(), Some("x"));
        assert_eq!(fm.description.as_deref(), Some("hi"));
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
        fs::write(path, text).expect("write");
    }

    #[test]
    fn reads_a_directory_of_definitions_recursively() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("agents");
        write(
            &dir.join("coder.md"),
            "---\nname: coder\ndescription: d1\n---\n",
        );
        // Frontmatter with no name falls back to the file stem.
        write(
            &dir.join("nested").join("scribe.md"),
            "---\ndescription: d2\n---\n",
        );
        // Not markdown, and markdown that is not an agent.
        write(&dir.join("notes.txt"), "ignore me");
        write(&dir.join("README.md"), "no frontmatter here");
        let mut defs = read_agent_dir(&dir, &AgentSource::User, None);
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["README", "coder", "scribe"]);
        assert_eq!(defs[1].description.as_deref(), Some("d1"));
        assert_eq!(defs[2].description.as_deref(), Some("d2"));
    }

    #[test]
    fn plugin_agents_get_the_namespace_the_cli_uses() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("agents");
        write(
            &dir.join("code-reviewer.md"),
            "---\nname: code-reviewer\ndescription: d\n---\n",
        );
        let source = AgentSource::Plugin {
            plugin: "pr-review-toolkit".to_owned(),
        };
        let defs = read_agent_dir(&dir, &source, Some("pr-review-toolkit"));
        assert_eq!(defs.len(), 1);
        // Must match the probe's spelling exactly, or enrichment misses.
        assert_eq!(defs[0].name, "pr-review-toolkit:code-reviewer");
    }
}
