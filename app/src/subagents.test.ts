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
		subagents.record(h, { agentId: "a1", agentType: "explore", description: "look around" });
		const first = subagents.live(h);
		expect(first).toHaveLength(1);
		const startedAt = first[0]?.startedAt;

		// A restart replay of the same agentId must not double-count
		// or reset when it started.
		subagents.record(h, { agentId: "a1", agentType: "explore", description: "look around" });
		const second = subagents.live(h);
		expect(second).toHaveLength(1);
		expect(second[0]?.startedAt).toBe(startedAt);
	});

	it("finish removes the entry", () => {
		const h = nextId();
		subagents.record(h, { agentId: "a1", agentType: "explore", description: null });
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
		subagents.record(h, { agentId: "a1", agentType: "explore", description: null });
		subagents.record(h, { agentId: "a2", agentType: "build", description: null });
		subagents.forget(h);
		expect(subagents.live(h)).toHaveLength(0);
	});

	it("forget for a harness with nothing tracked is a no-op, not a throw", () => {
		const h = nextId();
		expect(() => subagents.forget(h)).not.toThrow();
	});

	it("live returns entries oldest first, stable across insertion order", () => {
		const h = nextId();
		subagents.record(h, { agentId: "a1", agentType: "explore", description: null });
		subagents.record(h, { agentId: "a2", agentType: "build", description: null });
		subagents.record(h, { agentId: "a3", agentType: "plan", description: null });
		const ids = subagents.live(h).map((e) => e.agentId);
		expect(ids).toEqual(["a1", "a2", "a3"]);

		// Re-recording the middle one doesn't reorder it — startedAt is
		// preserved, and order is by startedAt.
		subagents.record(h, { agentId: "a2", agentType: "build", description: null });
		expect(subagents.live(h).map((e) => e.agentId)).toEqual(["a1", "a2", "a3"]);
	});

	it("tracks harnesses independently", () => {
		const a = nextId();
		const b = nextId();
		subagents.record(a, { agentId: "x", agentType: "explore", description: null });
		expect(subagents.live(b)).toHaveLength(0);
		expect(subagents.live(a)).toHaveLength(1);
	});
});
