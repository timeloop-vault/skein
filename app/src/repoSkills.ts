// Reads a room's repo skills — `.claude/skills/*/SKILL.md` — for #359's
// invocation nudges. Pure: no React, no Tauri import, no network. The
// Tauri-backed `list_dir` / `read_file_text` commands are injected as an
// `io` parameter so this module (and its tests) never touch the real
// filesystem — the same seam FileTree.tsx's caller already has to cross
// for those two commands.
//
// SKILL.md's frontmatter is YAML, but pulling in a YAML parser for two
// scalar fields would be a dependency for what is, in practice, a strict
// subset: `name:`/`description:` as a plain, quoted, or block-scalar
// value. `parseSkillFrontmatter` hand-rolls just that subset rather than
// adding one.
//
// Per Claude Code's own docs (code.claude.com/docs/en/skills), a project
// skill's slash COMMAND is always its directory name — frontmatter
// `name` is only a display label, defaulting to the directory name when
// absent. `RepoSkill.command` is therefore what gets typed; `.name` is
// what gets shown.

export interface RepoSkill {
	/// The slash-invocable token — always the skill's directory name,
	/// never the frontmatter `name`. Must pass the same safe-token check
	/// as a CLI would apply to a directory-derived command.
	command: string;
	/// Display label: frontmatter `name` if present, else `dirName`
	/// (same fallback as `command`, but never token-checked — it is
	/// shown, not typed).
	name: string;
	description: string;
}

interface SkillDirEntry {
	name: string;
	kind: "file" | "dir" | "symlink";
}

/// What `loadRepoSkills` needs from the filesystem — mirrors the shape
/// of Skein's `list_dir` / `read_file_text` Tauri commands (see
/// `FileTree.tsx` / `FilesBody.tsx`) so a real caller can pass those
/// straight through, and a test can pass an in-memory fake.
export interface RepoSkillsIo {
	listDir(path: string): Promise<SkillDirEntry[]>;
	readFile(path: string): Promise<string>;
}

/// A token safe to type into a terminal: letters/digits, optionally
/// hyphen/underscore-separated groups of more letters/digits — no
/// leading/trailing separator, no run of separators, no spaces, slashes,
/// colons or control characters — and short enough to obviously be a
/// command rather than pasted text.
const SAFE_TOKEN = /^[a-z0-9]+([-_][a-z0-9]+)*$/i;
const MAX_TOKEN_LENGTH = 64;

const BLOCK_SCALAR = /^[|>][+-]?$/;
// A top-level (unindented) `key: value` line. `[^\s:]` as the first key
// character is what excludes an indented continuation line — those start
// with whitespace, so the anchor at ^ never matches them.
const TOP_LEVEL_KEY = /^([^\s:][^:]*):[ \t]?(.*)$/;

function isSafeToken(token: string): boolean {
	return token.length > 0 && token.length <= MAX_TOKEN_LENGTH && SAFE_TOKEN.test(token);
}

/// Collapses whitespace/control characters to single spaces and trims —
/// display text folded out of a block scalar, or read off a directory
/// name, may carry newlines or runs of spaces that don't belong in a
/// one-line label.
function sanitizeDisplayName(value: string): string {
	// biome-ignore lint/suspicious/noControlCharactersInRegex: deliberately stripping raw control bytes from a display label
	return value.replace(/[\x00-\x1f\x7f\s]+/g, " ").trim();
}

function unquote(value: string): string {
	if (value.length >= 2) {
		const first = value[0];
		const last = value[value.length - 1];
		if ((first === '"' && last === '"') || (first === "'" && last === "'")) {
			return value.slice(1, -1);
		}
	}
	return value;
}

