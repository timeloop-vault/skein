// The review surface's data layer (#212, epic #52 D1/D5/D6/D7).
//
// Mirrors `app/src-tauri/src/review_surface.rs`. Every type here is the
// serde shape of a DTO over there; when one changes, both change.
// (#69 tracks generating these instead of hand-maintaining them.)
//
// Three scopes, one renderer:
//   branch  — merge-base(HEAD, base) → working tree. The default, and
//             the only one that counts committed and uncommitted work
//             together, which is what a branch mid-task actually is.
//   commit  — one commit against its first parent.
//   pending — #211's baseline → disk. The only scope with accept and
//             reject, because it is the only one where the change has
//             not been committed yet.

import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import type { ElementAnchor, ElementRect } from "../elementAnchor.ts";
import type { ReviewHunk } from "../liveContext/review.ts";

export type ReviewScope = "branch" | "commit" | "pending";

/// How a thread is attached (D5).
export type ThreadScope = "line" | "file" | "commit" | "review" | "element";

/// Where an element thread's anchor stands (#434). Everything but
/// `anchored` and `reanchored` is a guess or a miss and renders as one.
export type ElementState = "anchored" | "reanchored" | "stale" | "lost" | "unknown";

/// What the design pane last saw for an element thread — the evidence an
/// unplaceable one falls back on.
export interface ElementLastSeen {
	state: ElementState;
	selector?: string;
	rect?: ElementRect;
	stamp?: string;
	seenMs: number;
}

export interface ElementThreadInfo {
	anchor: ElementAnchor;
	lastSeen?: ElementLastSeen;
	state: ElementState;
}

/// Which side of the diff a line thread hangs on. A comment on a
/// deleted line lives in the old text and is re-anchored there.
export type Side = "old" | "new";

/// How a thread re-matched on this refresh. `shifted` and `outdated`
/// are both guesses — see `outdated`, which is true for both.
export type Placement = "unmoved" | "moved" | "shifted" | "outdated" | "element";

export interface ReviewComment {
	id: string;
	threadId: string;
	/** `agent` is written by the review API (#213). */
	authorKind: "user" | "agent";
	/** Harness id when `authorKind === "agent"`. */
	authorId?: string;
	/** The agent's byline — "claude · main". Resolved backend-side from
	 *  the room's harness list; absent once that harness is gone. */
	authorLabel?: string;
	body: string;
	createdMs: number;
	updatedMs: number;
}

/** An agent's claim that a thread is handled (#213). Not a resolution:
 *  the reviewer still closes the thread. */
export interface ReviewAddressed {
	commitSha?: string;
	/** Harness byline, or "agent". */
	by: string;
	note?: string;
	addressedMs: number;
}

export interface ReviewThread {
	id: string;
	scope: ThreadScope;
	filePath?: string;
	commitSha?: string;
	side?: Side;
	/** Where the thread sits now. Absent when it could not be placed. */
	lineStart?: number;
	lineEnd?: number;
	/** The code the comment was written against — always present, and
	 *  what an outdated thread renders instead of moving. */
	anchorLines: string[];
	placement: Placement;
	/** True for `shifted` as well as `outdated`: D6 says a guess
	 *  renders as a guess, never as a fact. */
	outdated: boolean;
	confidence?: number;
	resolvedMs?: number;
	/** Set when an agent has said it handled this (#213). */
	addressed?: ReviewAddressed;
	/** Only on `scope === "element"` (#434). */
	element?: ElementThreadInfo;
	createdMs: number;
	updatedMs: number;
	comments: ReviewComment[];
}

export interface ReviewCommit {
	sha: string;
	shortSha: string;
	summary: string;
	body: string;
	authorName: string;
	timeMs: number;
	isMerge: boolean;
	threadCount: number;
}

