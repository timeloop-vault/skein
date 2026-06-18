// Diff card data layer — issue #80 D3.
//
// The card is harness-first like Plan/Activity: its tabs, active file,
// and flicker all derive from the room's `patch` rows (what the agents
// touched), NOT from git. The diff *body* prefers the live worktree diff
// (git_diff — cumulative vs HEAD, structured) and falls back to the
// harness's own reported patch when git can't show the file (no repo, a
// gitignored path, or a file git doesn't list) — so the body is never
// blank for something an agent demonstrably edited. Both sources
// normalise to one shape so a single renderer handles them.

import { basename } from "./Row.tsx";
import { type Payload, num, obj, parsePayload, str } from "./payload.ts";
import type { HarnessAction } from "./store.ts";

/// One diff line. Mirrors the Rust `DiffLineDto` (git.rs) so a git
/// FileDiff drops in unchanged; harness-patch parsing produces the same.
export interface DiffLine {
	kind: "context" | "add" | "delete";
	content: string;
	oldLineno?: number;
	newLineno?: number;
}
export interface DiffHunk {
	header: string;
	lines: DiffLine[];
}
/// Matches the `git_diff` command's `FileDiffDto`.
export interface FileDiff {
	path: string;
	kind: string;
	binary: boolean;
	hunks: DiffHunk[];
}

/// A diff tab — one per file an agent touched this session.
export interface DiffTab {
	/** Full path as the harness reported it (for git matching + title). */
	fullPath: string;
	/** Display label (last path segment). */
	file: string;
	/** Harness whose latest edit owns this tab (drives the chip). */
	harnessId: string;
	/** Cumulative additions/deletions across this file's patch rows;
	 *  undefined when no row carried patch_info. */
	adds: number | undefined;
	dels: number | undefined;
}

/// A patch row that names a single edited file. Excludes the opencode
/// multi-file commit snapshot ({files, hash} — no tool, no patch_info)
/// and errored patches.
interface PatchInfo {
	fullPath: string;
	harnessId: string;
	adds: number | undefined;
	dels: number | undefined;
	payload: Payload;
}

function patchInfo(a: HarnessAction): PatchInfo | undefined {
	if (a.kind !== "patch") return undefined;
	const p = parsePayload(a.payload);
	if (p.is_error === true) return undefined;
	// A real single-file edit carries an edit/write/multiedit tool. The
	// opencode multi-file commit snapshot ({files, hash}) has no tool —
	// exclude it so it doesn't own a tab or shadow the file's real edit
	// in the harness-patch fallback.
	const tool = (str(p.tool) ?? "").toLowerCase();
	if (tool !== "edit" && tool !== "write" && tool !== "multiedit") return undefined;
	const files = Array.isArray(p.files) ? p.files : [];
	const file = str(files[0]) ?? str(obj(p.input)?.filePath) ?? str(obj(p.input)?.file_path);
	if (!file) return undefined;
	const pi = obj(p.patch_info);
	return {
		fullPath: file,
		harnessId: a.harnessId,
		adds: pi ? num(pi.additions) : undefined,
		dels: pi ? num(pi.deletions) : undefined,
		payload: p,
	};
}

/// The set of diff tabs, in most-recently-touched-last order, each
/// carrying the harness of its latest edit and summed deltas. Derived
/// purely from the feed (display-ordered actions in, see orderForDisplay).
export function deriveTabs(actions: HarnessAction[]): DiffTab[] {
	const byPath = new Map<string, DiffTab>();
	for (const a of actions) {
		const pi = patchInfo(a);
		if (!pi) continue;
		const existing = byPath.get(pi.fullPath);
		const addDelta = (base: number | undefined, d: number | undefined) =>
			d == null ? base : (base ?? 0) + d;
		if (existing) {
			existing.adds = addDelta(existing.adds, pi.adds);
			existing.dels = addDelta(existing.dels, pi.dels);
			existing.harnessId = pi.harnessId; // latest edit owns the chip
			// Re-order to most-recent-last.
			byPath.delete(pi.fullPath);
			byPath.set(pi.fullPath, existing);
		} else {
			byPath.set(pi.fullPath, {
				fullPath: pi.fullPath,
				file: basename(pi.fullPath),
				harnessId: pi.harnessId,
				adds: pi.adds,
				dels: pi.dels,
			});
		}
	}
	return [...byPath.values()];
}

