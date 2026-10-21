import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class {} }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));

import { harnessActivity } from "./harnessActivity.ts";
import { createHermesBinding, type HermesBindingDeps } from "./harnessEventsHermes.ts";
import { startHermesCapture } from "./hermesCapture.ts";
import type { HermesHookEvent } from "./hermesHookAdapter.ts";

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
		expect(calls[calls.length - 1]).toEqual(["hermes_actions_detach", { harnessId: "hh" }]);
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
		expect(phase()).toBe("waiting");
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
});
