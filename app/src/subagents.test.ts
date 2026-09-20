import { describe, expect, it } from "vitest";
import { subagents } from "./subagents.ts";

// #298 — per-harness live-subagent bookkeeping, kept pure so it's
// testable without the activity store or an event stream.

const nextId = (() => {
	let n = 0;
	return () => `h_${++n}`;
})();

describe("subagents", () => {
	it("record is idempotent and preserves the original startedAt", () => {
		const h = nextId();
		subagents.record(h, { agentId: "a1", agentType: "explore", description: "look around" }, false);
		const first = subagents.live(h);
		expect(first).toHaveLength(1);
		const startedAt = first[0]?.startedAt;

		// A restart replay of the same agentId must not double-count
		// or reset when it started.
		subagents.record(h, { agentId: "a1", agentType: "explore", description: "look around" }, false);
		const second = subagents.live(h);
		expect(second).toHaveLength(1);
		expect(second[0]?.startedAt).toBe(startedAt);
	});

	it("finish removes the entry", () => {
		const h = nextId();
		subagents.record(h, { agentId: "a1", agentType: "explore", description: null }, false);
		subagents.finish(h, "a1");
		expect(subagents.live(h)).toHaveLength(0);
	});

	it("finish for an unknown agentId is a no-op, not a throw", () => {
		const h = nextId();
		expect(() => subagents.finish(h, "does-not-exist")).not.toThrow();
		expect(subagents.live(h)).toHaveLength(0);
	});

	it("forget clears every subagent tracked for the harness", () => {
		const h = nextId();
		subagents.record(h, { agentId: "a1", agentType: "explore", description: null }, false);
		subagents.record(h, { agentId: "a2", agentType: "build", description: null }, false);
		subagents.forget(h);
		expect(subagents.live(h)).toHaveLength(0);
	});

	it("forget for a harness with nothing tracked is a no-op, not a throw", () => {
		const h = nextId();
		expect(() => subagents.forget(h)).not.toThrow();
	});

	it("live returns entries oldest first, stable across insertion order", () => {
		const h = nextId();
		subagents.record(h, { agentId: "a1", agentType: "explore", description: null }, false);
		subagents.record(h, { agentId: "a2", agentType: "build", description: null }, false);
		subagents.record(h, { agentId: "a3", agentType: "plan", description: null }, false);
		const ids = subagents.live(h).map((e) => e.agentId);
		expect(ids).toEqual(["a1", "a2", "a3"]);

		// Re-recording the middle one doesn't reorder it — startedAt is
		// preserved, and order is by startedAt.
		subagents.record(h, { agentId: "a2", agentType: "build", description: null }, false);
		expect(subagents.live(h).map((e) => e.agentId)).toEqual(["a1", "a2", "a3"]);
	});

	it("tracks harnesses independently", () => {
		const a = nextId();
		const b = nextId();
		subagents.record(a, { agentId: "x", agentType: "explore", description: null }, false);
		expect(subagents.live(b)).toHaveLength(0);
		expect(subagents.live(a)).toHaveLength(1);
	});
});

// #277 — `fromAttach` / `workingCount` / `presumeGone`: the counting
// layer the delegation deferral reads, distinct from `live` (which
// stays an honest disk mirror per #276's contract).
describe("fromAttach / workingCount / presumeGone (#277)", () => {
	it("a live start (initial=false) is never fromAttach", () => {
		const h = nextId();
		subagents.record(h, { agentId: "a1", agentType: "explore", description: null }, false);
		expect(subagents.live(h)[0]?.fromAttach).toBe(false);
		expect(subagents.workingCount(h)).toBe(1);
	});

	it("an attach-time start (initial=true) is fromAttach and not counted as working", () => {
		const h = nextId();
		subagents.record(h, { agentId: "a1", agentType: "explore", description: null }, true);
		expect(subagents.live(h)[0]?.fromAttach).toBe(true);
		expect(subagents.workingCount(h)).toBe(0);
		// live() itself stays an honest mirror — the orphan is still
		// reported, only the counting excludes it.
		expect(subagents.live(h)).toHaveLength(1);
	});

	it("an attach-seeded id promoted by a later live start counts as working", () => {
		const h = nextId();
		subagents.record(h, { agentId: "a1", agentType: "explore", description: null }, true);
		expect(subagents.workingCount(h)).toBe(0);

		subagents.record(h, { agentId: "a1", agentType: "explore", description: null }, false);
		expect(subagents.live(h)[0]?.fromAttach).toBe(false);
		expect(subagents.workingCount(h)).toBe(1);
	});

	it("promotion is sticky: a later replay with initial=true does not revert it", () => {
		const h = nextId();
		subagents.record(h, { agentId: "a1", agentType: "explore", description: null }, false);
		subagents.record(h, { agentId: "a1", agentType: "explore", description: null }, true);
		expect(subagents.live(h)[0]?.fromAttach).toBe(false);
		expect(subagents.workingCount(h)).toBe(1);
	});

	it("workingCount is 0 for a harness with nothing tracked", () => {
		expect(subagents.workingCount(nextId())).toBe(0);
	});

	it("presumeGone drops only the working entries and reports the count", () => {
		const h = nextId();
		subagents.record(h, { agentId: "attached", agentType: "explore", description: null }, true);
		subagents.record(h, { agentId: "live1", agentType: "build", description: null }, false);
		subagents.record(h, { agentId: "live2", agentType: "plan", description: null }, false);

		const dropped = subagents.presumeGone(h);
		expect(dropped).toBe(2);
		expect(subagents.workingCount(h)).toBe(0);
		// The attach-only entry was never counted as working, so
		// presumeGone leaves it alone.
		expect(subagents.live(h).map((e) => e.agentId)).toEqual(["attached"]);
	});

	it("presumeGone for a harness with nothing tracked is a no-op, not a throw", () => {
		expect(() => expect(subagents.presumeGone(nextId())).toBe(0)).not.toThrow();
	});
});
