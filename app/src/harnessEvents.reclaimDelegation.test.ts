import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

// #634 — a LIVE subagent/background start flushed after the main end_turn
// must pull a `waiting` harness back into the #277 deferral, silently.
// Own file: fake timers must be installed before the first `spawned()`.

const h = vi.hoisted(() => {
	const channels: Array<{ onmessage: (e: unknown) => void }> = [];
	return { channels };
});

vi.mock("@tauri-apps/api/core", () => ({
	Channel: class {
		onmessage: (e: unknown) => void = () => {};
		constructor() {
			h.channels.push(this);
		}
	},
	invoke: () => new Promise<void>(() => {}),
}));
vi.mock("./frontendLog.ts", () => ({ logBoth: vi.fn(), logToRust: vi.fn() }));

import { backgroundTasks } from "./backgroundTasks.ts";
import { harnessActivity, TRANSITION_SOURCE } from "./harnessActivity.ts";
import { statusLabel } from "./harnessActivityLabels.ts";
import { attachClaudeEvents } from "./harnessEvents.ts";
import { classifyTransition, isNotifiable } from "./harnessNotifyLogic.ts";
import { subagents } from "./subagents.ts";

let n = 0;
const send = (event: unknown) => h.channels[h.channels.length - 1]?.onmessage(event);

const attached = (): string => {
	const id = `rc_${++n}`;
	harnessActivity.spawned(id);
	attachClaudeEvents(id, "room", "sid", "/cwd");
	send({ kind: "assistant_turn" });
	return id;
};
const waitingHarness = (): string => {
	const id = attached();
	send({ kind: "awaiting_prompt" });
	expect(harnessActivity.get(id)?.phase).toBe("waiting");
	return id;
};
const sub = (initial: boolean): void =>
	send({
		kind: "subagent_start",
		agent_id: "a1",
		agent_type: "explore",
		description: null,
		initial,
	});
const bg = (initial: boolean): void =>
	send({
		kind: "background_start",
		task_id: "t1",
		tool_use_id: "tu_t1",
		task_kind: "bash",
		description: null,
		command: "sleep 999",
		timeout_ms: null,
		persistent: false,
		auto_backgrounded: false,
		agent_id: null,
		initial,
	});
const cleanup = (id: string): void => {
	harnessActivity.forget(id);
	subagents.forget(id);
	backgroundTasks.forget(id);
};
const watch = (id: string) => {
	const seen: Array<{ from: string; to: string; source: string }> = [];
	const unsub = harnessActivity.subscribeTransitions((tid, from, to, source) => {
		if (tid === id) seen.push({ from, to, source });
	});
	return { seen, unsub };
};

describe("live delegated-work start reclaims a waiting harness (#634)", () => {
	beforeAll(() => {
		vi.useFakeTimers();
	});
	afterAll(() => {
		vi.useRealTimers();
	});
	beforeEach(() => {
		h.channels.length = 0;
	});

	it("live subagent_start after a zero-work end turn: running, deferral armed, silent, then settles", () => {
		const id = waitingHarness();
		const { seen, unsub } = watch(id);
		sub(false);
		const a = harnessActivity.get(id);
		expect(a?.phase).toBe("running");
		expect(a?.delegationDeferredAt).not.toBeNull();
		expect(statusLabel("running", null, null, subagents.workingCount(id))).toBe(
			"delegating · 1 agent",
		);
		expect(seen).toEqual([{ from: "waiting", to: "running", source: "delegation-resumed" }]);
		expect(seen[0]?.source).toBe(TRANSITION_SOURCE.DelegationResumed);
		expect(
			isNotifiable(classifyTransition("waiting", "running"), TRANSITION_SOURCE.DelegationResumed),
		).toBe(false);

		send({ kind: "subagent_end", agent_id: "a1", agent_type: "explore", description: null });
		vi.advanceTimersByTime(61_000);
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		expect(seen[1]?.source).toBe(TRANSITION_SOURCE.DelegationSettled);
		unsub();
		cleanup(id);
	});

	it("live background_start does the same", () => {
		const id = waitingHarness();
		const { seen, unsub } = watch(id);
		bg(false);
		expect(harnessActivity.get(id)?.phase).toBe("running");
		expect(harnessActivity.get(id)?.delegationDeferredAt).not.toBeNull();
		expect(seen.map((s) => s.source)).toEqual([TRANSITION_SOURCE.DelegationResumed]);
		send({ kind: "background_end", task_id: "t1", agent_id: null });
		vi.advanceTimersByTime(61_000);
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		expect(seen[1]?.source).toBe(TRANSITION_SOURCE.DelegationSettled);
		unsub();
		cleanup(id);
	});

	it("initial (attach) starts never reclaim", () => {
		const id = waitingHarness();
		sub(true);
		bg(true);
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		expect(harnessActivity.get(id)?.delegationDeferredAt).toBeNull();
		cleanup(id);
	});

	it("permission, a work signal, or a non-authoritative harness are left alone", () => {
		const p = waitingHarness();
		harnessActivity.setPermissionFromAdapter(p, TRANSITION_SOURCE.L2c1ClaudePermission, "Bash");
		sub(false);
		expect(harnessActivity.get(p)?.phase).toBe("permission");
		cleanup(p);

		// get() hands back the live store entry, so the gates can be flipped directly.
		const w = waitingHarness();
		const e = harnessActivity.get(w) as NonNullable<ReturnType<typeof harnessActivity.get>>;
		e.lastTurnSignal = { kind: "work", at: Date.now() };
		sub(false);
		expect(harnessActivity.get(w)?.phase).toBe("waiting");
		e.lastTurnSignal = { kind: "end", at: Date.now() };
		e.authoritative = false;
		// Any event re-asserts authority (adapterDelivered), so call the gate directly.
		harnessActivity.reclaimDelegationFromAdapter(w);
		expect(harnessActivity.get(w)?.phase).toBe("waiting");
		cleanup(w);
	});
});
