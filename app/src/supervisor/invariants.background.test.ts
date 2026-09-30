import { describe, expect, it } from "vitest";
import { INVARIANTS, type SupervisorCode, type SupervisorSnapshot } from "./invariants";

const NOW = 1_000_000;

function snap(over: Partial<SupervisorSnapshot> = {}): SupervisorSnapshot {
	return {
		harnessId: "h1",
		phase: "running",
		phaseSince: NOW - 100_000,
		lastOutputAt: NOW - 100_000,
		authoritative: true,
		adapterSilent: false,
		liveAttach: true,
		lastAdapterEvent: null,
		authorityLostAt: null,
		lastTurnSignal: null,
		subagentsWorking: 0,
		backgroundWorking: 0,
		delegationDeferredAt: null,
		delegationEmptiedAt: null,
		permissionAt: null,
		lastSubmitAt: null,
		permissionAgentId: null,
		mail: null,
		transcript: null,
		...over,
	};
}

function fires(code: SupervisorCode, s: SupervisorSnapshot): boolean {
	const inv = INVARIANTS.find((i) => i.code === code);
	if (!inv) throw new Error(`no invariant ${code}`);
	return inv.check(s, NOW).violated;
}

describe("invariants with background tasks working", () => {
	it("ended_turn_not_waiting stays quiet", () => {
		const s = snap({ backgroundWorking: 1, lastTurnSignal: { kind: "end", at: NOW - 20_000 } });
		expect(fires("ended_turn_not_waiting", s)).toBe(false);
	});

	it("deferral_without_work stays quiet", () => {
		const s = snap({ delegationDeferredAt: NOW - 20_000, backgroundWorking: 1 });
		expect(fires("deferral_without_work", s)).toBe(false);
	});
});
