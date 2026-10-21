import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class {} }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));

import { backgroundTasks } from "./backgroundTasks.ts";
import { harnessActivity } from "./harnessActivity.ts";
import { createHermesBinding, type HermesBindingDeps } from "./harnessEventsHermes.ts";
import { startHermesCapture } from "./hermesCapture.ts";
import type { HermesHookEvent } from "./hermesHookAdapter.ts";
import type { HermesWorkEvent } from "./hermesWork.ts";
import { subagents } from "./subagents.ts";

const base: HermesHookEvent = {
	roomId: "r",
	harnessId: "hh",
	event: "pre_llm_call",
	sessionId: "S",
	turnId: "T",
	toolCallId: null,
	toolName: null,
	platform: "cli",
	surface: null,
	description: null,
	choice: null,
	completed: null,
	interrupted: null,
	failed: null,
	turnExitReason: null,
	model: null,
	childSessionId: null,
};

function setup(screen: "waiting" | "unknown" = "unknown", bound?: string) {
	const calls: [string, Record<string, unknown>][] = [];
	let tickFn: (() => void) | null = null;
	const deps: HermesBindingDeps = {
		call: async (cmd, args) => {
			calls.push([cmd, args]);
		},
		readScreen: () => screen,
		setInterval: (fn) => {
			tickFn = fn;
			return 1;
		},
		clearInterval: () => {},
	};
	harnessActivity.spawned("hh");
	const captured: string[] = [];
	const b = createHermesBinding(
		{
			harnessId: "hh",
			roomId: "r",
			boundSessionId: bound,
			getSessionId: () => undefined,
			onSessionCaptured: (s) => captured.push(s),
		},
		deps,
	);
	return { b, calls, captured, tick: () => tickFn?.() };
}
const phase = () => harnessActivity.get("hh")?.phase;

