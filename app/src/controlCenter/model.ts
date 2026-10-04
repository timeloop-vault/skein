// Control Center model (#492): pure ranking of rooms by how much they
// need the user. No React, no Tauri — the UI builds `RoomSnapshot`s
// from its stores and hands them, with the strip segments, to
// `buildControlCenter`. Grouping is NOT re-derived here: it comes from
// `buildStrip`'s segments.

import type { ActivityPhase } from "../harnessActivityTypes";
import { groupDisplayName, groupRooms, type StripSegment } from "../roomGroups";
import type { HarnessKind, Room, Status } from "../types";

export interface HarnessSnapshot {
	id: string;
	name: string;
	kind: HarnessKind;
	status: ActivityPhase;
	/// The app's display status (`effectiveStatus`, pendingNotifications
	/// applied): "waiting" only while unseen; a seen waiting harness is "idle".
	/// The dot colour and the unseen/seen split come from this, not `status`.
	display: Status;
	/// Display label, e.g. "delegating · 2 agents".
	label: string;
	lastActivityAt: number | null;
	/// phaseSince.
	since: number | null;
	/// Time of the latest transition into running from a non-working phase
	/// (not from permission); null = unknown, which has no effect.
	turnStartedAt: number | null;
}

export interface SignoffSnapshot {
	approved: boolean;
	stale: boolean;
	unresolvedCount: number;
	unaddressedCount: number;
}

export interface RoomSnapshot {
	roomId: string;
	/// Room order; the first is the lead.
	harnesses: HarnessSnapshot[];
	signoff: SignoffSnapshot | null;
	lastStatus: { body: string; createdMs: number } | null;
	branch: string | null;
}

export const ATTENTION_ORDER = ["blocked", "review", "waiting", "working", "idle"] as const;
export type Attention = (typeof ATTENTION_ORDER)[number];

export interface RoomAttention {
	rank: Attention;
	reason: string;
	since: number | null;
	/// The harness that drives the rank: the permission/waiting/running one;
	/// for status-line and sign-off ranks and idle, the lead (first) harness.
	harnessId: string | null;
	/// True only for a waiting rank whose waiting harnesses have all been
	/// looked at: same text, neutral styling, sorted after unseen ones.
	seen: boolean;
}

export interface ParsedStatusLine {
	state: string;
	task: string;
	line: string;
}

export interface CCRow {
	room: Room;
	snap: RoomSnapshot;
	attention: RoomAttention;
}

export interface CCSection {
	key: string;
	label: string;
	isGroup: boolean;
	rank: Attention;
	rows: CCRow[];
}

const STATUS_RE = /^status:\s*(.+?)(?:(?:\s*(?:—|--)\s*|\s+-\s+)(.*))?$/i;

/// Parse a first line of the form `status: <state> — <task>` (also
/// ` - ` and `--` as separators; the task part may be absent). `null` when it is not a status line.
export function parseStatusLine(body: string): ParsedStatusLine | null {
	const line = (body.split(/\r?\n/, 1)[0] ?? "").trim();
	const m = STATUS_RE.exec(line);
	if (!m) {
		return null;
	}
	const state = (m[1] ?? "").trim().toLowerCase();
	if (!state) {
		return null;
	}
	return { state, task: (m[2] ?? "").trim(), line };
}

export function maxOrNull(values: readonly (number | null)[]): number | null {
	let best: number | null = null;
	for (const v of values) {
		if (v !== null && (best === null || v > best)) {
			best = v;
		}
	}
	return best;
}

/// Latest sign of life in a room: each harness's meaningful activity
/// (`lastActivityAt`; NOT `since`, which a shell redraw also resets) and the
/// last status message.
export function roomLastActivity(snap: RoomSnapshot): number | null {
	return maxOrNull([
		...snap.harnesses.map((h) => h.lastActivityAt),
		snap.lastStatus?.createdMs ?? null,
	]);
}

/// A transition that begins a new turn: into running from a non-working
/// phase. permission -> running is a mid-turn approval, not a new turn.
export function isTurnStart(from: ActivityPhase, to: ActivityPhase): boolean {
	return (
		to === "running" &&
		(from === "waiting" || from === "idle" || from === "exited" || from === "spawning")
	);
}

function isBusy(h: HarnessSnapshot): boolean {
	return h.status === "running" || h.status === "spawning";
}

function oldestOf(hs: readonly HarnessSnapshot[]): HarnessSnapshot | null {
	let best: HarnessSnapshot | null = null;
	for (const h of hs) {
		if (h.since !== null && (best === null || (best.since ?? Infinity) > h.since)) {
			best = h;
		}
	}
	return best ?? hs[0] ?? null;
}

function oldest(hs: readonly HarnessSnapshot[]): number | null {
	let best: number | null = null;
	for (const h of hs) {
		if (h.since !== null && (best === null || h.since < best)) {
			best = h.since;
		}
	}
	return best;
}

