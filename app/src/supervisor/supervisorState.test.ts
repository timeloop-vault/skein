import { describe, expect, it } from "vitest";
import type { Invariant, SupervisorSnapshot } from "./invariants";
import {
	type Effect,
	emptyHarnessState,
	evaluate,
	formatEffect,
	formatRecoveryOutcome,
	type HarnessSupervisorState,
	HISTORY_CAP,
	RECOVERY_CAP,
	RECOVERY_MIN_INTERVAL_MS,
	RECOVERY_WINDOW_MS,
} from "./supervisorState";

const snapshot = {} as SupervisorSnapshot;

function makeInv(over: Partial<Invariant> = {}): { inv: Invariant; on: { v: boolean } } {
	const on = { v: false };
	const inv: Invariant = {
		code: "ended_turn_not_waiting",
		graceMs: 10_000,
		severity: "warn",
		recovery: "set_waiting",
		check: () => (on.v ? { violated: true, evidence: "k=v" } : { violated: false }),
		...over,
	};
	return { inv, on };
}

function run(state: HarnessSupervisorState, inv: Invariant, now: number) {
	return evaluate(state, snapshot, now, [inv]);
}

const types = (e: Effect[]) => e.map((x) => x.type);

describe("evaluate", () => {
	it("respects the grace period and reports once per onset", () => {
		const { inv, on } = makeInv({ recovery: null });
		on.v = true;
		let r = run(emptyHarnessState(), inv, 0);
		expect(r.effects).toEqual([]);
		r = run(r.state, inv, 9_999);
		expect(r.effects).toEqual([]);
		r = run(r.state, inv, 10_000);
		expect(r.effects).toEqual([
			{ type: "violation", code: "ended_turn_not_waiting", severity: "warn", evidence: "k=v" },
		]);
		r = run(r.state, inv, 20_000);
		expect(r.effects).toEqual([]);
	});

	it("emits cleared and closes the history entry", () => {
		const { inv, on } = makeInv({ recovery: null, graceMs: 0 });
		on.v = true;
		let r = run(emptyHarnessState(), inv, 100);
		on.v = false;
		r = run(r.state, inv, 600);
		expect(r.effects).toEqual([{ type: "cleared", code: "ended_turn_not_waiting", afterMs: 500 }]);
		expect(r.state.history).toEqual([
			{
				code: "ended_turn_not_waiting",
				severity: "warn",
				since: 100,
				evidence: "k=v",
				clearedAt: 600,
			},
		]);
	});

	it("a clear before the grace ends emits nothing and restarts the clock", () => {
		const { inv, on } = makeInv({ recovery: null });
		on.v = true;
		let r = run(emptyHarnessState(), inv, 0);
		on.v = false;
		r = run(r.state, inv, 5_000);
		expect(r.effects).toEqual([]);
		on.v = true;
		r = run(r.state, inv, 6_000);
		r = run(r.state, inv, 15_999);
		expect(r.effects).toEqual([]);
		r = run(r.state, inv, 16_000);
		expect(types(r.effects)).toEqual(["violation"]);
	});

	it("rate limits recovery and caps it with exactly one exhausted", () => {
		const { inv, on } = makeInv({ graceMs: 0 });
		on.v = true;
		const all: Effect[] = [];
		let state = emptyHarnessState();
		let now = 0;
		for (let i = 0; i < 12; i++) {
			const r = run(state, inv, now);
			state = r.state;
			all.push(...r.effects);
			now += RECOVERY_MIN_INTERVAL_MS / 2;
		}
		expect(types(all)).toEqual([
			"violation",
			"recover",
			"recover",
			"recover",
			"recovery_exhausted",
		]);
		const ex = all[all.length - 1];
		expect(ex).toEqual({
			type: "recovery_exhausted",
			code: "ended_turn_not_waiting",
			attempts: RECOVERY_CAP,
		});
		expect(state.codes.ended_turn_not_waiting?.attempts).toEqual([0, 30_000, 60_000]);
	});

	it("attempts persist across a clear and re-onset", () => {
		const { inv, on } = makeInv({ graceMs: 0 });
		let state = emptyHarnessState();
		const all: Effect[] = [];
		for (let i = 0; i < 5; i++) {
			on.v = true;
			let r = run(state, inv, i * 100_000);
			all.push(...r.effects);
			on.v = false;
			r = run(r.state, inv, i * 100_000 + 1_000);
			all.push(...r.effects);
			state = r.state;
		}
		expect(all.filter((e) => e.type === "recover")).toHaveLength(RECOVERY_CAP);
		expect(all.filter((e) => e.type === "recovery_exhausted")).toHaveLength(1);
		expect(all.filter((e) => e.type === "violation")).toHaveLength(5);
	});

	it("forgets attempts after the window, so a later recurrence recovers again", () => {
		const { inv, on } = makeInv({ graceMs: 0 });
		on.v = true;
		let state = emptyHarnessState();
		const all: Effect[] = [];
		for (let i = 0; i < 6; i++) {
			const r = run(state, inv, i * RECOVERY_MIN_INTERVAL_MS);
			state = r.state;
			all.push(...r.effects);
		}
		expect(all.filter((e) => e.type === "recovery_exhausted")).toHaveLength(1);
		const r = run(state, inv, 6 * RECOVERY_MIN_INTERVAL_MS + RECOVERY_WINDOW_MS);
		expect(types(r.effects)).toEqual(["recover"]);
		expect(r.state.codes.ended_turn_not_waiting?.exhausted).toBe(false);
	});

	it("caps history at 50, dropping the oldest", () => {
		const { inv, on } = makeInv({ graceMs: 0, recovery: null });
		let state = emptyHarnessState();
		for (let i = 0; i < HISTORY_CAP + 10; i++) {
			on.v = true;
			state = run(state, inv, i * 10).state;
			on.v = false;
			state = run(state, inv, i * 10 + 5).state;
		}
		expect(state.history).toHaveLength(HISTORY_CAP);
		expect(state.history[0]?.since).toBe(10 * 10);
	});

	it("does not mutate its input", () => {
		const { inv, on } = makeInv({ graceMs: 0 });
		on.v = true;
		const first = run(emptyHarnessState(), inv, 0);
		const frozen = structuredClone(first.state);
		on.v = false;
		run(first.state, inv, 1_000);
		on.v = true;
		run(first.state, inv, 40_000);
		expect(first.state).toEqual(frozen);
	});
});