export interface ReviewFile {
	path: string;
	name: string;
	/** added | modified | deleted | renamed | … | `unchanged` (#434: an
	 *  entry HTML listed only for its element threads, no diff). */
	change: string;
	additions: number;
	deletions: number;
	binary: boolean;
	/** Digest of the new side — what a viewed marker stores. */
	contentHash: string;
	viewed: boolean;
	/** Marked viewed, but the content moved on since. */
	changedSinceViewed: boolean;
	threadCount: number;
	unresolvedCount: number;
	hasPending: boolean;
	/** Last harness to write it, when Skein knows (D4). */
	harnessId?: string;
}

export interface ReviewScopeData {
	isRepo: boolean;
	baseRef?: string;
	baseResolved: boolean;
	baseSha?: string;
	headBranch?: string;
	headSha?: string;
	commits: ReviewCommit[];
	truncated: boolean;
	files: ReviewFile[];
	additions: number;
	deletions: number;
	pendingCount: number;
	unresolvedCount: number;
	branches: string[];
	threads: ReviewThread[];
	error?: string;
}

export interface ReviewFileDetail {
	path: string;
	name: string;
	change: string;
	binary: boolean;
	blocked?: "binary" | "toolarge" | "symlink" | "unreadable";
	contentHash: string;
	hunks: ReviewHunk[];
	threads: ReviewThread[];
}

/// What opens a new thread. `anchorLines` is sent by the caller rather
/// than re-read on the backend: the anchor has to be the text the user
/// was looking at, and re-reading could capture an agent edit that
/// landed between the click and the submit.
export interface NewThread {
	scope: ThreadScope;
	// `| undefined` throughout: exactOptionalPropertyTypes is on, and
	// every one of these is genuinely absent for some scope — a review
	// thread has no file, a file thread has no line, and so on.
	filePath?: string | undefined;
	commitSha?: string | undefined;
	side?: Side | undefined;
	lineStart?: number | undefined;
	lineEnd?: number | undefined;
	anchorLines: string[];
	body: string;
	/** Required for `scope === "element"`. */
	element?: ElementAnchor | undefined;
}

export const fetchScope = (
	roomId: string,
	cwd: string,
	scope: ReviewScope,
	commitSha?: string,
): Promise<ReviewScopeData> =>
	invoke<ReviewScopeData>("review_scope", {
		roomId,
		cwd,
		scope,
		commitSha: commitSha ?? null,
	});

export const fetchFile = (
	roomId: string,
	cwd: string,
	path: string,
	scope: ReviewScope,
	commitSha?: string,
): Promise<ReviewFileDetail> =>
	invoke<ReviewFileDetail>("review_file", {
		roomId,
		cwd,
		path,
		scope,
		commitSha: commitSha ?? null,
	});

/// Which half of a before/after image pair (#409): the content the diff
/// is *from* vs. the content it is *to*, mirroring `Side` above but
/// named for the image diff rather than a line's gutter.
export type ImageDiffSide = "old" | "new";

/// Raw bytes for one side of an image file in the review pane. Rejects
/// with the same error strings `read_image_bytes` uses, plus
/// `"unavailable"` for a pending-scope baseline whose bytes weren't kept
/// (see `parseImageError` in `../imageFiles.ts`).
export const fetchImageBytes = (
	roomId: string,
	cwd: string,
	path: string,
	scope: ReviewScope,
	side: ImageDiffSide,
	commitSha?: string,
): Promise<ArrayBuffer> =>
	invoke<ArrayBuffer>("review_image_bytes", {
		roomId,
		cwd,
		path,
		scope,
		commitSha: commitSha ?? null,
		side,
	});

/// A thread/comment write touches only sqlite, so nothing else tells the
/// other surfaces showing this room's threads (review pane, design pane).
/// Emit the event the backend emits for an agent's write, same payload.
const written = async <T>(roomId: string, write: Promise<T>): Promise<T> => {
	const out = await write;
	try {
		await emit("skein://review-changed", { roomId });
	} catch (e) {
		console.error("[skein] review-changed emit failed:", e);
	}
	return out;
};

export const addThread = (roomId: string, cwd: string, thread: NewThread): Promise<ReviewThread> =>
	written(roomId, invoke<ReviewThread>("review_add_thread", { roomId, cwd, thread }));