/// Parses just the `name`/`description` top-level frontmatter keys out of
/// a SKILL.md's text and pairs them with `dirName`, the directory the
/// file lives in. Tolerates a leading UTF-8 BOM and CRLF line endings.
/// Returns `null` when there is no frontmatter, the frontmatter is
/// unterminated, or `dirName` isn't a safe slash token — that's the
/// `command` this skill would be typed with, regardless of what
/// frontmatter `name` says.
export function parseSkillFrontmatter(text: string, dirName: string): RepoSkill | null {
	if (!isSafeToken(dirName)) return null;

	const src = text.charCodeAt(0) === 0xfeff ? text.slice(1) : text;
	const lines = src.split(/\r\n|\n/);
	if ((lines[0] ?? "").trim() !== "---") return null;

	let end = -1;
	for (let i = 1; i < lines.length; i++) {
		if ((lines[i] ?? "").trim() === "---") {
			end = i;
			break;
		}
	}
	if (end === -1) return null;

	const body = lines.slice(1, end);
	const fields: { name?: string; description?: string } = {};

	let i = 0;
	while (i < body.length) {
		const line = body[i] ?? "";
		const m = TOP_LEVEL_KEY.exec(line);
		if (!m) {
			i++;
			continue;
		}
		const key = (m[1] ?? "").trim();
		const rest = (m[2] ?? "").trim();
		i++;
		if (key === "name" || key === "description") {
			if (BLOCK_SCALAR.test(rest)) {
				const collected: string[] = [];
				while (i < body.length) {
					const next = body[i] ?? "";
					if (next.length > 0 && !/^[ \t]/.test(next)) break;
					const trimmed = next.trim();
					if (trimmed) collected.push(trimmed);
					i++;
				}
				fields[key] = collected.join(" ");
			} else {
				fields[key] = unquote(rest);
			}
		}
	}

	const name = sanitizeDisplayName(fields.name ?? dirName);
	return { command: dirName, name, description: fields.description ?? "" };
}

/// `template` (a `HarnessKindMeta.skillInvocation`, e.g. `"/{name}"`)
/// with its `{name}` placeholder filled in by `command` — the skill's
/// slash-invocable token (`RepoSkill.command`), not its display `name`.
export function skillInvocationLine(template: string, command: string): string {
	return template.replaceAll("{name}", command);
}

// Matches FileTree.tsx's own `joinPath`: `cwd` may be a Windows path, so
// the separator is inferred from `a` rather than assumed.
function joinPath(a: string, b: string): string {
	if (!b) return a;
	const sep = a.includes("\\") && !a.includes("/") ? "\\" : "/";
	const trimmed = a.replace(/[\\/]+$/, "");
	return `${trimmed}${sep}${b.replace(/\//g, sep)}`;
}

/// Every skill under `<cwd>/.claude/skills/*/SKILL.md`, sorted by
/// `command` and deduplicated on it (first after sort wins). Never
/// throws: a missing or unreadable `.claude/skills` yields `[]`; a
/// `file` entry is skipped (only `dir`/`symlink` entries are descended
/// into, and a `symlink` that fails to read is skipped like any other
/// unreadable entry); a SKILL.md that fails to read or fails to parse is
/// skipped, not fatal to the rest.
export async function loadRepoSkills(cwd: string, io: RepoSkillsIo): Promise<RepoSkill[]> {
	const skillsDir = joinPath(cwd, ".claude/skills");

	let entries: SkillDirEntry[];
	try {
		entries = await io.listDir(skillsDir);
	} catch {
		return [];
	}

	const found: RepoSkill[] = [];
	for (const entry of entries) {
		if (entry.kind !== "dir" && entry.kind !== "symlink") continue;
		const skillPath = joinPath(joinPath(skillsDir, entry.name), "SKILL.md");
		let text: string;
		try {
			text = await io.readFile(skillPath);
		} catch {
			continue;
		}
		const skill = parseSkillFrontmatter(text, entry.name);
		if (skill) found.push(skill);
	}

	found.sort((a, b) => a.command.localeCompare(b.command));
	const byCommand = new Map<string, RepoSkill>();
	for (const skill of found) {
		if (!byCommand.has(skill.command)) byCommand.set(skill.command, skill);
	}
	return [...byCommand.values()];
}
