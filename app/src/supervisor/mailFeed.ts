/// #423: what `useMailDelivery` knows about a harness's mail, mirrored
/// out of its React refs so the supervisor (which lives outside React)
/// can read it. Write-only from the hook's side; the supervisor only reads.
export interface MailState {
	unread: number;
	lastRefusal: string | null;
	/// Last DELIVERED nudge — kept after the settle window clears.
	lastNudgeAt: number | null;
}

const states = new Map<string, MailState>();

export function noteMailState(harnessId: string, patch: Partial<MailState>): void {
	const cur = states.get(harnessId) ?? { unread: 0, lastRefusal: null, lastNudgeAt: null };
	states.set(harnessId, { ...cur, ...patch });
}

export function mailState(harnessId: string): MailState | null {
	return states.get(harnessId) ?? null;
}

export function forgetMailState(harnessId: string): void {
	states.delete(harnessId);
}
