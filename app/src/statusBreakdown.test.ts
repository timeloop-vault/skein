import { describe, expect, it } from "vitest";
import type { ActivityPhase, HarnessActivity } from "./harnessActivityTypes.ts";
import {
	BREAKDOWN_MAX_ROWS,
	BREAKDOWN_MAX_SUBAGENTS,
	type BreakdownDeps,
	type BreakdownHarnessInput,
	type BreakdownRoomInput,
	buildBreakdown,
} from "./statusBreakdown.ts";
import type { SubagentEntry } from "./subagents.ts";

// #331 — table-driven tests for the pure breakdown-popover model. No
// store, no DOM: every fixture is built by hand and threaded in via
// `BreakdownDeps`, the same seam `aggregateRoomStatus` uses.

const activity = (over: Partial<HarnessActivity> = {}): HarnessActivity => ({
	phase: "waiting",
	lastOutputAt: null,
	exitCode: null,
	spawnedAt: 0,
	hasUserInput: false,
	authoritative: true,
	tail: "",
	permissionTool: null,
	permissionAgentType: null,
	permissionAgentId: null,
	adapterHeard: true,
	promptSubmittedAt: null,
	adapterSilent: false,
	degradedBy: null,
	launchSignalAt: null,
	injected: true,
	delegationDeferredAt: null,
	delegationActivityAt: 0,
	delegationEmptiedAt: null,
	delegatedCount: 0,
	...over,
});

const phaseActivity = (
	phase: ActivityPhase,
	over: Partial<HarnessActivity> = {},
): HarnessActivity => activity({ phase, ...over });

const harness = (over: Partial<BreakdownHarnessInput> & { id: string }): BreakdownHarnessInput => ({
	kind: "claude",
	name: over.id,
	...over,
});

const room = (
	id: string,
	harnesses: readonly BreakdownHarnessInput[],
	name = id,
): BreakdownRoomInput => ({ id, name, harnesses });

// Builds `BreakdownDeps` from plain maps — a harness id missing from
// `activities` reports `null`, matching a harness with no store entry
// (e.g. `files`, or the first paint before any adapter has fired).
const makeDeps = (
	activities: Record<string, HarnessActivity>,
	subagentsById: Record<string, readonly SubagentEntry[]> = {},
	workingCounts: Record<string, number> = {},
): BreakdownDeps => ({
	activity: (id) => activities[id] ?? null,
	subagents: (id) => subagentsById[id] ?? [],
	workingCount: (id) => workingCounts[id] ?? 0,
});

const subagent = (over: Partial<SubagentEntry> & { agentId: string }): SubagentEntry => ({
	agentType: null,
	description: null,
	startedAt: 0,
	fromAttach: false,
	...over,
});

