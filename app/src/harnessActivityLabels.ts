// Pure derivation from a `HarnessActivity` (or a room's worth of them) to
// display strings and the `Status` enum — no store, no React. Split out of
// harnessActivity.ts (#19). See harnessActivity.ts for the module this
// belongs to.

import type { HarnessActivity } from "./harnessActivityTypes.ts";
import type { Status } from "./types.ts";

/// Map the internal activity phase onto the existing display
/// `Status` enum used by `StatusDot` and the status bar. Keep
/// the mapping centralized so future strategies (additional L2c
/// adapters, eventual L2b pattern fallback) refine it in one place.
export function activityToStatus(activity: HarnessActivity | null): Status {
	if (!activity) return "running";
	switch (activity.phase) {
		case "spawning":
		case "running":
			return "running";
		case "idle":
			return "idle";
		case "waiting":
			return "waiting";
		case "permission":
			return "permission";
		case "exited":
			return "exited";
	}
}

/// Human-facing text for a `Status`, for the surfaces that print the
/// raw word (bottom status bar, the hover popover). `permission` reads
/// as "permission needed" — never the bare word "permission", which
/// reads as a noun with no verb and doesn't say what's expected of the
/// user — plus the tool name when the adapter could say (#86), plus
/// the subagent name when the dialog belongs to one rather than the
/// main session (#298), e.g. "permission needed · explore · Bash".
/// Both existing shapes (no tool, tool only) are unchanged when
/// `permissionAgentType` is absent or `null`.
///
/// #277: `running` with a non-zero `workingSubagentCount` reads as
/// "delegating · N agents" ("1 agent" singular) instead of the bare
/// "running" — the phase itself doesn't change (the dot stays green
/// via `effectiveStatus`), only the word this function prints. Any
/// other status ignores the count entirely, `permission` included —
/// a harness can't be both blocked on a dialog and shown as
/// delegating.
export function statusLabel(
	status: Status,
	permissionTool?: string | null,
	permissionAgentType?: string | null,
	workingSubagentCount?: number,
): string {
	if (status === "permission") {
		const parts = [permissionAgentType, permissionTool].filter(
			(p): p is string => p !== null && p !== undefined,
		);
		return parts.length > 0 ? `permission needed · ${parts.join(" · ")}` : "permission needed";
	}
	if (status === "running" && workingSubagentCount) {
		return `delegating · ${workingSubagentCount} agent${workingSubagentCount === 1 ? "" : "s"}`;
	}
	return status;
}

/// #277: the notification-wording half of the deferred end-of-turn —
/// how many subagents this harness delegated since the user's last
/// prompt (`HarnessActivity.delegatedCount`), worded for a toast
/// subtitle / OS banner suffix. `null` for zero, so call sites can
/// `if (summary)` rather than check the count themselves. Pure and
/// exported so the wording is unit-testable without going through
/// App.tsx's notification effect.
export function delegationSummary(count: number): string | null {
	if (count <= 0) return null;
	return count === 1 ? "1 delegated agent finished" : `${count} delegated agents finished`;
}

/// `activityToStatus` with the "acknowledged" downgrade applied:
/// when the phase is `waiting` and the harness has zero pending
/// notifications, render as `idle` (grey) instead of `waiting`
/// (blue + pulse). The user has already been to the harness
/// since the last transition; the dot's job there is "telling you
/// what's new," and there's nothing new to tell.
///
/// The bottom-bar TEXT label still uses the underlying phase
/// (`waiting`) so callers can honestly report what Claude is
/// doing; only the visual indicator collapses.
export function effectiveStatus(
	activity: HarnessActivity | null,
	pendingNotifications: number,
): Status {
	const base = activityToStatus(activity);
	if (base === "waiting" && pendingNotifications === 0) return "idle";
	return base;
}

/// Priority order for combining multiple harness statuses into a
/// single room-level status (epic #50 L4). Higher = more important
/// to surface on the room dot. `permission` lands top (#86) — a
/// harness blocked on an approval dialog is a harder stop than one
/// merely waiting for the user's next prompt.
const STATUS_PRIORITY: Record<Status, number> = {
	permission: 6,
	waiting: 5,
	running: 4,
	idle: 3,
	exited: 2,
	error: 1,
};

/// Minimal shape `useRoomActivity` needs from each harness: an id
/// for store lookup + the pending-notifications count for the
/// effective-status downgrade. `pendingNotifications` is explicitly
/// `| undefined` so it accepts the `Harness` type (where the field
/// is optional) under `exactOptionalPropertyTypes`.
export interface RoomHarnessRef {
	id: string;
	pendingNotifications?: number | undefined;
}

/// Pick whichever of two statuses is more important to surface, per
/// `STATUS_PRIORITY`. Exported so the priority ordering itself — e.g.
/// "permission outranks waiting" (#86) — is unit-testable directly,
/// without going through `useRoomActivity`, which needs a React
/// render context this project's node-environment test suite doesn't
/// have. Ties keep `a`, matching the previous inline `>` comparison
/// this replaced (first-seen wins a tie).
export function higherPriorityStatus(a: Status, b: Status): Status {
	return STATUS_PRIORITY[b] > STATUS_PRIORITY[a] ? b : a;
}

/// Combine a room's harnesses into one room-level status. `lookup`
/// is injected (rather than reading the module-level `store`
/// directly) so this is unit-testable without React or the store's
/// global state — see `harnessActivity.test.ts`.
///
/// Returns `"idle"`, never `null`, when no harness in the list has a
/// record yet (e.g. a room made up only of non-PTY harnesses like
/// `files`, or the first paint before any LiveTerminal effect has
/// fired) — issue #290: a room's status derives ONLY from its
/// harnesses, so "nothing has reported in" reads as idle rather than
/// falling back to the room's stale persisted `status` field.
export function aggregateRoomStatus(
	harnesses: readonly RoomHarnessRef[],
	lookup: (id: string) => HarnessActivity | null | undefined,
): Status {
	let best: Status | null = null;
	for (const h of harnesses) {
		const a = lookup(h.id);
		if (!a) continue;
		// Per-harness effective status: waiting downgrades to idle
		// when the harness has been viewed since the last
		// transition. Otherwise a room with one waiting-but-
		// acknowledged harness would keep pulsing the room tab
		// even though the user knows.
		const s = effectiveStatus(a, h.pendingNotifications ?? 0);
		best = best === null ? s : higherPriorityStatus(best, s);
	}
	return best ?? "idle";
}
