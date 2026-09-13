// Pure per-harness tracking of opencode's outstanding permission and
// question prompts (#86). opencode's SSE stream tells us when a
// prompt is asked and when it's answered, but not which activity
// phase that implies on its own — a harness can have more than one
// prompt in flight at once (a subagent child session's prompt blocks
// the user too, per the harness-events contract), so the phase has to
// be derived from set membership rather than the latest event alone.
// Kept separate from `harnessActivity` so that membership logic is
// unit-testable without the store, the event stream, or React.
//
// Permission always outranks a question: a tool waiting on approval
// is a harder stop than a question the agent asked, so as long as any
// permission is outstanding the harness reads as `permission` even if
// a question also landed in the meantime.

/// What `harnessActivity` phase should follow a pending-set mutation.
export type PendingPhase = "permission" | "waiting" | "running";

interface HarnessPending {
	permissions: Set<string>;
	questions: Set<string>;
}

const pending = new Map<string, HarnessPending>();

const ensure = (harnessId: string): HarnessPending => {
	let p = pending.get(harnessId);
	if (!p) {
		p = { permissions: new Set(), questions: new Set() };
		pending.set(harnessId, p);
	}
	return p;
};

/// Derive the phase implied by a harness's current pending sets:
/// `permission` while any is outstanding, else `waiting` while a
/// question is, else `running` — everything drained, resume work.
const resolvePhase = (p: HarnessPending | undefined): PendingPhase => {
	if (p && p.permissions.size > 0) return "permission";
	if (p && p.questions.size > 0) return "waiting";
	return "running";
};

export const pendingPrompts = {
	/// A permission dialog appeared. Always yields `permission`.
	permissionAsked(harnessId: string, requestId: string): PendingPhase {
		ensure(harnessId).permissions.add(requestId);
		return "permission";
	},

	/// The permission dialog was answered. The SSE event doesn't say
	/// approved vs. denied, and it doesn't matter here — either way the
	/// gate this one request held is gone.
	permissionReplied(harnessId: string, requestId: string): PendingPhase {
		pending.get(harnessId)?.permissions.delete(requestId);
		return resolvePhase(pending.get(harnessId));
	},

	/// A non-permission question was asked (opencode's own prompt
	/// primitive, distinct from a tool permission). Outranked by any
	/// permission already pending on this harness.
	questionAsked(harnessId: string, requestId: string): PendingPhase {
		const p = ensure(harnessId);
		p.questions.add(requestId);
		return resolvePhase(p);
	},

	questionResolved(harnessId: string, requestId: string): PendingPhase {
		pending.get(harnessId)?.questions.delete(requestId);
		return resolvePhase(pending.get(harnessId));
	},

	/// Drop every pending id for a harness: `session_idle` (opencode
	/// finished its turn, whatever was in flight is moot),
	/// `session_end` / `connected` (a reconnect replays nothing, per
	/// the harness-events contract), and adapter detach.
	forget(harnessId: string): void {
		pending.delete(harnessId);
	},
};
