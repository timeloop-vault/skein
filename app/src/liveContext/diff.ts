// Diff rendering shapes, and the two harness patch-payload parsers.
//
// The review surface owns diffing now (`app/src/review/`, issue #212);
// the Diff card and its baseline hook are gone. What is left here is:
//
//   • the rendered line/hunk shape, which the backend's
//     `skein-review::Hunk` serializes into unchanged, and
//   • the two parsers that turn a harness's *own* reported patch into
//     that shape. Those are no longer a diff source — the baseline
//     model covers gitignored and non-repo paths directly — but they
//     are still how a `patch` row's line ranges are recovered for
//     per-hunk harness attribution (D4). See `review.ts`.

import { num, obj, str } from "./payload.ts";

/// One diff line. Mirrors the Rust `HunkLine` (skein-review) and
/// `DiffLineDto` (git.rs) — one renderer handles both.
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
