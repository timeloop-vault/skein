import { DELEGATION_SETTLE_MS } from "../harnessActivityConstants";
import type { ActivityPhase as Phase } from "../harnessActivityTypes";

/// Everything an invariant may look at, gathered by the runtime. Pure data:
/// the checks below never touch a store, a timer or the backend.
export interface SupervisorSnapshot {
	harnessId: string;
	phase: Phase;
	/// ms of the last phase change
	phaseSince: number;
	/// last PTY chunk
	lastOutputAt: number | null;
	authoritative: boolean;
	/// a watchdog degraded the adapter
	adapterSilent: boolean;
	/// an adapter attach is live for this harness
	liveAttach: boolean;
	lastAdapterEvent: { at: number; restoresAuthority: boolean } | null;
	authorityLostAt: number | null;
	/// main session only
	lastTurnSignal: { kind: "work" | "end"; at: number } | null;
	subagentsWorking: number;
	backgroundWorking: number;
	delegationDeferredAt: number | null;
	delegationEmptiedAt: number | null;
	permissionAt: number | null;
	/// last user Enter / seam submit
	lastSubmitAt: number | null;
	/// null = main-session dialog
	permissionAgentId: string | null;
	mail: { unread: number; lastRefusal: string | null; lastNudgeAt: number | null } | null;
	/// stat samples of the transcript, oldest first
	transcript: { at: number; size: number; mtimeMs: number }[] | null;
}

export type Verdict = { violated: false } | { violated: true; evidence: string };

export type SupervisorCode =
	| "adapter_without_authority"
	| "ended_turn_not_waiting"
	| "mail_held"
	| "tail_silent_file_growing"
	| "deferral_without_work"
	| "permission_orphaned";

export type Recovery = "regrant_authority" | "set_waiting";

export interface Invariant {
	code: SupervisorCode;
	graceMs: number;
	severity: "warn" | "error";
	recovery: Recovery | null;
	check(s: SupervisorSnapshot, now: number): Verdict;
}

export const ADAPTER_WITHOUT_AUTHORITY_GRACE_MS = 5_000;
export const ENDED_TURN_GRACE_MS = 10_000;
/// Output this recent means someone is typing into a non-authoritative harness.
export const ENDED_TURN_QUIET_MS = 10_000;
export const MAIL_HELD_GRACE_MS = 60_000;
export const TAIL_SILENT_GRACE_MS = 0;
/// How long the adapter must have been silent while the file grows.
export const TAIL_SILENT_MIN_MS = 60_000;
export const DEFERRAL_WITHOUT_WORK_GRACE_MS = 15_000;
export const PERMISSION_ORPHANED_GRACE_MS = 0;
/// A main-session dialog is orphaned once the turn moved on this long ago.
export const PERMISSION_TURN_MOVED_MS = 10_000;
export const PERMISSION_CEILING_MS = 15 * 60_000;

const ok: Verdict = { violated: false };
const fired = (evidence: string): Verdict => ({ violated: true, evidence });

