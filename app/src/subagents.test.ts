import { beforeEach, describe, expect, it } from "vitest";
import { resetPhaseMirror } from "./phaseMirror.ts";
import { mirrorForget, mirrorSet, subagentEntry } from "./phaseTestSupport.ts";
import { subagents } from "./subagents.ts";

// #732: the registry is the daemon's; this is the read side over the mirror.

const H = "h";

beforeEach(() => {
	resetPhaseMirror();
});

describe("subagents (mirror read side)", () => {
	it("is empty for an unknown harness, with a stable empty list", () => {
		expect(subagents.live(H)).toEqual([]);
		expect(subagents.live(H)).toBe(subagents.live("other"));
		expect(subagents.workingCount(H)).toBe(0);
	});

	it("serves the list of the latest update, in the daemon's order", () => {
		mirrorSet(
			H,
			"running",
			{},
			{ subagents: [subagentEntry("a", { startedAt: 1 }), subagentEntry("b")] },
		);
		expect(subagents.live(H).map((s) => s.agentId)).toEqual(["a", "b"]);
		mirrorSet(H, "running", {}, { subagents: [subagentEntry("b")] });
		expect(subagents.live(H).map((s) => s.agentId)).toEqual(["b"]);
		mirrorSet(H, "running");
		expect(subagents.live(H)).toEqual([]);
	});

	it("counts everything but attach-time orphans as working (#277)", () => {
		mirrorSet(
			H,
			"running",
			{},
			{
				subagents: [
					subagentEntry("old", { fromAttach: true }),
					subagentEntry("live"),
					subagentEntry("live2"),
				],
			},
		);
		expect(subagents.live(H)).toHaveLength(3);
		expect(subagents.workingCount(H)).toBe(2);
	});

	it("keeps the list reference while the harness's list is unchanged", () => {
		mirrorSet(H, "running", {}, { subagents: [subagentEntry("a")] });
		const first = subagents.live(H);
		mirrorSet(H, "waiting", {}, { subagents: [subagentEntry("a")] });
		expect(subagents.live(H)).toBe(first);
	});

	it("notifies subscribers on a list change only, and stops after unsubscribe", () => {
		let calls = 0;
		const off = subagents.subscribe(H, () => calls++);
		mirrorSet(H, "running", {}, { subagents: [subagentEntry("a")] });
		expect(calls).toBe(1);
		mirrorSet(H, "waiting", {}, { subagents: [subagentEntry("a")] });
		expect(calls).toBe(1);
		mirrorSet(H, "waiting");
		expect(calls).toBe(2);
		off();
		mirrorSet(H, "waiting", {}, { subagents: [subagentEntry("z")] });
		expect(calls).toBe(2);
	});

	it("drops the list when the daemon forgets the harness", () => {
		mirrorSet(H, "running", {}, { subagents: [subagentEntry("a")] });
		mirrorForget(H);
		expect(subagents.live(H)).toEqual([]);
		expect(subagents.workingCount(H)).toBe(0);
	});
});
