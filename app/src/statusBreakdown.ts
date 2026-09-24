// #331: the pure data model behind a room/status-dot's breakdown
// popover — which harnesses (across which rooms) are the reason an
// aggregate dot reads permission/waiting/running, plus which subagents
// are behind a "delegating" one. No store, no DOM, no React: everything
// the aggregate itself needs (`HarnessActivity` lookup, live subagents,
// working count) is injected via `BreakdownDeps`, same seam
// `aggregateRoomStatus` uses for `lookup` — see harnessActivityLabels.ts.

import {
	aggregateRoomStatus,
	effectiveStatus,
	higherPriorityStatus,
	statusLabel,
} from "./harnessActivityLabels.ts";
import type { HarnessActivity } from "./harnessActivityTypes.ts";
import type { SubagentEntry } from "./subagents.ts";
import type { HarnessKind, Status } from "./types.ts";

/// Minimal shape of a harness the breakdown needs — a subset of
/// `Harness` plus the fields the dot itself reads, so this module never
/// imports the full app `Harness` type. `pendingNotifications` is
/// `| undefined` for the same reason as `RoomHarnessRef` in
/// harnessActivityLabels.ts: it accepts `Harness` directly under
/// `exactOptionalPropertyTypes`.
export interface BreakdownHarnessInput {
	id: string;
	kind: HarnessKind;
	name: string;
	pendingNotifications?: number | undefined;
}

/// Minimal shape of a room the breakdown needs.
export interface BreakdownRoomInput {
	id: string;
	name: string;
	harnesses: readonly BreakdownHarnessInput[];
}

/// Everything `buildBreakdown` reads from the live stores, injected
/// rather than read from module-level state — the same reason
/// `aggregateRoomStatus` takes a `lookup` function instead of reaching
/// into `harnessActivity` itself: this module stays testable without a
/// store, React, or a Tauri runtime.
export interface BreakdownDeps {
	activity(id: string): HarnessActivity | null | undefined;
	subagents(id: string): readonly SubagentEntry[];
	workingCount(id: string): number;
}

/// One subagent as shown in the breakdown, trimmed from `SubagentEntry`
/// to what the popover prints. `preRestart` mirrors `fromAttach` — worded
/// for the popover rather than the internal bookkeeping term, since here
/// it's display-facing rather than a counting rule.
export interface BreakdownSubagent {
	agentId: string;
	agentType: string | null;
	description: string | null;
	preRestart: boolean;
}

/// One harness surfaced in the breakdown — a harness whose effective
/// status is `permission`, `waiting` or `running`, i.e. the ones an
/// aggregate dot is actually trying to tell you about. Everything
/// `idle`/`exited`/`error`/unrecorded is quiet instead — see
/// `BreakdownQuietRoom`.
export interface BreakdownRow {
	roomId: string;
	roomName: string;
	harnessId: string;
	kind: HarnessKind;
	harnessName: string;
	status: Status;
	label: string;
	subagents: BreakdownSubagent[];
	hiddenSubagents: number;
}

/// One room's worth of quiet harnesses, grouped for the popover's
/// collapsed summary line rather than listed one row each.
export interface BreakdownQuietRoom {
	roomId: string;
	roomName: string;
	kinds: HarnessKind[];
}

/// The popover's whole payload: the aggregate status the dot itself
/// shows (so the popover can never disagree with the dot it's attached
/// to), the surfaced rows, and everything quiet, grouped per room.
export interface Breakdown {
	status: Status;
	rows: BreakdownRow[];
	moreRows: number;
	quietCount: number;
	quiet: BreakdownQuietRoom[];
}

export const BREAKDOWN_MAX_ROWS = 6;
export const BREAKDOWN_MAX_SUBAGENTS = 3;

/// A status is "surfaced" (gets its own row) rather than folded into
/// the quiet summary — permission/waiting/running are exactly the
/// statuses `activityToStatus` can produce for a harness with a live
/// record; everything else (idle, exited, error, or no record at all)
/// is quiet.
const isSurfaced = (status: Status): boolean =>
	status === "permission" || status === "waiting" || status === "running";

/// Order rows by urgency using the same priority table
/// `aggregateRoomStatus` ranks by, via `higherPriorityStatus` rather
/// than a second copy of `STATUS_PRIORITY`. `Array.prototype.sort` is
/// stable (guaranteed since ES2019 and true in every runtime this
/// project targets), so rows sharing a status keep the input order they
/// were pushed in — room order, then harness order — without this
/// comparator doing anything extra to preserve it.
const compareRowStatus = (a: Status, b: Status): number => {
	if (a === b) return 0;
	return higherPriorityStatus(a, b) === a ? -1 : 1;
};

/// Build the popover payload for one snapshot of rooms. Pure: every
/// live fact comes through `deps`, nothing is read from module state.
///
/// `status` is computed by delegating to `aggregateRoomStatus` over
/// every harness of every room flattened into one list — the exact
/// function (and therefore the exact result) the dot itself calls, so
/// the popover's headline can never read differently than the dot it's
/// attached to.
export function buildBreakdown(
	rooms: readonly BreakdownRoomInput[],
	deps: BreakdownDeps,
	opts?: { maxRows?: number; maxSubagents?: number },
): Breakdown {
	const maxRows = opts?.maxRows ?? BREAKDOWN_MAX_ROWS;
	const maxSubagents = opts?.maxSubagents ?? BREAKDOWN_MAX_SUBAGENTS;

	const allRefs = rooms.flatMap((r) =>
		r.harnesses.map((h) => ({ id: h.id, pendingNotifications: h.pendingNotifications })),
	);
	const status = aggregateRoomStatus(allRefs, deps.activity);

	const rows: BreakdownRow[] = [];
	const quiet: BreakdownQuietRoom[] = [];
	let quietCount = 0;

	for (const room of rooms) {
		const quietKinds: HarnessKind[] = [];
		for (const h of room.harnesses) {
			const a = deps.activity(h.id);
			if (!a) {
				// No activity record at all — same case `aggregateRoomStatus`
				// skips outright rather than letting `activityToStatus(null)`'s
				// "running" default leak in. Always quiet.
				quietCount++;
				quietKinds.push(h.kind);
				continue;
			}
			const s = effectiveStatus(a, h.pendingNotifications ?? 0);
			if (!isSurfaced(s)) {
				quietCount++;
				quietKinds.push(h.kind);
				continue;
			}
			const liveSubagents = deps.subagents(h.id);
			const shown = liveSubagents.slice(0, maxSubagents).map(
				(e): BreakdownSubagent => ({
					agentId: e.agentId,
					agentType: e.agentType,
					description: e.description,
					preRestart: e.fromAttach,
				}),
			);
			rows.push({
				roomId: room.id,
				roomName: room.name,
				harnessId: h.id,
				kind: h.kind,
				harnessName: h.name,
				status: s,
				label: statusLabel(s, a.permissionTool, a.permissionAgentType, deps.workingCount(h.id)),
				subagents: shown,
				hiddenSubagents: Math.max(0, liveSubagents.length - maxSubagents),
			});
		}
		if (quietKinds.length > 0) {
			quiet.push({ roomId: room.id, roomName: room.name, kinds: quietKinds });
		}
	}

	rows.sort((a, b) => compareRowStatus(a.status, b.status));
	const capped = rows.length > maxRows ? rows.slice(0, maxRows) : rows;

	return {
		status,
		rows: capped,
		moreRows: rows.length - capped.length,
		quietCount,
		quiet,
	};
}
