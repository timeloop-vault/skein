// Review baseline — the Diff card's data layer (issue #211, epic #52 D3/D4).
//
// The card used to derive its tabs from every `patch` row the room had
// ever seen, so nothing ever cleared. It now renders the *pending*
// diff: the backend holds a persisted baseline per file (the content
// the user last reviewed) and reports what has changed since. A tab
// disappears when, and only when, the file is reviewed.
//
// Two asymmetric verbs, mirroring `crates/skein-review`:
//   accept — advances the baseline, never touches disk
//   reject — writes disk back to the baseline, never moves it
//
// Harness attribution (D4) is a chip and a filter, never a partition:
// there is one worktree per room, so there is exactly one diff.
// `attributeHunks` joins pending hunks back to the `patch` rows already
// in the Live Context store to say which harness wrote each one.

import { invoke } from "@tauri-apps/api/core";
import { type DiffHunk, normalizeStructuredPatch, parseUnifiedDiff } from "./diff.ts";
import { obj, parsePayload, str } from "./payload.ts";
import type { HarnessAction } from "./store.ts";

/// Why a file has no line diff. Mirrors `PendingFileDto::blocked`.
export type BlockedReason = "binary" | "toolarge" | "symlink" | "unreadable";

/// A pending hunk. Superset of the renderer's `DiffHunk` — the extra
/// coordinates are the identity accept/reject verify against, so hunks
/// must be echoed back to the backend exactly as received.
export interface ReviewHunk extends DiffHunk {
	oldStart: number;
	oldLines: number;
	newStart: number;
	newLines: number;
}

/// One file with unreviewed changes. Mirrors `PendingFileDto` (review.rs).
export interface PendingFile {
	/** Worktree-relative, forward slashes — the identity for every command. */
	path: string;
	name: string;
	/** Harness that most recently wrote this file. */
	harnessId: string;
	change: "added" | "deleted" | "modified";
	blocked?: BlockedReason;
	additions: number;
	deletions: number;
	/** Echoed back on whole-file accept/reject as the staleness guard.
	 *  A decimal string because a JS number cannot hold 64 bits. */
	contentHash: string;
	hunks: ReviewHunk[];
	touchedMs: number;
}

export const fetchPending = (roomId: string, cwd: string): Promise<PendingFile[]> =>
	invoke<PendingFile[]>("review_pending", { roomId, cwd });

/// Advance the baseline. `path` undefined accepts every pending file in
/// the room; `hunks` empty accepts the whole file. Never writes to disk.
export const acceptReview = (
	roomId: string,
	cwd: string,
	path: string | undefined,
	hunks: ReviewHunk[],
	contentHash: string | undefined,
): Promise<number> =>
	invoke<number>("review_accept", {
		roomId,
		cwd,
		path: path ?? null,
		hunks,
		contentHash: contentHash ?? null,
	});

/// Put the working tree back to the baseline. `hunks` empty rejects the
/// whole file. There is deliberately no reject-all.
export const rejectReview = (
	roomId: string,
	cwd: string,
	path: string,
	hunks: ReviewHunk[],
	contentHash: string | undefined,
): Promise<void> =>
	invoke<void>("review_reject", {
		roomId,
		cwd,
		path,
		hunks,
		contentHash: contentHash ?? null,
	});

/// The line span a harness's own reported patch claims to have written,
/// in post-edit coordinates.
interface Touch {
	harnessId: string;
	ranges: Array<[number, number]>;
}

/// Does this action's payload name `relPath`? Backend keys are
/// worktree-relative with forward slashes; harnesses report absolute
/// paths in the OS separator, so match by suffix (the leading "/"
/// guards against partial-segment matches — same rule as matchGitFile).
function namesFile(payload: ReturnType<typeof parsePayload>, relPath: string): boolean {
	const files = Array.isArray(payload.files) ? payload.files : [];
	const input = obj(payload.input);
	const candidates = [...files.map((f) => str(f)), str(input?.filePath), str(input?.file_path)];
	const want = relPath.replace(/\\/g, "/");
	return candidates.some((c) => {
		if (!c) return false;
		const norm = c.replace(/\\/g, "/");
		return norm === want || norm.endsWith(`/${want}`);
	});
}

/// The post-edit line spans a `patch` row reports, from whichever of the
/// two payload shapes it carries (opencode's unified-diff string or
/// Claude's `structured_patch`).
function touchRanges(payload: ReturnType<typeof parsePayload>): Array<[number, number]> {
	const pinfo = obj(payload.patch_info);
	if (!pinfo) return [];
	const diffStr = str(pinfo.diff);
	const hunks: DiffHunk[] = diffStr
		? parseUnifiedDiff(diffStr)
		: Array.isArray(pinfo.structured_patch)
			? normalizeStructuredPatch(pinfo.structured_patch)
			: [];
	const out: Array<[number, number]> = [];
	for (const h of hunks) {
		const nums = h.lines.map((l) => l.newLineno).filter((n): n is number => n != null);
		if (nums.length > 0) out.push([Math.min(...nums), Math.max(...nums)]);
	}
	return out;
}

/// Which harness wrote each pending hunk, one entry per hunk.
///
/// Best effort, and deliberately so: a patch row's line numbers are from
/// the moment of that edit, and later edits shift them. When no row's
/// span overlaps a hunk we fall back to the file's last writer, which is
/// always true about the file even when it is imprecise about the line.
/// Getting this wrong costs a mislabelled chip, never a wrong diff.
export function attributeHunks(
	actions: HarnessAction[],
	file: PendingFile,
): Array<string | undefined> {
	const touches: Touch[] = [];
	for (const a of actions) {
		if (a.kind !== "patch") continue;
		const p = parsePayload(a.payload);
		if (p.is_error === true) continue;
		const tool = (str(p.tool) ?? "").toLowerCase();
		if (tool !== "edit" && tool !== "write" && tool !== "multiedit") continue;
		if (!namesFile(p, file.path)) continue;
		touches.push({ harnessId: a.harnessId, ranges: touchRanges(p) });
	}

	return file.hunks.map((h) => {
		const from = h.newStart;
		const to = h.newStart + h.newLines;
		// Newest first — the most recent writer of these lines owns them.
		for (let i = touches.length - 1; i >= 0; i--) {
			const t = touches[i];
			if (!t) continue;
			if (t.ranges.some(([lo, hi]) => lo < to && hi >= from)) return t.harnessId;
		}
		return file.harnessId;
	});
}

/// Every harness that owns a pending file, in tab order — the filter row.
export function contributingHarnesses(files: PendingFile[]): string[] {
	const seen: string[] = [];
	for (const f of files) if (!seen.includes(f.harnessId)) seen.push(f.harnessId);
	return seen;
}
