import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
	invoke: vi.fn(() => Promise.resolve()),
	Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("./frontendLog.ts", () => ({ logBoth: vi.fn(), logToRust: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import { createHermesBinding, type HermesBindingDeps } from "./harnessEventsHermes.ts";
import { startHermesCapture } from "./hermesCapture.ts";
import type { HermesHookEvent } from "./hermesHookAdapter.ts";
import { applyUpdate, resetPhaseMirror } from "./phaseMirror.ts";
import { phaseUpdate, wireActivity } from "./phaseTestSupport.ts";

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

const phaseInputs = (): unknown[] =>
	vi
		.mocked(invoke)
		.mock.calls.filter(([cmd]) => cmd === "harness_phase_input")
		.map(([, args]) => (args as { input: unknown }).input);

beforeEach(() => {
	resetPhaseMirror();
	vi.mocked(invoke).mockClear();
});

describe("Hermes hook binding", () => {
	it("attaches a resumed session immediately with resumed: true", () => {
		const { calls } = setup("unknown", "OLD");
		expect(calls[0]).toEqual([
			"hermes_actions_attach",
			{ harnessId: "hh", roomId: "r", sessionId: "OLD", hermesProfile: null, resumed: true },
		]);
	});

	it("attaches a fresh main session on on_session_start, capturing it", () => {
		const { b, calls, captured } = setup();
		b.handle({ ...base, event: "on_session_start" });
		expect(captured).toEqual(["S"]);
		expect(calls[0]).toEqual([
			"hermes_actions_attach",
			{ harnessId: "hh", roomId: "r", sessionId: "S", hermesProfile: null, resumed: false },
		]);
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
		const { b, captured } = setup();
		b.handle({ ...base, sessionId: "HOOK", event: "on_session_start" });
		expect(captured).toEqual(["HOOK"]);
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

	it("ignores a child session's hooks", () => {
		const { b, calls, captured } = setup();
		b.handle({ ...base, event: "on_session_start" });
		b.handle({ ...base, event: "subagent_start", childSessionId: "C" });
		const before = calls.length;
		const child = { ...base, sessionId: "C" };
		b.handle({ ...child, event: "on_session_start" });
		b.handle({ ...child, event: "on_session_end", completed: true });
		expect(calls).toHaveLength(before);
		expect(captured).toEqual(["S"]);
	});

	it("invokes hermes_actions_turn_end when the main session ends", () => {
		const { b, calls } = setup();
		b.handle({ ...base, event: "on_session_start" });
		b.handle({ ...base, event: "on_session_end", completed: true });
		expect(calls[calls.length - 1]).toEqual(["hermes_actions_turn_end", { harnessId: "hh" }]);
	});

	it("dispose detaches the actions once and silences later hooks", () => {
		const { b, calls } = setup();
		b.dispose();
		b.dispose();
		expect(calls.filter(([c]) => c === "hermes_actions_detach")).toEqual([
			["hermes_actions_detach", { harnessId: "hh" }],
		]);
		b.handle({ ...base, event: "on_session_start" });
		expect(calls).toHaveLength(1);
	});

	it("forwards hermesLaunchPoll while spawning, and stops forwarding after", () => {
		applyUpdate(phaseUpdate("hh", { view: wireActivity("spawning") }));
		const ready = setup("waiting");
		ready.tick();
		const unknown = setup("unknown");
		unknown.tick();
		expect(phaseInputs()).toEqual([
			{ kind: "hermesLaunchPoll", screenWaiting: true },
			{ kind: "hermesLaunchPoll", screenWaiting: false },
		]);
		applyUpdate(phaseUpdate("hh", { view: wireActivity("running") }));
		ready.tick();
		expect(phaseInputs()).toHaveLength(2);
	});
});