/// The file the Diff card should auto-focus: the focused harness's most
/// recent edit. Falls back to the latest edit by *any* harness when the
/// focused one hasn't touched anything (so the card isn't empty just
/// because you're chatting with a harness that hasn't edited yet).
export function autoFocusFile(
	actions: HarnessAction[],
	focusedHarnessId: string | undefined,
): string | undefined {
	let latestAny: string | undefined;
	let latestFocused: string | undefined;
	for (const a of actions) {
		const pi = patchInfo(a);
		if (!pi) continue;
		latestAny = pi.fullPath;
		if (focusedHarnessId && pi.harnessId === focusedHarnessId) latestFocused = pi.fullPath;
	}
	return latestFocused ?? latestAny;
}

/// The harness's own reported diff for a file's latest patch row, as the
/// normalised hunk shape — the body fallback when git has nothing.
/// opencode carries a unified-diff string (`patch_info.diff`); Claude a
/// `structured_patch` array of hunks.
export function harnessPatchHunks(
	actions: HarnessAction[],
	fullPath: string,
): DiffHunk[] | undefined {
	let latest: Payload | undefined;
	for (const a of actions) {
		const pi = patchInfo(a);
		if (pi && pi.fullPath === fullPath) latest = pi.payload;
	}
	if (!latest) return undefined;
	const pinfo = obj(latest.patch_info);
	if (!pinfo) return undefined;
	const diffStr = str(pinfo.diff);
	if (diffStr) return parseUnifiedDiff(diffStr);
	if (Array.isArray(pinfo.structured_patch))
		return normalizeStructuredPatch(pinfo.structured_patch);
	return undefined;
}

/// Parse the start line numbers from a `@@ -a,b +c,d @@` hunk header.
function parseHunkHeader(header: string): { oldStart: number; newStart: number } {
	const m = /@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(header);
	return { oldStart: m?.[1] ? Number(m[1]) : 1, newStart: m?.[2] ? Number(m[2]) : 1 };
}

/// Parse an opencode unified-diff string into normalised hunks. Skips the
/// file headers (---/+++/diff/index) and "\ No newline" markers; tracks
/// gutter line numbers from each `@@` header.
export function parseUnifiedDiff(diff: string): DiffHunk[] {
	const hunks: DiffHunk[] = [];
	let current: DiffHunk | undefined;
	let oldNo = 0;
	let newNo = 0;
	// Split on either line ending so CRLF diffs don't leave a stray \r in
	// the rendered content.
	for (const raw of diff.split(/\r?\n/)) {
		if (raw.startsWith("@@")) {
			const { oldStart, newStart } = parseHunkHeader(raw);
			oldNo = oldStart;
			newNo = newStart;
			current = { header: raw, lines: [] };
			hunks.push(current);
			continue;
		}
		if (!current) continue; // preamble before the first hunk
		if (raw.startsWith("\\")) continue; // "\ No newline at end of file"
		const marker = raw[0];
		const content = raw.slice(1);
		if (marker === "+") {
			current.lines.push({ kind: "add", content, newLineno: newNo++ });
		} else if (marker === "-") {
			current.lines.push({ kind: "delete", content, oldLineno: oldNo++ });
		} else if (marker === " ") {
			current.lines.push({ kind: "context", content, oldLineno: oldNo++, newLineno: newNo++ });
		}
		// any other line (e.g. trailing "") is ignored
	}
	return hunks;
}

/// Map Claude's `structured_patch` (jsdiff-style hunks) into the
/// normalised shape. Each hunk: {oldStart, newStart, lines:["+..."| "-..."| " ..."]}.
export function normalizeStructuredPatch(raw: unknown[]): DiffHunk[] {
	const hunks: DiffHunk[] = [];
	for (const h of raw) {
		const ho = obj(h);
		if (!ho || !Array.isArray(ho.lines)) continue;
		let oldNo = num(ho.oldStart) ?? 1;
		let newNo = num(ho.newStart) ?? 1;
		const lines: DiffLine[] = [];
		for (const l of ho.lines) {
			const s = str(l);
			if (s == null) continue;
			const marker = s[0];
			const content = s.slice(1);
			if (marker === "+") {
				lines.push({ kind: "add", content, newLineno: newNo++ });
			} else if (marker === "-") {
				lines.push({ kind: "delete", content, oldLineno: oldNo++ });
			} else {
				lines.push({ kind: "context", content, oldLineno: oldNo++, newLineno: newNo++ });
			}
		}
		const oldStart = num(ho.oldStart) ?? 1;
		const newStart = num(ho.newStart) ?? 1;
		hunks.push({ header: `@@ -${oldStart} +${newStart} @@`, lines });
	}
	return hunks;
}

/// Find the git worktree diff for `fullPath` among `git_diff`'s results.
/// git paths are repo-relative; harness paths are absolute, so match by
/// suffix (a leading "/" guards against partial-segment matches).
export function matchGitFile(files: FileDiff[], fullPath: string): FileDiff | undefined {
	return files.find((f) => fullPath === f.path || fullPath.endsWith(`/${f.path}`));
}
