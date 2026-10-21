import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";

// #628: a lost `subagent_stop` hook is repaired by the disk stream's
// `delegationsIdle` edge. Own file: the idle tick is a module-global
// interval, so fake timers must be installed before the first `spawned()`.

const h = vi.hoisted(() => ({ channels: [] as Array<{ onmessage: (e: unknown) => void }> }));

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

import { harnessActivity, TRANSITION_SOURCE } from "./harnessActivity.ts";
import { createHermesBinding } from "./harnessEventsHermes.ts";
import type { HermesHookEvent } from "./hermesHookAdapter.ts";
import { subagents } from "./subagents.ts";

const base: HermesHookEvent = {
	roomId: "r",
	harnessId: "hi",
	event: "on_session_start",
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

describe("delegationsIdle (#628)", () => {
	beforeAll(() => {
		vi.useFakeTimers();
	});
	afterAll(() => {
		vi.useRealTimers();
	});

	it("finishes a hook-started subagent whose stop was lost, and the deferral settles", () => {
		harnessActivity.spawned("hi");
		const b = createHermesBinding(
			{ harnessId: "hi", roomId: "r", getSessionId: () => undefined },
			{
				call: async () => {},
				readScreen: () => "unknown",
				setInterval: () => 1,
				clearInterval: () => {},
			},
		);
		b.handle(base);
		b.handle({ ...base, event: "subagent_start", childSessionId: "C" });
		b.handle({ ...base, event: "on_session_end" });
		expect(harnessActivity.get("hi")?.phase).toBe("running");
		expect(subagents.workingCount("hi")).toBe(1);

		const ch = h.channels[h.channels.length - 1];
		ch?.onmessage({ kind: "delegationsIdle" });
		expect(subagents.workingCount("hi")).toBe(0);

		const sources: string[] = [];
		const unsub = harnessActivity.subscribeTransitions((id, _f, _t, source) => {
			if (id === "hi") sources.push(source);
		});
		vi.advanceTimersByTime(61_000);
		expect(harnessActivity.get("hi")?.phase).toBe("waiting");
		expect(sources).toEqual([TRANSITION_SOURCE.DelegationSettled]);
		unsub();
		b.dispose();
	});
});