export function roomAttention(snap: RoomSnapshot): RoomAttention {
	const { harnesses, signoff, lastStatus } = snap;
	const busy = harnesses.some(isBusy);
	const statusMs = lastStatus?.createdMs ?? null;
	// A status line is stale once any harness started a new turn after it.
	const lastTurnStart = maxOrNull(harnesses.map((h) => h.turnStartedAt));
	const superseded = statusMs !== null && lastTurnStart !== null && statusMs < lastTurnStart;
	const status = lastStatus && !superseded ? parseStatusLine(lastStatus.body) : null;

	const leadId = harnesses[0]?.id ?? null;
	const permission = harnesses.filter((h) => h.status === "permission");
	if (permission.length > 0) {
		return {
			rank: "blocked",
			reason: "permission",
			since: oldest(permission),
			harnessId: oldestOf(permission)?.id ?? leadId,
			seen: false,
		};
	}
	if (status?.state === "blocked" && !busy) {
		return {
			rank: "blocked",
			reason: "blocked on you",
			since: statusMs,
			harnessId: leadId,
			seen: false,
		};
	}

	const approvedCurrent = signoff?.approved === true && !signoff.stale;
	if (signoff?.approved && signoff.stale && !busy) {
		return {
			rank: "review",
			reason: "sign-off stale",
			since: statusMs,
			harnessId: leadId,
			seen: false,
		};
	}
	if (status?.state === "review" && !busy && !approvedCurrent) {
		const open = signoff?.unresolvedCount ?? 0;
		const reason = open > 0 ? `ready for review, ${open} open threads` : "ready for review";
		return { rank: "review", reason, since: statusMs, harnessId: leadId, seen: false };
	}

	const waiting = harnesses.filter((h) => h.status === "waiting");
	if (waiting.length > 0) {
		// Unseen (display "waiting") drives the rank when there is one.
		const unseen = waiting.filter((h) => h.display === "waiting");
		const driving = unseen.length > 0 ? unseen : waiting;
		return {
			rank: "waiting",
			reason: "your turn",
			since: oldest(driving),
			harnessId: oldestOf(driving)?.id ?? leadId,
			seen: unseen.length === 0,
		};
	}
	if (busy) {
		const running = harnesses.filter(isBusy);
		return {
			rank: "working",
			reason: "working",
			since: roomLastActivity(snap),
			harnessId: running[0]?.id ?? leadId,
			seen: false,
		};
	}
	return {
		rank: "idle",
		reason: "idle",
		since: roomLastActivity(snap),
		harnessId: leadId,
		seen: false,
	};
}

function emptySnap(roomId: string): RoomSnapshot {
	return { roomId, harnesses: [], signoff: null, lastStatus: null, branch: null };
}

interface Keyed {
	attention: RoomAttention;
	activity: number | null;
	order: number;
}

function compare(a: Keyed, b: Keyed): number {
	const ra = ATTENTION_ORDER.indexOf(a.attention.rank);
	const rb = ATTENTION_ORDER.indexOf(b.attention.rank);
	if (ra !== rb) {
		return ra - rb;
	}
	if (a.attention.rank === "waiting" && a.attention.seen !== b.attention.seen) {
		return a.attention.seen ? 1 : -1;
	}
	const needsYou = a.attention.rank !== "working" && a.attention.rank !== "idle";
	if (needsYou) {
		// Longest-waiting first; unknown times sort after known ones.
		const sa = a.attention.since;
		const sb = b.attention.since;
		if (sa !== sb) {
			if (sa === null) return 1;
			if (sb === null) return -1;
			return sa - sb;
		}
	} else if (a.activity !== b.activity) {
		// Most recent first; no activity last.
		if (a.activity === null) return 1;
		if (b.activity === null) return -1;
		return b.activity - a.activity;
	}
	return a.order - b.order;
}

/// Sections (one per strip segment) of rows ranked by need for attention.
export function buildControlCenter(
	segments: readonly StripSegment[],
	snaps: ReadonlyMap<string, RoomSnapshot>,
): CCSection[] {
	let order = 0;
	const built = segments.map((seg, segIndex) => {
		const rows = groupRooms(seg).map((room) => {
			const snap = snaps.get(room.id) ?? emptySnap(room.id);
			const attention = roomAttention(snap);
			return {
				row: { room, snap, attention } satisfies CCRow,
				activity: roomLastActivity(snap),
				order: order++,
			};
		});
		rows.sort((a, b) =>
			compare({ ...a, attention: a.row.attention }, { ...b, attention: b.row.attention }),
		);
		const best = rows[0];
		const isGroup = seg.kind === "group";
		const section: CCSection = {
			key: seg.kind === "group" ? seg.key : seg.room.id,
			label: seg.kind === "group" ? groupDisplayName(seg) : seg.room.name,
			isGroup,
			rank: best?.row.attention.rank ?? "idle",
			rows: rows.map((r) => r.row),
		};
		return { section, best, segIndex };
	});
	built.sort((a, b) => {
		if (!a.best || !b.best) {
			return a.segIndex - b.segIndex;
		}
		const c = compare(
			{ ...a.best, attention: a.best.row.attention, order: a.segIndex },
			{ ...b.best, attention: b.best.row.attention, order: b.segIndex },
		);
		return c;
	});
	return built.map((b) => b.section);
}
