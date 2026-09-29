import {
	INVARIANTS,
	type Invariant,
	type Recovery,
	type SupervisorCode,
	type SupervisorSnapshot,
} from "./invariants";

export const RECOVERY_MIN_INTERVAL_MS = 30_000;
export const RECOVERY_CAP = 3;
export const HISTORY_CAP = 50;
/// Attempts older than this stop counting toward the cap.
export const RECOVERY_WINDOW_MS = 60 * 60_000;

export interface CodeState {
	firstSeenAt: number | null;
	/// reported as violated
	active: boolean;
	/// attempt timestamps within RECOVERY_WINDOW_MS; never reset by a clear, so flapping still hits the cap
	attempts: number[];
	exhausted: boolean;
}

export interface ViolationRecord {
	code: SupervisorCode;
	severity: "warn" | "error";
	since: number;
	evidence: string;
	clearedAt: number | null;
}

export interface HarnessSupervisorState {
	codes: Partial<Record<SupervisorCode, CodeState>>;
	history: ViolationRecord[];
}

export type Effect =
	| { type: "violation"; code: SupervisorCode; severity: "warn" | "error"; evidence: string }
	| { type: "cleared"; code: SupervisorCode; afterMs: number }
	| { type: "recover"; code: SupervisorCode; recovery: Recovery }
	| { type: "recovery_exhausted"; code: SupervisorCode; attempts: number };

export function emptyHarnessState(): HarnessSupervisorState {
	return { codes: {}, history: [] };
}

const freshCode = (): CodeState => ({
	firstSeenAt: null,
	active: false,
	attempts: [],
	exhausted: false,
});

/// Advance one harness's supervisor state by one snapshot. Pure: returns a new
/// state and the effects the runtime should carry out.
export function evaluate(
	state: HarnessSupervisorState,
	snapshot: SupervisorSnapshot,
	now: number,
	invariants: readonly Invariant[] = INVARIANTS,
): { state: HarnessSupervisorState; effects: Effect[] } {
	const codes: Partial<Record<SupervisorCode, CodeState>> = { ...state.codes };
	let history = state.history.map((h) => ({ ...h }));
	const effects: Effect[] = [];

	for (const inv of invariants) {
		const prev = codes[inv.code] ?? freshCode();
		const cs: CodeState = { ...prev, attempts: [...prev.attempts] };
		codes[inv.code] = cs;
		cs.attempts = cs.attempts.filter((t) => now - t < RECOVERY_WINDOW_MS);
		if (cs.attempts.length < RECOVERY_CAP) cs.exhausted = false;
		const verdict = inv.check(snapshot, now);

		if (!verdict.violated) {
			if (cs.active) {
				const since = cs.firstSeenAt ?? now;
				effects.push({ type: "cleared", code: inv.code, afterMs: now - since });
				const open = [...history]
					.reverse()
					.find((h) => h.code === inv.code && h.clearedAt === null);
				if (open) open.clearedAt = now;
			}
			cs.firstSeenAt = null;
			cs.active = false;
			continue;
		}

		cs.firstSeenAt ??= now;
		if (!cs.active) {
			if (now - cs.firstSeenAt < inv.graceMs) continue;
			cs.active = true;
			effects.push({
				type: "violation",
				code: inv.code,
				severity: inv.severity,
				evidence: verdict.evidence,
			});
			history.push({
				code: inv.code,
				severity: inv.severity,
				since: cs.firstSeenAt,
				evidence: verdict.evidence,
				clearedAt: null,
			});
			if (history.length > HISTORY_CAP) history = history.slice(history.length - HISTORY_CAP);
		}

		if (inv.recovery !== null && !cs.exhausted) {
			const last = cs.attempts[cs.attempts.length - 1];
			const due = last === undefined || now - last >= RECOVERY_MIN_INTERVAL_MS;
			if (due) {
				if (cs.attempts.length >= RECOVERY_CAP) {
					cs.exhausted = true;
					effects.push({
						type: "recovery_exhausted",
						code: inv.code,
						attempts: cs.attempts.length,
					});
				} else {
					cs.attempts.push(now);
					effects.push({ type: "recover", code: inv.code, recovery: inv.recovery });
				}
			}
		}
	}

	return { state: { codes, history }, effects };
}

export function formatEffect(
	harnessId: string,
	effect: Effect,
): { level: "info" | "warn" | "error"; message: string } {
	const head = `[skein] supervisor harness=${harnessId} code=${effect.code}`;
	switch (effect.type) {
		case "violation":
			return {
				level: effect.severity,
				message: `${head} violation evidence=${effect.evidence}`,
			};
		case "cleared":
			return { level: "info", message: `${head} cleared afterMs=${effect.afterMs}` };
		case "recover":
			return { level: "info", message: `${head} recover recovery=${effect.recovery}` };
		case "recovery_exhausted":
			return {
				level: "error",
				message: `${head} recovery_exhausted attempts=${effect.attempts}`,
			};
	}
}

export function formatRecoveryOutcome(
	harnessId: string,
	code: SupervisorCode,
	recovery: Recovery,
	applied: boolean,
): { level: "info" | "warn"; message: string } {
	return {
		level: applied ? "info" : "warn",
		message: `[skein] supervisor harness=${harnessId} code=${code} recovery_outcome recovery=${recovery} applied=${applied}`,
	};
}