describe("Hermes hook binding", () => {
	it("drives running / permission / waiting and the action commands", () => {
		const { b, calls, captured } = setup();
		expect(harnessActivity.get("hh")?.authoritative).toBe(true);
		b.handle({ ...base, event: "on_session_start" });
		expect(phase()).toBe("running");
		expect(captured).toEqual(["S"]);
		expect(calls[0]).toEqual([
			"hermes_actions_attach",
			{ harnessId: "hh", roomId: "r", sessionId: "S", hermesProfile: null, resumed: false },
		]);
		b.handle({
			...base,
			event: "pre_approval_request",
			surface: "cli",
			toolCallId: "c",
			toolName: "terminal",
		});
		expect(phase()).toBe("permission");
		b.handle({ ...base, event: "pre_llm_call" });
		expect(phase()).toBe("permission");
		b.handle({ ...base, event: "post_approval_response", surface: "cli", toolCallId: "c" });
		expect(phase()).toBe("running");
		b.handle({ ...base, event: "on_session_end", completed: true });
		expect(phase()).toBe("waiting");
		expect(calls[calls.length - 1]).toEqual(["hermes_actions_turn_end", { harnessId: "hh" }]);
		b.dispose();
		expect(calls).toContainEqual(["hermes_actions_detach", { harnessId: "hh" }]);
		expect(harnessActivity.get("hh")?.authoritative).toBe(false);
	});

	it("attaches a resumed session immediately with resumed: true", () => {
		const { calls } = setup("unknown", "OLD");
		expect(calls[0]?.[1]).toMatchObject({ sessionId: "OLD", resumed: true });
	});

	it("a session id from the hook claims it and stops this harness's capture poll", async () => {
		const timers: (() => void)[] = [];
		const cleared: unknown[] = [];
		const claim = vi.fn(async () => "OTHER");
		startHermesCapture({
			harnessId: "hh",
			cwd: "/w",
			spawnedAtMs: 0,
			onCapture: () => {},
			invoke: claim,
			setTimer: (fn) => timers.push(fn),
			clearTimer: (h) => cleared.push(h),
		});
		const { b } = setup();
		b.handle({ ...base, sessionId: "HOOK", event: "on_session_start" });
		expect(cleared).toEqual([1]);
		// a late timer fire is a no-op: the loop is stopped
		timers[0]?.();
		expect(claim).not.toHaveBeenCalled();
		// and the id is claimed: no other harness's claim result can take it
		const onOther = vi.fn();
		const fns: (() => void)[] = [];
		const other = startHermesCapture({
			harnessId: "other",
			cwd: "/w",
			spawnedAtMs: 0,
			onCapture: onOther,
			invoke: async () => "HOOK",
			setTimer: (fn) => fns.push(fn),
			clearTimer: () => {},
		});
		fns[0]?.();
		await new Promise((r) => setTimeout(r, 0));
		other();
		expect(onOther).not.toHaveBeenCalled();
	});

	it("a child's approval answered after the turn ended settles to waiting", () => {
		const { b } = setup();
		b.handle({ ...base, event: "on_session_start" });
		b.handle({ ...base, event: "subagent_start", childSessionId: "C" });
		b.handle({ ...base, event: "on_session_end" });
		expect(phase()).toBe("running"); // #628: the child still runs, so the end defers
		const child = { ...base, surface: "cli", sessionId: "C", toolCallId: "k" };
		b.handle({ ...child, event: "pre_approval_request" });
		expect(phase()).toBe("permission");
		b.handle({ ...child, event: "post_approval_response" });
		expect(phase()).toBe("waiting");
	});

	it("leaves spawning on two ready screen reads, without a hook", () => {
		const { tick } = setup("waiting");
		expect(phase()).toBe("spawning");
		tick();
		expect(phase()).toBe("spawning");
		tick();
		expect(phase()).toBe("waiting");
	});

	it("defers a turn end while a delegated child runs, and waits after its wake turn (#628)", () => {
		const { b } = setup();
		b.handle({ ...base, event: "on_session_start" });
		b.handle({ ...base, event: "subagent_start", childSessionId: "C" });
		expect(subagents.workingCount("hh")).toBe(1);
		b.handle({ ...base, event: "on_session_end" });
		expect(phase()).toBe("running");
		expect(harnessActivity.get("hh")?.delegationDeferredAt).not.toBeNull();
		// a child hook never touches the parent phase
		b.handle({ ...base, sessionId: "C", platform: "subagent", event: "on_session_end" });
		expect(phase()).toBe("running");
		b.handle({ ...base, event: "subagent_stop", childSessionId: "C" });
		expect(subagents.workingCount("hh")).toBe(0);
		b.handle({ ...base, event: "pre_llm_call" });
		b.handle({ ...base, event: "on_session_end" });
		expect(phase()).toBe("waiting");
		expect(harnessActivity.get("hh")?.delegationDeferredAt).toBeNull();
	});

	it("attaches the work stream and applies a background process from it (#628)", () => {
		const { b, calls } = setup();
		b.handle({ ...base, event: "on_session_start" });
		const attach = calls.find(([c]) => c === "hermes_work_attach");
		expect(attach?.[1]).toMatchObject({ harnessId: "hh", sessionId: "S", resumed: false });
		const ch = attach?.[1].onEvent as { onmessage: (e: HermesWorkEvent) => void };
		ch.onmessage({ kind: "backgroundStart", taskId: "p1", command: "sleep 9", initial: false });
		b.handle({ ...base, event: "on_session_end" });
		expect(phase()).toBe("running");
		ch.onmessage({ kind: "backgroundEnd", taskId: "p1" });
		expect(backgroundTasks.workingCount("hh")).toBe(0);
		b.dispose();
		expect(calls.some(([c]) => c === "hermes_work_detach")).toBe(true);
	});

	it("clears the old session's delegated work on a session change and on dispose (#628)", () => {
		const { b } = setup();
		b.handle({ ...base, event: "on_session_start" });
		b.handle({ ...base, event: "subagent_start", childSessionId: "C" });
		backgroundTasks.record(
			"hh",
			{
				taskId: "p1",
				kind: "bash",
				description: null,
				command: null,
				timeoutMs: null,
				persistent: false,
				agentId: null,
			},
			false,
		);
		expect(subagents.workingCount("hh")).toBe(1);
		b.handle({ ...base, sessionId: "S2", event: "on_session_start" });
		expect(subagents.workingCount("hh")).toBe(0);
		expect(backgroundTasks.workingCount("hh")).toBe(0);
		b.handle({ ...base, sessionId: "S2", event: "subagent_start", childSessionId: "D" });
		expect(subagents.workingCount("hh")).toBe(1);
		b.dispose();
		expect(subagents.workingCount("hh")).toBe(0);
	});
});
