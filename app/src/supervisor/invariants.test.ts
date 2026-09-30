import { describe, expect, it } from "vitest";
import { DELEGATION_SETTLE_MS } from "../harnessActivityConstants";
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

function fires(code: SupervisorCode, s: SupervisorSnapshot, now = NOW): boolean {
	const inv = INVARIANTS.find((i) => i.code === code);
	if (!inv) throw new Error(`no invariant ${code}`);
	return inv.check(s, now).violated;
}

const sample = (at: number, size: number, mtimeMs = 1) => ({ at, size, mtimeMs });

const rows: [SupervisorCode, string, Partial<SupervisorSnapshot>, boolean][] = [
	// adapter_without_authority
	[
		"adapter_without_authority",
		"fires",
		{
			authoritative: false,
			lastAdapterEvent: { at: NOW - 1000, restoresAuthority: true },
			authorityLostAt: NOW - 5000,
		},
		true,
	],
	[
		"adapter_without_authority",
		"fires with no authorityLostAt",
		{ authoritative: false, lastAdapterEvent: { at: NOW - 1000, restoresAuthority: true } },
		true,
	],
	[
		"adapter_without_authority",
		"authoritative",
		{ lastAdapterEvent: { at: NOW - 1000, restoresAuthority: true } },
		false,
	],
	[
		"adapter_without_authority",
		"exited",
		{
			phase: "exited",
			authoritative: false,
			lastAdapterEvent: { at: NOW - 1000, restoresAuthority: true },
		},
		false,
	],
	[
		"adapter_without_authority",
		"adapterSilent",
		{
			authoritative: false,
			adapterSilent: true,
			lastAdapterEvent: { at: NOW - 1000, restoresAuthority: true },
		},
		false,
	],
	[
		"adapter_without_authority",
		"session_end event",
		{ authoritative: false, lastAdapterEvent: { at: NOW - 1000, restoresAuthority: false } },
		false,
	],
	[
		"adapter_without_authority",
		"event predates authorityLostAt",
		{
			authoritative: false,
			lastAdapterEvent: { at: NOW - 5000, restoresAuthority: true },
			authorityLostAt: NOW - 1000,
		},
		false,
	],
	[
		"adapter_without_authority",
		"no live attach",
		{
			authoritative: false,
			liveAttach: false,
			lastAdapterEvent: { at: NOW - 1000, restoresAuthority: true },
		},
		false,
	],
	["adapter_without_authority", "no adapter event", { authoritative: false }, false],
	// ended_turn_not_waiting
	[
		"ended_turn_not_waiting",
		"submit after the end",
		{ lastSubmitAt: NOW - 15_000, lastTurnSignal: { kind: "end", at: NOW - 20_000 } },
		false,
	],
	[
		"ended_turn_not_waiting",
		"submit before the end",
		{ lastSubmitAt: NOW - 25_000, lastTurnSignal: { kind: "end", at: NOW - 20_000 } },
		true,
	],
	[
		"ended_turn_not_waiting",
		"running",
		{ lastTurnSignal: { kind: "end", at: NOW - 20_000 } },
		true,
	],
	[
		"ended_turn_not_waiting",
		"idle, no output ever",
		{ phase: "idle", lastOutputAt: null, lastTurnSignal: { kind: "end", at: NOW - 20_000 } },
		true,
	],
	["ended_turn_not_waiting", "no turn signal", {}, false],
	[
		"ended_turn_not_waiting",
		"work signal",
		{ lastTurnSignal: { kind: "work", at: NOW - 20_000 } },
		false,
	],
	[
		"ended_turn_not_waiting",
		"subagents working",
		{ subagentsWorking: 1, lastTurnSignal: { kind: "end", at: NOW - 20_000 } },
		false,
	],
	[
		"ended_turn_not_waiting",
		"deferred",
		{ delegationDeferredAt: NOW - 1000, lastTurnSignal: { kind: "end", at: NOW - 20_000 } },
		false,
	],
	[
		"ended_turn_not_waiting",
		"recent output",
		{ lastOutputAt: NOW - 2000, lastTurnSignal: { kind: "end", at: NOW - 20_000 } },
		false,
	],
	...(["permission", "exited", "waiting", "spawning"] as const).map(
		(phase) =>
			[
				"ended_turn_not_waiting",
				`phase ${phase}`,
				{ phase, lastTurnSignal: { kind: "end", at: NOW - 20_000 } },
				false,
			] as [SupervisorCode, string, Partial<SupervisorSnapshot>, boolean],
	),
	// mail_held
	[
		"mail_held",
		"fires",
		{ phase: "waiting", mail: { unread: 2, lastRefusal: "busy", lastNudgeAt: null } },
		true,
	],
	[
		"mail_held",
		"nudge before waiting began",
		{
			phase: "waiting",
			mail: { unread: 1, lastRefusal: null, lastNudgeAt: NOW - 200_000 },
		},
		true,
	],
	["mail_held", "no mail", { phase: "waiting" }, false],
	[
		"mail_held",
		"nothing unread",
		{ phase: "waiting", mail: { unread: 0, lastRefusal: null, lastNudgeAt: null } },
		false,
	],
	[
		"mail_held",
		"not waiting",
		{ phase: "running", mail: { unread: 1, lastRefusal: null, lastNudgeAt: null } },
		false,
	],
	[
		"mail_held",
		"nudged since waiting",
		{ phase: "waiting", mail: { unread: 1, lastRefusal: null, lastNudgeAt: NOW - 50_000 } },
		false,
	],
	// tail_silent_file_growing
	[
		"tail_silent_file_growing",
		"size grew",
		{ transcript: [sample(NOW - 70_000, 100), sample(NOW, 200)] },
		true,
	],
	[
		"tail_silent_file_growing",
		"mtime grew only",
		{ transcript: [sample(NOW - 70_000, 100, 1), sample(NOW, 100, 2)] },
		true,
	],
	[
		"tail_silent_file_growing",
		"silent since last adapter event",
		{
			lastAdapterEvent: { at: NOW - 70_000, restoresAuthority: true },
			transcript: [sample(NOW - 100_000, 10), sample(NOW - 70_000, 100), sample(NOW, 200)],
		},
		true,
	],
	[
		"tail_silent_file_growing",
		"growth predates adapter event",
		{
			lastAdapterEvent: { at: NOW - 30_000, restoresAuthority: true },
			transcript: [sample(NOW - 100_000, 10), sample(NOW - 70_000, 100), sample(NOW, 100)],
		},
		false,
	],
	["tail_silent_file_growing", "no samples", {}, false],
	["tail_silent_file_growing", "one sample", { transcript: [sample(NOW, 100)] }, false],
	[
		"tail_silent_file_growing",
		"no live attach",
		{ liveAttach: false, transcript: [sample(NOW - 70_000, 100), sample(NOW, 200)] },
		false,
	],
	[
		"tail_silent_file_growing",
		"unchanged file",
		{ transcript: [sample(NOW - 70_000, 100), sample(NOW, 100)] },
		false,
	],
	[
		"tail_silent_file_growing",
		"window too short",
		{ transcript: [sample(NOW - 30_000, 100), sample(NOW, 200)] },
		false,
	],
	// deferral_without_work
	["deferral_without_work", "fires", { delegationDeferredAt: NOW - 20_000 }, true],
	[
		"deferral_without_work",
		"emptied past settle",
		{
			delegationDeferredAt: NOW - 200_000,
			delegationEmptiedAt: NOW - DELEGATION_SETTLE_MS,
		},
		true,
	],
	["deferral_without_work", "not deferred", {}, false],
	[
		"deferral_without_work",
		"subagents working",
		{ delegationDeferredAt: NOW - 20_000, subagentsWorking: 2 },
		false,
	],
	[
		"deferral_without_work",
		"permission",
		{ delegationDeferredAt: NOW - 20_000, phase: "permission" },
		false,
	],
	[
		"deferral_without_work",
		"exited",
		{ delegationDeferredAt: NOW - 20_000, phase: "exited" },
		false,
	],
	[
		"deferral_without_work",
		"emptied recently",
		{ delegationDeferredAt: NOW - 20_000, delegationEmptiedAt: NOW - 1000 },
		false,
	],
	// permission_orphaned
	[
		"permission_orphaned",
		"turn moved on",
		{
			phase: "permission",
			permissionAt: NOW - 30_000,
			lastTurnSignal: { kind: "work", at: NOW - 20_000 },
		},
		true,
	],
	[
		"permission_orphaned",
		"ceiling",
		{ phase: "permission", permissionAt: NOW - 15 * 60_000 },
		true,
	],
	[
		"permission_orphaned",
		"ceiling with subagent dialog",
		{ phase: "permission", permissionAt: NOW - 16 * 60_000, permissionAgentId: "a1" },
		true,
	],
	["permission_orphaned", "not in permission", { permissionAt: NOW - 16 * 60_000 }, false],
	["permission_orphaned", "no permissionAt", { phase: "permission" }, false],
	[
		"permission_orphaned",
		"subagent dialog, turn moved on",
		{
			phase: "permission",
			permissionAt: NOW - 30_000,
			permissionAgentId: "a1",
			lastTurnSignal: { kind: "work", at: NOW - 20_000 },
		},
		false,
	],
	[
		"permission_orphaned",
		"turn moved on but subagents working",
		{
			phase: "permission",
			permissionAt: NOW - 30_000,
			subagentsWorking: 1,
			lastTurnSignal: { kind: "work", at: NOW - 20_000 },
		},
		false,
	],
	[
		"permission_orphaned",
		"subagents working still hit the ceiling",
		{ phase: "permission", permissionAt: NOW - 16 * 60_000, subagentsWorking: 1 },
		true,
	],
	[
		"permission_orphaned",
		"turn signal older than dialog",
		{
			phase: "permission",
			permissionAt: NOW - 30_000,
			lastTurnSignal: { kind: "work", at: NOW - 40_000 },
		},
		false,
	],
	[
		"permission_orphaned",
		"turn signal too fresh",
		{
			phase: "permission",
			permissionAt: NOW - 30_000,
			lastTurnSignal: { kind: "work", at: NOW - 5000 },
		},
		false,
	],
];

describe("invariants", () => {
	it.each(rows)("%s: %s", (code, _name, over, expected) => {
		expect(fires(code, snap(over))).toBe(expected);
	});

	it("evidence names the facts", () => {
		const inv = INVARIANTS.find((i) => i.code === "ended_turn_not_waiting");
		const v = inv?.check(snap({ lastTurnSignal: { kind: "end", at: NOW - 20_000 } }), NOW);
		expect(v).toMatchObject({ violated: true });
		if (v?.violated) {
			expect(v.evidence).toContain("phase=running");
			expect(v.evidence).toContain("lastTurn=end@20000ms");
		}
	});

	it("permission evidence says which arm", () => {
		const inv = INVARIANTS.find((i) => i.code === "permission_orphaned");
		const a = inv?.check(snap({ phase: "permission", permissionAt: NOW - 16 * 60_000 }), NOW);
		expect(a).toMatchObject({ violated: true, evidence: expect.stringContaining("arm=ceiling") });
	});

	it("declares each code once", () => {
		const codes = INVARIANTS.map((i) => i.code);
		expect(new Set(codes).size).toBe(codes.length);
	});
});
