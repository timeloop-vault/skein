// #755: `Harness.sessionId` is daemon-owned, so a session id this client has
// just reported (`reportHarnessSession`) reaches rooms state only one
// `skein://rooms-changed` event later. Two decisions would otherwise act on
// the lagging record: the opencode adapter's capture-versus-follow and
// "already on this session" checks (A -> B -> A before B's event lands is
// judged "no change"), and the claimed-ids set that keeps two opencode
// harnesses in one cwd from capturing the same new session. This module holds
// the ids reported and not yet seen in the record, so both reads can look past
// the lag. Module state like harnessInput.ts; no React.
//
// An entry ends in one of three ways: the record catches up (it equals the
// pending id), the report is refused or fails (`changed` false / rejection),
// or the grace expires: once the daemon took the report its rooms-changed
// event is already out, so the entry only lives REPORTED_SESSION_GRACE_MS
// more, in case the record is later changed by someone else. An entry whose
// report has not settled has no expiry. The clock is injectable (`now`).

export const REPORTED_SESSION_GRACE_MS = 2000;

type Now = () => number;

interface Entry {
	sessionId: string;
	expiresAt: number | null;
}

const pending = new Map<string, Entry>();

/** A capture/replace report for `harnessId` was just fired. */
export function noteReported(harnessId: string, sessionId: string): void {
	pending.set(harnessId, { sessionId, expiresAt: null });
}

/** That report settled. Not taken by the daemon (`changed` false, or a
 *  rejection): drop the pending id if it is still the one reported. Taken:
 *  keep it for the grace period. */
export function settleReported(
	harnessId: string,
	sessionId: string,
	changed: boolean,
	now: Now = Date.now,
): void {
	const e = pending.get(harnessId);
	if (e?.sessionId !== sessionId) return;
	if (changed) e.expiresAt = now() + REPORTED_SESSION_GRACE_MS;
	else pending.delete(harnessId);
}

const live = (harnessId: string, now: Now): Entry | undefined => {
	const e = pending.get(harnessId);
	if (e && e.expiresAt !== null && now() >= e.expiresAt) {
		pending.delete(harnessId);
		return undefined;
	}
	return e;
};

/** The session id to decide against: the pending one while it differs from
 *  the record, else the record's (clearing the entry once the record caught up). */
export function effectiveSessionId(
	harnessId: string,
	recordSessionId: string | undefined,
	now: Now = Date.now,
): string | undefined {
	const e = live(harnessId, now);
	if (e === undefined) return recordSessionId;
	if (e.sessionId === recordSessionId) {
		pending.delete(harnessId);
		return recordSessionId;
	}
	return e.sessionId;
}

/** Every unexpired pending id, for the claimed set. */
export function pendingSessionIds(now: Now = Date.now): string[] {
	return [...pending.keys()]
		.map((id) => live(id, now)?.sessionId)
		.filter((s): s is string => s !== undefined);
}

/** Drop whatever is pending for a harness that no longer exists. */
export function forgetReported(harnessId: string): void {
	pending.delete(harnessId);
}