/// What the design pane found when it re-located an element thread.
export interface ElementSeen {
	state: ElementState;
	selector?: string | undefined;
	rect?: ElementRect | undefined;
	/** Source files the page was built from, for the backend's own checks. */
	files: string[];
}

export const reportElementSeen = (
	roomId: string,
	threadId: string,
	seen: ElementSeen,
): Promise<void> => invoke<void>("review_element_seen", { roomId, threadId, seen });

export const fetchElementThreads = (roomId: string, entry: string): Promise<ReviewThread[]> =>
	invoke<ReviewThread[]>("review_element_threads", { roomId, entry });

export const replyToThread = (
	roomId: string,
	threadId: string,
	body: string,
): Promise<ReviewComment> =>
	written(roomId, invoke<ReviewComment>("review_reply", { roomId, threadId, body }));

export const editComment = (roomId: string, commentId: string, body: string): Promise<void> =>
	written(roomId, invoke<void>("review_edit_comment", { commentId, body }));

/// Returns the thread id when deleting the comment emptied the thread.
export const deleteComment = (roomId: string, commentId: string): Promise<string | null> =>
	written(roomId, invoke<string | null>("review_delete_comment", { commentId }));

export const deleteThread = (roomId: string, threadId: string): Promise<void> =>
	written(roomId, invoke<void>("review_delete_thread", { threadId }));

/// Human-only by design (D8): an agent that can resolve its own
/// comments removes the gate the review loop exists to provide.
export const resolveThread = (roomId: string, threadId: string, resolved: boolean): Promise<void> =>
	written(roomId, invoke<void>("review_resolve_thread", { threadId, resolved }));

export const markViewed = (
	roomId: string,
	path: string,
	contentHash: string,
	viewed: boolean,
): Promise<void> => invoke<void>("review_mark_viewed", { roomId, path, contentHash, viewed });

export const setBaseRef = (
	roomId: string,
	cwd: string,
	baseRef: string,
): Promise<ReviewScopeData> => invoke<ReviewScopeData>("review_set_base", { roomId, cwd, baseRef });

// ── derived helpers ───────────────────────────────────────────────

/// Threads that hang on a specific line of the rendered diff, keyed
/// `<side>:<line>` so a hunk row can look up its own in one hit.
export function threadsByLine(threads: ReviewThread[]): Map<string, ReviewThread[]> {
	const map = new Map<string, ReviewThread[]>();
	for (const t of threads) {
		if (t.scope !== "line" || t.lineStart == null) continue;
		// Anchor to the *end* of the range: a comment on lines 10-14
		// belongs under 14, the way GitHub renders a multi-line remark.
		const key = `${t.side ?? "new"}:${t.lineEnd ?? t.lineStart}`;
		const list = map.get(key);
		if (list) list.push(t);
		else map.set(key, [t]);
	}
	return map;
}

/// Threads with no line to sit on — file-scoped ones, element ones (#434,
/// they live on the page, not a line), plus every line thread the matcher
/// could not place. All render above the diff, so an orphaned comment is
/// impossible to miss (D6: never silently drop).
export function unplacedThreads(threads: ReviewThread[]): ReviewThread[] {
	return threads.filter(
		(t) =>
			t.scope === "file" || t.scope === "element" || (t.scope === "line" && t.lineStart == null),
	);
}

export const isUnresolved = (t: ReviewThread): boolean => t.resolvedMs == null;

/// Every harness that owns a file in this list, in list order.
///
/// D4's filter row. Harness is attribution, never scope — all harnesses
/// in a room share one worktree, so there is exactly one diff, and
/// partitioning it per harness would render several partial views each
/// of which is wrong. Filtering the *list* is the safe form of the same
/// question, because nothing about the diff changes underneath it.
export function contributingHarnesses(files: ReviewFile[]): string[] {
	const seen: string[] = [];
	for (const f of files) {
		if (f.harnessId && !seen.includes(f.harnessId)) seen.push(f.harnessId);
	}
	return seen;
}
