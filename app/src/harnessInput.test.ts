import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({
	Channel: class {
		onmessage: (e: unknown) => void = () => {};
	},
	invoke: invokeMock,
}));
vi.mock("./frontendLog.ts", () => ({ logBoth: vi.fn(), logToRust: vi.fn() }));

import { insertText } from "./harnessInput.ts";
import { type HarnessInputTarget, harnessInput } from "./harnessInputRegistry.ts";
import { mirrorSet } from "./phaseTestSupport.ts";

const nextId = (() => {
	let n = 0;
	return () => `h_${++n}`;
})();

describe("insertText", () => {
	let id: string;
	let target: HarnessInputTarget;
	let calls: string[];

	beforeEach(() => {
		id = nextId();
		calls = [];
		invokeMock.mockReset();
		target = {
			paste: vi.fn(() => calls.push("paste")),
			bracketedPaste: () => false,
		};
		// A live store entry, so the local activity-record check passes.
		mirrorSet(id, "running");
	});

	it("pastes but never submits, when the daemon allows it", async () => {
		invokeMock.mockResolvedValue({ ok: true });
		harnessInput.register(id, target);

		const result = await insertText(id, "claude", "/tmp/dropped.txt ");

		expect(result).toEqual({ ok: true });
		expect(calls).toEqual(["paste"]);
		expect(invokeMock).toHaveBeenCalledWith("harness_gate_check", {
			harnessId: id,
			body: null,
			mode: "insert",
		});
	});

	it("refuses — and never pastes — a harness that never registered", async () => {
		const result = await insertText(id, "claude", "/tmp/dropped.txt ");
		expect(result.ok).toBe(false);
		expect(target.paste).not.toHaveBeenCalled();
		expect(invokeMock).not.toHaveBeenCalled();
	});

	it("never pastes when the daemon refuses", async () => {
		invokeMock.mockResolvedValue({ ok: false, code: "permission", reason: "dialog open" });
		harnessInput.register(id, target);

		const result = await insertText(id, "claude", "/tmp/dropped.txt ");
		expect(result).toEqual({ ok: false, reason: "dialog open", code: "permission" });
		expect(target.paste).not.toHaveBeenCalled();
	});
});

// #386: `clientFacts`' once-per-registration trigger — near the other
// `register` tests since this is the same call firing a second effect.
describe("harnessInput.subscribeRegistered (#386)", () => {
	it("fires with the harness id once register() has set the target", () => {
		const id = nextId();
		const target: HarnessInputTarget = {
			paste: vi.fn(),
			bracketedPaste: () => false,
		};
		const seen: string[] = [];
		const unsubscribe = harnessInput.subscribeRegistered((registeredId) => seen.push(registeredId));

		harnessInput.register(id, target);

		expect(seen).toEqual([id]);
		unsubscribe();
	});

	it("stops firing after unsubscribe", () => {
		const id = nextId();
		const target: HarnessInputTarget = {
			paste: vi.fn(),
			bracketedPaste: () => false,
		};
		const seen: string[] = [];
		const unsubscribe = harnessInput.subscribeRegistered((registeredId) => seen.push(registeredId));
		unsubscribe();

		harnessInput.register(id, target);

		expect(seen).toEqual([]);
	});
});

// #733: what `clientFacts` listens to — any draft event or user-input bump.
describe("harnessInput.subscribeFacts (#733)", () => {
	it("fires for a draft event and for a user-input bump, and stops after unsubscribe", () => {
		const id = nextId();
		const seen: string[] = [];
		const off = harnessInput.subscribeFacts((facts) => seen.push(facts));
		harnessInput.noteDraftEvent(id, { type: "key", key: "a" });
		harnessInput.noteUserInput(id);
		expect(seen).toEqual([id, id]);
		off();
		harnessInput.noteUserInput(id);
		expect(seen).toHaveLength(2);
	});

	it("a daemon seam event moves the draft without counting as user input", () => {
		const id = nextId();
		harnessInput.noteDraftEvent(id, { type: "seamPaste" });
		expect(harnessInput.draft(id).kind).toBe("unknown");
		harnessInput.noteDraftEvent(id, { type: "seamSubmit" });
		expect(harnessInput.draft(id).kind).toBe("clean");
		expect(harnessInput.userInputCount(id)).toBe(0);
	});
});
