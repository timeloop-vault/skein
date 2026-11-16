import { beforeEach, describe, expect, it, vi } from "vitest";
import type { HarnessInputTarget } from "./harnessInput.ts";
import { harnessInput, insertText } from "./harnessInput.ts";
import { mirrorSet, SENDABLE } from "./phaseTestSupport.ts";

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
		target = {
			paste: vi.fn(() => calls.push("paste")),
			bracketedPaste: () => false,
		};
		// Bring a real store entry to a state canInsertText allows —
		// mid-turn (`running`), authoritative, heard-from, injected.
		mirrorSet(id, "running", SENDABLE);
	});

	it("pastes but never submits, when the gate allows it", () => {
		harnessInput.register(id, target);

		const result = insertText(id, "claude", "/tmp/dropped.txt ");

		expect(result).toEqual({ ok: true });
		expect(calls).toEqual(["paste"]);
	});

	it("refuses — and never pastes — a harness that never registered", () => {
		const result = insertText(id, "claude", "/tmp/dropped.txt ");
		expect(result.ok).toBe(false);
		expect(target.paste).not.toHaveBeenCalled();
	});

	it("re-checks the gate at call time rather than trusting a stale render", () => {
		harnessInput.register(id, target);
		mirrorSet(id, "exited", { ...SENDABLE });

		const result = insertText(id, "claude", "/tmp/dropped.txt ");
		expect(result.ok).toBe(false);
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
