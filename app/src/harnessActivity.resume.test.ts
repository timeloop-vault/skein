import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

// #336 — a resumed Claude harness whose transcript stops mid-turn belongs
// to a dead process. Rust replays that as `awaiting_prompt` when the attach
// is for a freshly spawned process (`freshProcess`). Driven through the real
// `attachClaudeEvents` translator on a mocked Channel. Fake timers must be
// live before the first `spawned()` starts the module-global tick.

const h = vi.hoisted(() => {
	const channels: Array<{ onmessage: (e: unknown) => void }> = [];
	const invokes: Array<{ cmd: string; args: Record<string, unknown> }> = [];
	return { channels, invokes };
});

vi.mock("@tauri-apps/api/core", () => ({
	Channel: class {
		onmessage: (e: unknown) => void = () => {};
		constructor() {
			h.channels.push(this);
		}
	},
	invoke: (cmd: string, args: Record<string, unknown>) => {
		h.invokes.push({ cmd, args });
		return new Promise<void>(() => {});
	},
}));
vi.mock("./frontendLog.ts", () => ({ logBoth: vi.fn(), logToRust: vi.fn() }));

import { harnessActivity } from "./harnessActivity.ts";
import {
	ADAPTER_SILENT_AFTER_MS,
	DELEGATION_CEILING_MS,
	LAUNCH_SILENT_AFTER_MS,
} from "./harnessActivityConstants.ts";
import { attachClaudeEvents } from "./harnessEvents.ts";
import { attachAdapters } from "./terminalSpawnAdapters.ts";

let n = 0;
const send = (event: unknown) => h.channels[h.channels.length - 1]?.onmessage(event);
const lastAttach = () => {
	const all = h.invokes.filter((i) => i.cmd === "claude_events_attach");
	return all[all.length - 1];
};

describe("resume onto a transcript that ends mid tool call (#336)", () => {
	beforeAll(() => {
		vi.useFakeTimers();
	});
	afterAll(() => {
		vi.useRealTimers();
	});
	beforeEach(() => {
		h.channels.length = 0;
		h.invokes.length = 0;
	});

	it("sends freshProcess in the attach payload", () => {
		const a = `resume_${++n}`;
		harnessActivity.spawned(a);
		attachClaudeEvents(a, "room", "sid", "/cwd", true);
		expect(lastAttach()?.args.freshProcess).toBe(true);

		const b = `resume_${++n}`;
		harnessActivity.spawned(b);
		attachClaudeEvents(b, "room", "sid", "/cwd", false);
		expect(lastAttach()?.args.freshProcess).toBe(false);

		harnessActivity.forget(a);
		harnessActivity.forget(b);
	});

	it("a resumed harness whose replay is awaiting_prompt reads waiting with no prompt typed", () => {
		const id = `resume_${++n}`;
		harnessActivity.spawned(id);
		attachClaudeEvents(id, "room", "sid", "/cwd", true);
		harnessActivity.recordOutput(id, "welcome banner");

		send({ kind: "awaiting_prompt" });

		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		harnessActivity.forget(id);
	});

	it("the real attachAdapters sends freshProcess: true for a Claude harness", () => {
		const id = `resume_${++n}`;
		harnessActivity.spawned(id);
		attachAdapters({
			harnessId: id,
			roomId: "room",
			cwd: "/cwd",
			harnessKind: "claude",
			sessionId: "sid",
			opencodePort: undefined,
			onSessionCaptured: undefined,
			onSessionFollowed: undefined,
			sessionIdRef: { current: "sid" },
			claudeAdapterRef: { current: null },
		});
		expect(lastAttach()?.args.freshProcess).toBe(true);
		harnessActivity.forget(id);
	});

	it("an adapter-reported assistant_turn with no further events never decays to waiting", () => {
		const id = `resume_${++n}`;
		harnessActivity.spawned(id);
		attachClaudeEvents(id, "room", "sid", "/cwd", false);
		send({ kind: "assistant_turn" });
		expect(harnessActivity.get(id)?.phase).toBe("running");

		const past = Math.max(ADAPTER_SILENT_AFTER_MS, LAUNCH_SILENT_AFTER_MS, DELEGATION_CEILING_MS);
		vi.advanceTimersByTime(past * 2);

		expect(harnessActivity.get(id)?.phase).toBe("running");
		harnessActivity.forget(id);
	});
});
