import { beforeEach, describe, expect, it, vi } from "vitest";

// #672 — an open opencode `question` reads as phase `waiting` but must
// make `canSendPrompt` refuse, so mail / nudges are never pasted into it.

const h = vi.hoisted(() => {
	const channels: Array<{ onmessage: (e: unknown) => void }> = [];
	return { channels, logBoth: vi.fn(), logToRust: vi.fn() };
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
vi.mock("./frontendLog.ts", () => ({ logBoth: h.logBoth, logToRust: h.logToRust }));

import { HARNESS_KINDS } from "./data.tsx";
import { harnessActivity } from "./harnessActivity.ts";
import { attachOpencodeEvents } from "./harnessEvents.ts";
import { canSendPrompt } from "./harnessInput.ts";

let n = 0;
const fresh = (): string => {
	const id = `q_${++n}`;
	harnessActivity.spawned(id);
	harnessActivity.setInjected(id, true);
	attachOpencodeEvents(id, "room", "/cwd", 1234, undefined, undefined, () => undefined, undefined);
	return id;
};
const send = (event: unknown) => h.channels[h.channels.length - 1]?.onmessage(event);
const open = (id: string): boolean | undefined => harnessActivity.get(id)?.questionOpen;
const gate = (id: string) =>
	canSendPrompt({
		capabilities: HARNESS_KINDS.opencode.capabilities,
		activity: harnessActivity.get(id),
		registered: true,
		bracketedPasteOn: false,
		body: "nudge",
	});
const asked = { kind: "question_asked", request_id: "q1", session_id: "s" };
const REFUSED = { ok: false, reason: "the harness is waiting on an answer to its own question" };

beforeEach(() => {
	h.channels.length = 0;
});

describe("opencode question gating", () => {
	it("question_asked sets questionOpen and the gate refuses", () => {
		const id = fresh();
		expect(open(id)).toBe(false);
		send({ kind: "session_idle" });
		expect(gate(id)).toEqual({ ok: true });
		send(asked);
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		expect(open(id)).toBe(true);
		expect(gate(id)).toEqual(REFUSED);
	});

	it("question_resolved clears it, then session_idle reaches a sendable waiting", () => {
		const id = fresh();
		send(asked);
		send({ kind: "question_resolved", request_id: "q1", session_id: "s" });
		expect(open(id)).toBe(false);
		expect(harnessActivity.get(id)?.phase).toBe("running");
		send({ kind: "session_idle" });
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		expect(gate(id)).toEqual({ ok: true });
	});

	it("does not notify subscribers when unchanged", () => {
		const id = fresh();
		send(asked);
		let emits = 0;
		const unsub = harnessActivity.subscribe(id, () => emits++);
		harnessActivity.setQuestionOpen(id, true);
		unsub();
		expect(emits).toBe(0);
	});

	it.each([
		["session_idle", { kind: "session_idle" }],
		["session_end", { kind: "session_end" }],
		["connected", { kind: "connected" }],
		["session_baseline_idle", { kind: "session_baseline_idle" }],
	])("%s clears it", (_name, event) => {
		const id = fresh();
		send(asked);
		expect(open(id)).toBe(true);
		send(event);
		expect(open(id)).toBe(false);
	});
});
