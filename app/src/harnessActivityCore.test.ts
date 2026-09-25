import { describe, expect, it } from "vitest";
import { phaseSnapshot } from "./harnessActivityCore.ts";
import type { ActivityPhase, HarnessActivity } from "./harnessActivityTypes.ts";

// #356: `phaseSnapshot` backs the `harness_phases` agent-request kind
// (list_harnesses's phase column). Pure and store-free on purpose — a
// bare `Map` stands in for the real module-global store, so this needs
// no `spawned()`/tick machinery the way harnessActivity.test.ts does.

const mkActivity = (phase: ActivityPhase): HarnessActivity => ({
	phase,
	lastOutputAt: null,
	exitCode: null,
	spawnedAt: 0,
	hasUserInput: false,
	authoritative: false,
	tail: "",
	permissionTool: null,
	permissionAgentType: null,
	permissionAgentId: null,
	adapterHeard: false,
	promptSubmittedAt: null,
	adapterSilent: false,
	degradedBy: null,
	launchSignalAt: null,
	injected: false,
	delegationDeferredAt: null,
	delegationActivityAt: 0,
	delegationEmptiedAt: null,
	delegatedCount: 0,
});

describe("phaseSnapshot", () => {
	it("is empty for an empty store", () => {
		expect(phaseSnapshot(new Map())).toEqual({});
	});

	it("maps every harness id to its own phase", () => {
		const store = new Map<string, HarnessActivity>([
			["h1", mkActivity("running")],
			["h2", mkActivity("waiting")],
		]);
		expect(phaseSnapshot(store)).toEqual({ h1: "running", h2: "waiting" });
	});

	it("includes every phase as-is, exited and permission included", () => {
		const store = new Map<string, HarnessActivity>([
			["h1", mkActivity("exited")],
			["h2", mkActivity("permission")],
		]);
		expect(phaseSnapshot(store)).toEqual({ h1: "exited", h2: "permission" });
	});

	it("a harness never spawned in this process is simply absent, not guessed at", () => {
		const store = new Map<string, HarnessActivity>([["h1", mkActivity("idle")]]);
		const result = phaseSnapshot(store);
		expect(Object.keys(result)).toEqual(["h1"]);
		expect(result.h2).toBeUndefined();
	});
});
