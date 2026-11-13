import { describe, expect, it } from "vitest";
import { harnessDisplayStatus, statusLabel } from "./harnessActivity.ts";
import type { ActivityPhase, HarnessActivity } from "./harnessActivityTypes.ts";

// #421: a harness with no activity entry must never read as running.

const mkActivity = (
	phase: ActivityPhase,
	over: Partial<HarnessActivity> = {},
): HarnessActivity => ({
	phase,
	lastOutputAt: null,
	spawnedAt: 0,
	hasUserInput: false,
	authoritative: false,
	permissionTool: null,
	permissionAgentType: null,
	questionOpen: false,
	adapterHeard: false,
	adapterSilent: false,
	launchSignalAt: null,
	injected: false,
	delegationDeferredAt: null,
	delegatedCount: 0,
	phaseSince: 0,
	lastTurnSignal: null,
	lastSubmitAt: null,
	...over,
});

describe("harnessDisplayStatus", () => {
	it("reads idle / not started with no activity, never running", () => {
		for (const a of [null, undefined]) {
			expect(harnessDisplayStatus(true, a, 0)).toEqual({ status: "idle", label: "not started" });
			expect(harnessDisplayStatus(true, a, 3, 2, 1)).toEqual({
				status: "idle",
				label: "not started",
			});
		}
	});

	it("reads running for a running activity", () => {
		expect(harnessDisplayStatus(true, mkActivity("running"), 0)).toEqual({
			status: "running",
			label: "running",
		});
	});

	it("downgrades the dot but not the label for an acknowledged waiting harness", () => {
		expect(harnessDisplayStatus(true, mkActivity("waiting"), 0)).toEqual({
			status: "idle",
			label: "waiting",
		});
		expect(harnessDisplayStatus(true, mkActivity("waiting"), 1).status).toBe("waiting");
	});

	it("names the tool for a permission dialog", () => {
		const a = mkActivity("permission", { permissionTool: "Bash" });
		expect(harnessDisplayStatus(true, a, 0)).toEqual({
			status: "permission",
			label: "permission needed · Bash",
		});
	});

	it("reads delegating while subagents work", () => {
		const r = harnessDisplayStatus(true, mkActivity("running"), 0, 2);
		expect(r.status).toBe("running");
		expect(r.label).toContain("delegating");
	});

	it("reads idle / idle with no activity for a non-PTY harness", () => {
		for (const a of [null, undefined]) {
			expect(harnessDisplayStatus(false, a, 0)).toEqual({ status: "idle", label: "idle" });
		}
	});

	it("labels background tasks as statusLabel does", () => {
		const r = harnessDisplayStatus(true, mkActivity("running"), 0, 0, 2);
		expect(r.status).toBe("running");
		expect(r.label).toBe(statusLabel("running", null, null, 0, 2));
	});
});
