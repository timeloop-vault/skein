import { describe, expect, it, vi } from "vitest";
import { harnessActivity, TRANSITION_SOURCE } from "./harnessActivity.ts";
import { classifyTransition, isNotifiable } from "./harnessNotifyLogic.ts";

// #731 — a re-attach to a running process must not leave a non-authoritative
// kind on `spawning` until its next output.

describe("reattached", () => {
	it("moves a non-authoritative harness from spawning to idle, with a non-notifying source", () => {
		vi.useFakeTimers();
		const seen: Array<[string, string, string?]> = [];
		const off = harnessActivity.subscribeTransitions((_id, from, to, source) =>
			seen.push([from, to, source]),
		);
		harnessActivity.spawned("ra1");
		harnessActivity.reattached("ra1");
		expect(harnessActivity.get("ra1")?.phase).toBe("idle");
		expect(seen).toEqual([["spawning", "idle", TRANSITION_SOURCE.Reattached]]);
		expect(isNotifiable(classifyTransition("spawning", "idle"), TRANSITION_SOURCE.Reattached)).toBe(
			false,
		);
		off();
		harnessActivity.forget("ra1");
		vi.useRealTimers();
	});

	it("leaves an authoritative harness to its adapter", () => {
		vi.useFakeTimers();
		harnessActivity.spawned("ra2");
		harnessActivity.attachAuthoritativeSource("ra2");
		harnessActivity.reattached("ra2");
		expect(harnessActivity.get("ra2")?.phase).toBe("spawning");
		harnessActivity.forget("ra2");
		vi.useRealTimers();
	});

	it("does not touch a harness that already left spawning", () => {
		vi.useFakeTimers();
		harnessActivity.spawned("ra3");
		harnessActivity.recordOutput("ra3", "x\n");
		harnessActivity.reattached("ra3");
		expect(harnessActivity.get("ra3")?.phase).toBe("running");
		harnessActivity.forget("ra3");
		vi.useRealTimers();
	});
});