describe("buildBreakdown", () => {
	it("returns idle with no rows for no rooms at all", () => {
		const result = buildBreakdown([], makeDeps({}));
		expect(result.status).toBe("idle");
		expect(result.rows).toEqual([]);
		expect(result.quiet).toEqual([]);
		expect(result.quietCount).toBe(0);
	});

	it.each<[string, ActivityPhase, number, "row" | "quiet"]>([
		["permission surfaces a row", "permission", 0, "row"],
		["running surfaces a row", "running", 0, "row"],
		["waiting with pending notifications surfaces a row", "waiting", 1, "row"],
		["acknowledged waiting (0 pending) goes quiet", "waiting", 0, "quiet"],
		["idle goes quiet", "idle", 0, "quiet"],
		["exited goes quiet", "exited", 0, "quiet"],
	])("%s", (_name, phase, pendingNotifications, expected) => {
		const rooms = [room("r1", [harness({ id: "h1", pendingNotifications })])];
		const deps = makeDeps({ h1: phaseActivity(phase) });
		const result = buildBreakdown(rooms, deps);
		if (expected === "row") {
			expect(result.rows).toHaveLength(1);
			expect(result.quiet).toEqual([]);
		} else {
			expect(result.rows).toHaveLength(0);
			expect(result.quiet).toEqual([{ roomId: "r1", roomName: "r1", kinds: ["claude"] }]);
		}
	});

	it("counts a harness with no activity record as quiet, not running", () => {
		const rooms = [room("r1", [harness({ id: "h1" })])];
		const result = buildBreakdown(rooms, makeDeps({}));
		expect(result.rows).toEqual([]);
		expect(result.quietCount).toBe(1);
		expect(result.quiet).toEqual([{ roomId: "r1", roomName: "r1", kinds: ["claude"] }]);
	});

	it("orders rows by urgency — permission, then waiting, then running — across rooms", () => {
		const rooms = [
			room("r1", [harness({ id: "h-running", pendingNotifications: 0 })]),
			room("r2", [
				harness({ id: "h-permission" }),
				harness({ id: "h-waiting", pendingNotifications: 1 }),
			]),
		];
		const deps = makeDeps({
			"h-running": phaseActivity("running"),
			"h-permission": phaseActivity("permission"),
			"h-waiting": phaseActivity("waiting"),
		});
		const result = buildBreakdown(rooms, deps, { maxRows: 10 });
		expect(result.rows.map((r) => r.harnessId)).toEqual(["h-permission", "h-waiting", "h-running"]);
	});

	it("keeps input order as a tiebreak for rows sharing a status", () => {
		const rooms = [
			room("r1", [harness({ id: "h1" }), harness({ id: "h2" })]),
			room("r2", [harness({ id: "h3" })]),
		];
		const deps = makeDeps({
			h1: phaseActivity("running"),
			h2: phaseActivity("running"),
			h3: phaseActivity("running"),
		});
		const result = buildBreakdown(rooms, deps, { maxRows: 10 });
		expect(result.rows.map((r) => r.harnessId)).toEqual(["h1", "h2", "h3"]);
	});

	it("the top row's status equals the aggregate status", () => {
		const rooms = [
			room("r1", [harness({ id: "h1" })]),
			room("r2", [harness({ id: "h2" }), harness({ id: "h3" })]),
		];
		const deps = makeDeps({
			h1: phaseActivity("waiting", {}),
			h2: phaseActivity("permission"),
			h3: phaseActivity("running"),
		});
		const result = buildBreakdown(rooms, deps, { maxRows: 10 });
		expect(result.status).toBe("permission");
		expect(result.rows[0]?.status).toBe(result.status);
	});

	it("labels a running harness with working subagents as delegating", () => {
		const rooms = [room("r1", [harness({ id: "h1" })])];
		const deps = makeDeps({ h1: phaseActivity("running") }, {}, { h1: 2 });
		const result = buildBreakdown(rooms, deps);
		expect(result.rows[0]?.label).toBe("delegating · 2 agents");
	});

	it("labels a permission row with the tool and subagent name", () => {
		const rooms = [room("r1", [harness({ id: "h1" })])];
		const deps = makeDeps({
			h1: phaseActivity("permission", { permissionTool: "Bash", permissionAgentType: "explore" }),
		});
		const result = buildBreakdown(rooms, deps);
		expect(result.rows[0]?.label).toBe("permission needed · explore · Bash");
	});

	it("groups idle/exited/error harnesses per room, in harness order, omitting rooms with none", () => {
		const rooms = [
			room("r1", [harness({ id: "h1", kind: "claude" }), harness({ id: "h2", kind: "opencode" })]),
			room("r2", [harness({ id: "h3" })]),
		];
		const deps = makeDeps({
			h1: phaseActivity("idle"),
			h2: phaseActivity("exited"),
			h3: phaseActivity("running"),
		});
		const result = buildBreakdown(rooms, deps);
		expect(result.quiet).toEqual([{ roomId: "r1", roomName: "r1", kinds: ["claude", "opencode"] }]);
	});

	it("caps rows at maxRows and reports the remainder in moreRows", () => {
		const harnesses = Array.from({ length: BREAKDOWN_MAX_ROWS + 2 }, (_, i) =>
			harness({ id: `h${i}` }),
		);
		const activities: Record<string, HarnessActivity> = {};
		for (const h of harnesses) activities[h.id] = phaseActivity("running");
		const rooms = [room("r1", harnesses)];
		const result = buildBreakdown(rooms, makeDeps(activities));
		expect(result.rows).toHaveLength(BREAKDOWN_MAX_ROWS);
		expect(result.moreRows).toBe(2);
	});

	it("caps subagents at maxSubagents and reports the remainder in hiddenSubagents", () => {
		const rooms = [room("r1", [harness({ id: "h1" })])];
		const live = Array.from({ length: BREAKDOWN_MAX_SUBAGENTS + 1 }, (_, i) =>
			subagent({ agentId: `a${i}`, startedAt: i }),
		);
		const deps = makeDeps({ h1: phaseActivity("running") }, { h1: live });
		const result = buildBreakdown(rooms, deps);
		expect(result.rows[0]?.subagents).toHaveLength(BREAKDOWN_MAX_SUBAGENTS);
		expect(result.rows[0]?.hiddenSubagents).toBe(1);
	});

	it("marks an attach-time-only subagent as preRestart", () => {
		const rooms = [room("r1", [harness({ id: "h1" })])];
		const live = [
			subagent({
				agentId: "a1",
				fromAttach: true,
				agentType: "explore",
				description: "look around",
			}),
			subagent({ agentId: "a2", fromAttach: false }),
		];
		const deps = makeDeps({ h1: phaseActivity("running") }, { h1: live });
		const result = buildBreakdown(rooms, deps);
		const byId = new Map(result.rows[0]?.subagents.map((s) => [s.agentId, s]));
		expect(byId.get("a1")?.preRestart).toBe(true);
		expect(byId.get("a1")?.agentType).toBe("explore");
		expect(byId.get("a1")?.description).toBe("look around");
		expect(byId.get("a2")?.preRestart).toBe(false);
	});
});
