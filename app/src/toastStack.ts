// The toast stack's pure model (#180). No React, no DOM, so the
// "expiry is fixed at entry, never restarted by a re-render or a
// coalesce" rule is testable without a component.

import type { HarnessKind } from "./types.ts";

export interface ToastEntry {
	id: string;
	roomId: string;
	harnessId: string;
	kind: HarnessKind;
	roomName: string;
	harnessName: string;
	// "waiting" lands here once L2c-1 (Claude JSONL adapter) reports
	// a `last-prompt` row → harness is awaiting user input. Rendered
	// verbatim in the toast subtitle. "error" is the D2f api_error
	// variant — red treatment plus the dim `detail` line. "permission"
	// (#86) is a harder stop than "waiting" — the harness is blocked on
	// an approval dialog, not merely at end-of-turn. "created" (#330) is
	// the receipt for a room an agent's `create_room` call opened in the
	// background — not a harness-activity transition at all, so it never
	// comes from `harnessActivity.subscribeTransitions` the way every
	// other variant does; `useAgentRequests.ts` pushes it directly.
	// "info" (#410) is the same kind of direct push, for a message
	// that doesn't reduce to a phase word — see `message` below.
	state: "idle" | "exited" | "waiting" | "error" | "permission" | "created" | "info";
	/** Error variant only: summary under the subtitle, e.g.
	 *  "Overloaded (529), retrying · attempt 4 of 10 · retry in 4.4s". */
	detail?: string | undefined;
	/** Permission variant only: the tool name when the adapter could
	 *  say (opencode's permission-asked event carries none). */
	tool?: string | undefined;
	/** Permission variant only: the subagent name when the dialog
	 *  belongs to one rather than the main session (#298). */
	agentType?: string | undefined;
	/** Waiting variant only (#277): `delegationSummary` of
	 *  `HarnessActivity.delegatedCount`, when the harness delegated
	 *  work since the user's last prompt and this end-of-turn wasn't
	 *  flushed by the ceiling (see the `DelegationCeiling` check at the
	 *  call site) — "Skein cannot claim the delegated work finished"
	 *  there, so no suffix rides along. */
	delegationNote?: string | undefined;
	/** "created" variant only: the room that asked for this one, so the
	 *  toast can read "<requesterRoomName> opened <roomName>" — `roomName`
	 *  here is the NEW room, the toast's own click/navigation target, same
	 *  as every other variant. */
	requesterRoomName?: string | undefined;
	/** "info" variant only (#410): the full subtitle text, since a
	 *  one-off notice like a "Reattach telemetry" outcome doesn't
	 *  reduce to a phase word the way every other variant's `state`
	 *  does. */
	message?: string | undefined;
	/** Absolute ms timestamp (Date.now()-comparable) the toast's
	 *  auto-dismiss timer expires at, fixed the moment it entered the
	 *  stack (`appendToast`). NEVER moved by a coalesce (#180) — a
	 *  toast's lifetime is measured from when it first appeared, not
	 *  from the App's latest re-render or its latest update in place. */
	expiresAt: number;
}

/** Everything a caller supplies for a brand-new toast — `expiresAt` is
 *  stamped by `appendToast`, never picked by the caller. */
export type NewToast = Omit<ToastEntry, "expiresAt">;

export const TOAST_DISMISS_MS = 6_000;
export const TOAST_MAX_VISIBLE = 5;

/// Append a new toast, stamping its `expiresAt` from `now`, and cap the
/// stack at `TOAST_MAX_VISIBLE` — dropping the OLDEST entries first, same
/// as the plain `.slice(-N)` this replaces. Existing entries are returned
/// unchanged (same object references) other than the truncation, so a
/// later append never touches an earlier toast's `expiresAt` (#180).
export function appendToast(prev: ToastEntry[], entry: NewToast, now: number): ToastEntry[] {
	const stamped: ToastEntry = { ...entry, expiresAt: now + TOAST_DISMISS_MS };
	return [...prev, stamped].slice(-TOAST_MAX_VISIBLE);
}

/// Update the toast at `index` in place with `patch`, keeping its `id`
/// and `expiresAt` — a coalesce (e.g. a retry burst refreshing an
/// api_error toast's `detail`) must not extend the toast's life (#180).
export function coalesceToast(
	prev: ToastEntry[],
	index: number,
	patch: Partial<Omit<ToastEntry, "id" | "expiresAt">>,
): ToastEntry[] {
	const existing = prev[index];
	if (!existing) return prev;
	const next = [...prev];
	next[index] = { ...existing, ...patch, id: existing.id, expiresAt: existing.expiresAt };
	return next;
}

/// Milliseconds remaining until `expiresAt`, clamped so a toast whose
/// expiry has already passed (a burst of renders after the timer should
/// have fired) never yields a negative `setTimeout` delay.
export function toastRemainingMs(expiresAt: number, now: number): number {
	return Math.max(0, expiresAt - now);
}