describe("formatting", () => {
	it.each<[Effect, "info" | "warn" | "error", string]>([
		[
			{ type: "violation", code: "mail_held", severity: "warn", evidence: "unread=1" },
			"warn",
			"[skein] supervisor harness=h1 code=mail_held violation evidence=unread=1",
		],
		[
			{ type: "violation", code: "adapter_without_authority", severity: "error", evidence: "x=1" },
			"error",
			"[skein] supervisor harness=h1 code=adapter_without_authority violation evidence=x=1",
		],
		[
			{ type: "cleared", code: "mail_held", afterMs: 5 },
			"info",
			"[skein] supervisor harness=h1 code=mail_held cleared afterMs=5",
		],
		[
			{ type: "recover", code: "ended_turn_not_waiting", recovery: "set_waiting" },
			"info",
			"[skein] supervisor harness=h1 code=ended_turn_not_waiting recover recovery=set_waiting",
		],
		[
			{ type: "recovery_exhausted", code: "ended_turn_not_waiting", attempts: 3 },
			"error",
			"[skein] supervisor harness=h1 code=ended_turn_not_waiting recovery_exhausted attempts=3",
		],
	])("formatEffect %#", (effect, level, message) => {
		expect(formatEffect("h1", effect)).toEqual({ level, message });
	});

	it("formatRecoveryOutcome", () => {
		expect(formatRecoveryOutcome("h1", "mail_held", "set_waiting", true).message).toContain(
			"applied=true",
		);
		expect(formatRecoveryOutcome("h1", "mail_held", "set_waiting", false).level).toBe("warn");
	});
});
