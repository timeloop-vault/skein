import { describe, expect, it } from "vitest";
import {
	activityToStatus,
	aggregateRoomStatus,
	atSafeStoppingPoint,
	delegationSummary,
	effectiveStatus,
	higherPriorityStatus,
	statusLabel,
} from "./harnessActivityLabels.ts";
import { mkActivity } from "./phaseTestSupport.ts";

describe("activityToStatus / statusLabel", () => {
	it("maps the permission phase to the permission status", () => {
		expect(activityToStatus(mkActivity("permission", { permissionTool: "Bash" }))).toBe(
			"permission",
		);
	});

	it("reads as 'permission needed' plus the tool when known", () => {
		expect(statusLabel("permission", "Bash")).toBe("permission needed · Bash");
		expect(statusLabel("permission", null)).toBe("permission needed");
		expect(statusLabel("permission")).toBe("permission needed");
	});

	it("names the subagent when the dialog belongs to one (#298)", () => {
		expect(statusLabel("permission", "Bash", "explore")).toBe("permission needed · explore · Bash");
		expect(statusLabel("permission", null, "explore")).toBe("permission needed · explore");
		expect(statusLabel("permission", "Bash", null)).toBe("permission needed · Bash");
		expect(statusLabel("permission", "Bash", undefined)).toBe("permission needed · Bash");
	});

	it("passes other statuses through unchanged", () => {
		expect(statusLabel("waiting")).toBe("waiting");
		expect(statusLabel("running")).toBe("running");
		expect(statusLabel("running", null, null, 0, 1)).toBe("background · 1 task");
		expect(statusLabel("running", null, null, 0, 3)).toBe("background · 3 tasks");
		expect(statusLabel("running", null, null, 2, 1)).toBe("delegating · 2 agents, 1 task");
		expect(statusLabel("waiting", null, null, 2, 1)).toBe("waiting");
		expect(statusLabel("permission", null, null, 2, 1)).toBe("permission needed");
	});

	it("reads 'delegating · N agents' for a running harness with working subagents (#277)", () => {
		expect(statusLabel("running", null, null, 2)).toBe("delegating · 2 agents");
		expect(statusLabel("running", null, null, 1)).toBe("delegating · 1 agent");
		expect(statusLabel("running", null, null, 0)).toBe("running");
		expect(statusLabel("running")).toBe("running");
	});

	it("ignores the working-subagent count for any status but running", () => {
		expect(statusLabel("waiting", null, null, 3)).toBe("waiting");
		expect(statusLabel("permission", null, null, 3)).toBe("permission needed");
		expect(statusLabel("permission", "Bash", null, 3)).toBe("permission needed · Bash");
		expect(statusLabel("permission", "Bash", "explore", 3)).toBe(
			"permission needed · explore · Bash",
		);
	});
});

describe("delegationSummary", () => {
	it("is null for zero", () => {
		expect(delegationSummary(0)).toBeNull();
	});

	it("is singular for one, plural otherwise", () => {
		expect(delegationSummary(1)).toBe("1 delegated agent finished");
		expect(delegationSummary(3)).toBe("3 delegated agents finished");
	});
});

describe("effectiveStatus", () => {
	it("does not downgrade permission when acknowledged", () => {
		const a = mkActivity("permission", { permissionTool: "Bash" });
		expect(effectiveStatus(a, 0)).toBe("permission");
		expect(effectiveStatus(a, 1)).toBe("permission");
	});

	it("still downgrades acknowledged waiting to idle", () => {
		expect(effectiveStatus(mkActivity("waiting"), 0)).toBe("idle");
	});
});

describe("higherPriorityStatus (room aggregate ranking)", () => {
	it("ranks permission above waiting", () => {
		expect(higherPriorityStatus("waiting", "permission")).toBe("permission");
		expect(higherPriorityStatus("permission", "waiting")).toBe("permission");
	});

	it("ranks waiting above running, idle and exited", () => {
		expect(higherPriorityStatus("waiting", "running")).toBe("waiting");
		expect(higherPriorityStatus("idle", "waiting")).toBe("waiting");
		expect(higherPriorityStatus("exited", "waiting")).toBe("waiting");
	});
});

describe("aggregateRoomStatus (#290)", () => {
	it("returns idle for a room with no harnesses", () => {
		expect(aggregateRoomStatus([], () => null)).toBe("idle");
	});

	it("returns idle when no harness in the room has an activity record", () => {
		const refs = [{ id: "h_files" }, { id: "h_other" }];
		expect(aggregateRoomStatus(refs, () => undefined)).toBe("idle");
	});

	it("aggregates across a mix of recorded and unrecorded harnesses", () => {
		const records = new Map([["a", mkActivity("running")]]);
		const refs = [{ id: "h_files" }, { id: "a" }];
		expect(aggregateRoomStatus(refs, (id) => records.get(id))).toBe("running");
	});

	it("downgrades waiting to idle with no pending notifications", () => {
		const records = new Map([["a", mkActivity("waiting")]]);
		const refs = [{ id: "a", pendingNotifications: 0 }];
		expect(aggregateRoomStatus(refs, (id) => records.get(id))).toBe("idle");
	});

	it("keeps waiting when notifications are pending", () => {
		const records = new Map([["a", mkActivity("waiting")]]);
		const refs = [{ id: "a", pendingNotifications: 1 }];
		expect(aggregateRoomStatus(refs, (id) => records.get(id))).toBe("waiting");
	});
});

// #381: the pure predicate behind the #238 seam treating a #277
// delegation-deferred `running` the same as `waiting`.
describe("atSafeStoppingPoint", () => {
	it("waiting is always a stopping point", () => {
		expect(atSafeStoppingPoint(mkActivity("waiting"))).toBe(true);
	});

	it("running with an armed deferral is a stopping point", () => {
		expect(atSafeStoppingPoint(mkActivity("running", { delegationDeferredAt: 123 }))).toBe(true);
	});

	it("running with no deferral armed is not a stopping point", () => {
		expect(atSafeStoppingPoint(mkActivity("running"))).toBe(false);
	});

	it("permission is never a stopping point, even with a deferral armed underneath it", () => {
		expect(atSafeStoppingPoint(mkActivity("permission", { delegationDeferredAt: 123 }))).toBe(
			false,
		);
	});

	it.each(["spawning", "idle", "exited"] as const)(
		"phase %s is not a stopping point, deferral or not",
		(phase) => {
			expect(atSafeStoppingPoint(mkActivity(phase))).toBe(false);
			expect(atSafeStoppingPoint(mkActivity(phase, { delegationDeferredAt: 123 }))).toBe(false);
		},
	);
});
