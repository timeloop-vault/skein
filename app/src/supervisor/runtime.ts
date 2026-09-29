import { logBoth } from "../frontendLog.ts";
import { harnessActivity, onTick } from "../harnessActivity.ts";
import { liveAttach } from "../harnessEvents.ts";
import { subagents } from "../subagents.ts";
import type { SupervisorSnapshot } from "./invariants.ts";
import { mailState } from "./mailFeed.ts";
import {
	type HarnessSupervisorState,
	type ViolationRecord,
	emptyHarnessState,
	evaluate,
	formatEffect,
	formatRecoveryOutcome,
} from "./supervisorState.ts";
import { claudeTranscriptStat } from "./transcriptStat.ts";

/// #423: the runtime around the pure supervisor. Each activity tick it
/// snapshots every harness in the store, runs `evaluate`, logs the
/// effects and applies the two recoveries. It never writes to a PTY and
/// never touches the mail or input seams.

const TARGET = "skein::supervisor";
export const TRANSCRIPT_SAMPLE_MS = 15_000;
const TRANSCRIPT_SAMPLES_CAP = 8;

type Sample = { at: number; size: number; mtimeMs: number };

interface Entry {
	spawnedAt: number;
	state: HarnessSupervisorState;
	sessionId: string | null;
	samples: Sample[];
	lastSampleAt: number;
	inFlight: boolean;
}

const entries = new Map<string, Entry>();
let stop: (() => void) | null = null;

const sampleTranscript = (
	id: string,
	entry: Entry,
	sessionId: string,
	cwd: string,
	now: number,
) => {
	if (entry.inFlight || now - entry.lastSampleAt < TRANSCRIPT_SAMPLE_MS) return;
	entry.inFlight = true;
	entry.lastSampleAt = now;
	claudeTranscriptStat(sessionId, cwd)
		.then((st) => {
			// Dropped or re-pointed while the stat was in flight.
			if (entries.get(id) !== entry || entry.sessionId !== sessionId || st === null) return;
			entry.samples.push({ at: now, size: st.size, mtimeMs: st.mtimeMs });
			if (entry.samples.length > TRANSCRIPT_SAMPLES_CAP) entry.samples.shift();
		})
		.catch((err: unknown) => {
			console.debug(`[skein] supervisor transcript stat failed for ${id}: ${String(err)}`);
		})
		.finally(() => {
			entry.inFlight = false;
		});
};

const tick = (now: number): void => {
	const live = new Set(harnessActivity.ids());
	for (const id of [...entries.keys()]) if (!live.has(id)) entries.delete(id);

	for (const id of live) {
		const a = harnessActivity.get(id);
		if (!a) continue;
		let entry = entries.get(id);
		if (!entry || entry.spawnedAt !== a.spawnedAt) {
			entry = {
				spawnedAt: a.spawnedAt,
				state: emptyHarnessState(),
				sessionId: null,
				samples: [],
				lastSampleAt: Number.NEGATIVE_INFINITY,
				inFlight: false,
			};
			entries.set(id, entry);
		}

		const attach = liveAttach(id);
		let transcript: Sample[] | null = null;
		if (attach?.kind === "claude") {
			if (entry.sessionId !== attach.sessionId) {
				entry.sessionId = attach.sessionId;
				entry.samples = [];
			}
			sampleTranscript(id, entry, attach.sessionId, attach.cwd, now);
			transcript = [...entry.samples];
		} else {
			entry.sessionId = null;
			entry.samples = [];
		}

		const snapshot: SupervisorSnapshot = {
			harnessId: id,
			phase: a.phase,
			phaseSince: a.phaseSince,
			lastOutputAt: a.lastOutputAt,
			authoritative: a.authoritative,
			adapterSilent: a.adapterSilent,
			liveAttach: attach !== null,
			lastAdapterEvent: a.lastAdapterEvent,
			authorityLostAt: a.authorityLostAt,
			lastTurnSignal: a.lastTurnSignal,
			subagentsWorking: subagents.workingCount(id),
			delegationDeferredAt: a.delegationDeferredAt,
			delegationEmptiedAt: a.delegationEmptiedAt,
			permissionAt: a.permissionAt,
			lastSubmitAt: a.lastSubmitAt,
			permissionAgentId: a.permissionAgentId,
			mail: mailState(id),
			transcript,
		};

		const { state, effects } = evaluate(entry.state, snapshot, now);
		entry.state = state;
		for (const effect of effects) {
			const line = formatEffect(id, effect);
			logBoth(line.level, TARGET, line.message);
			if (effect.type !== "recover") continue;
			const applied =
				effect.recovery === "regrant_authority"
					? harnessActivity.supervisorRegrantAuthority(id)
					: harnessActivity.supervisorSetWaiting(id);
			const outcome = formatRecoveryOutcome(id, effect.code, effect.recovery, applied);
			logBoth(outcome.level, TARGET, outcome.message);
		}
	}
};

/// Idempotent; returns the stop function.
export function startSupervisor(): () => void {
	if (stop === null) {
		const off = onTick(tick);
		stop = () => {
			off();
			stop = null;
		};
	}
	return stop;
}

/// Violation history for one harness (newest last), for #414's UI.
export function supervisorViolations(harnessId: string): ViolationRecord[] {
	return entries.get(harnessId)?.state.history ?? [];
}

export function __resetSupervisorForTests(): void {
	stop?.();
	entries.clear();
}