export const INVARIANTS: readonly Invariant[] = [
	{
		code: "adapter_without_authority",
		graceMs: ADAPTER_WITHOUT_AUTHORITY_GRACE_MS,
		severity: "error",
		recovery: "regrant_authority",
		check(s, now) {
			const e = s.lastAdapterEvent;
			if (
				s.liveAttach &&
				!s.authoritative &&
				!s.adapterSilent &&
				s.phase !== "exited" &&
				e?.restoresAuthority &&
				(s.authorityLostAt === null || e.at > s.authorityLostAt)
			) {
				return fired(
					`phase=${s.phase} authoritative=false lastAdapterEvent=${now - e.at}ms authorityLostAt=${
						s.authorityLostAt === null ? "none" : `${now - s.authorityLostAt}ms`
					}`,
				);
			}
			return ok;
		},
	},
	{
		code: "ended_turn_not_waiting",
		graceMs: ENDED_TURN_GRACE_MS,
		severity: "warn",
		recovery: "set_waiting",
		check(s, now) {
			const t = s.lastTurnSignal;
			if (
				t?.kind === "end" &&
				(s.lastSubmitAt === null || t.at > s.lastSubmitAt) &&
				s.subagentsWorking + s.backgroundWorking === 0 &&
				s.delegationDeferredAt === null &&
				(s.phase === "running" || s.phase === "idle") &&
				(s.lastOutputAt === null || now - s.lastOutputAt >= ENDED_TURN_QUIET_MS)
			) {
				return fired(
					`phase=${s.phase} lastTurn=end@${now - t.at}ms authoritative=${s.authoritative} output=${
						s.lastOutputAt === null ? "none" : `${now - s.lastOutputAt}ms`
					}`,
				);
			}
			return ok;
		},
	},
	{
		code: "mail_held",
		graceMs: MAIL_HELD_GRACE_MS,
		severity: "warn",
		recovery: null,
		check(s, now) {
			const m = s.mail;
			if (
				m !== null &&
				m.unread > 0 &&
				s.phase === "waiting" &&
				(m.lastNudgeAt === null || m.lastNudgeAt < s.phaseSince)
			) {
				return fired(
					`unread=${m.unread} waiting=${now - s.phaseSince}ms lastRefusal=${m.lastRefusal ?? "none"}`,
				);
			}
			return ok;
		},
	},
	{
		code: "tail_silent_file_growing",
		graceMs: TAIL_SILENT_GRACE_MS,
		severity: "warn",
		recovery: null,
		check(s) {
			const samples = s.transcript;
			if (!s.liveAttach || samples === null || samples.length < 2) return ok;
			const since = s.lastAdapterEvent?.at ?? Number.NEGATIVE_INFINITY;
			const i0 = samples.findIndex((x) => x.at >= since);
			const s0 = samples[i0];
			if (s0 === undefined) return ok;
			for (let i = samples.length - 1; i > i0; i--) {
				const sn = samples[i];
				if (sn === undefined) continue;
				if ((sn.size > s0.size || sn.mtimeMs > s0.mtimeMs) && sn.at - s0.at >= TAIL_SILENT_MIN_MS) {
					return fired(`grewBytes=${sn.size - s0.size} silentFor=${sn.at - s0.at}ms`);
				}
			}
			return ok;
		},
	},
	{
		code: "deferral_without_work",
		graceMs: DEFERRAL_WITHOUT_WORK_GRACE_MS,
		severity: "warn",
		recovery: null,
		check(s, now) {
			if (
				s.delegationDeferredAt !== null &&
				s.subagentsWorking + s.backgroundWorking === 0 &&
				s.phase !== "permission" &&
				s.phase !== "exited" &&
				(s.delegationEmptiedAt === null || now - s.delegationEmptiedAt >= DELEGATION_SETTLE_MS)
			) {
				return fired(
					`phase=${s.phase} deferred=${now - s.delegationDeferredAt}ms emptied=${
						s.delegationEmptiedAt === null ? "none" : `${now - s.delegationEmptiedAt}ms`
					}`,
				);
			}
			return ok;
		},
	},
	{
		code: "permission_orphaned",
		graceMs: PERMISSION_ORPHANED_GRACE_MS,
		severity: "warn",
		recovery: null,
		check(s, now) {
			if (s.phase !== "permission" || s.permissionAt === null) return ok;
			const t = s.lastTurnSignal;
			if (
				s.permissionAgentId === null &&
				// subagents only: background tasks open no dialogs
				s.subagentsWorking === 0 &&
				t !== null &&
				t.at > s.permissionAt &&
				now - t.at >= PERMISSION_TURN_MOVED_MS
			) {
				return fired(
					`arm=turn_moved_on permission=${now - s.permissionAt}ms lastTurn=${t.kind}@${now - t.at}ms`,
				);
			}
			if (now - s.permissionAt >= PERMISSION_CEILING_MS) {
				return fired(
					`arm=ceiling permission=${now - s.permissionAt}ms agent=${s.permissionAgentId ?? "main"}`,
				);
			}
			return ok;
		},
	},
];
