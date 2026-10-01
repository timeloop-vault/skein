import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { harnessActivity } from "./harnessActivity.ts";
import type { HarnessInputTarget } from "./harnessInput.ts";
import { harnessInput, insertText, sendPrompt } from "./harnessInput.ts";
import { SUBMIT_GAP_MS } from "./submitRetry.ts";

const nextId = (() => {
	let n = 0;
	return () => `h_${++n}`;
})();

describe("sendPrompt", () => {
	let id: string;
	let target: HarnessInputTarget;
	let calls: string[];

	beforeEach(() => {
		// #380: paste and submit are no longer the same tick — every
		// test here has to move the clock past `SUBMIT_GAP_MS` to see
		// the submit land. Fake timers, not file-scope, so the rest of
		// this file's describes (built well before #380) are untouched.
		vi.useFakeTimers();
		id = nextId();
		calls = [];
		target = {
			paste: vi.fn(() => calls.push("paste")),
			bracketedPaste: () => false,
			submit: vi.fn(() => calls.push("submit")),
		};
		// Bring a real store entry to the one state the gate allows:
		// waiting, authoritative, heard-from, injected.
		harnessActivity.spawned(id);
		harnessActivity.attachAuthoritativeSource(id);
		harnessActivity.adapterDelivered(id);
		harnessActivity.setWaitingFromAdapter(id, "test");
		harnessActivity.setInjected(id, true);
	});

	afterEach(() => {
		vi.useRealTimers();
	});

	it("pastes immediately, then submits only after the #380 gap — not in the same tick", () => {
		harnessInput.register(id, target);

		const result = sendPrompt(id, "claude", "do the thing");

		expect(result).toEqual({ ok: true });
		expect(calls).toEqual(["paste"]);

		vi.advanceTimersByTime(SUBMIT_GAP_MS);
		expect(calls).toEqual(["paste", "submit"]);
	});

	it("refuses — and never pastes — a harness that never registered", () => {
		const result = sendPrompt(id, "claude", "do the thing");
		expect(result.ok).toBe(false);
		expect(target.paste).not.toHaveBeenCalled();
	});

	it("re-checks the gate at call time rather than trusting a stale render", () => {
		harnessInput.register(id, target);
		harnessActivity.setRunningFromAdapter(id, "test");

		const result = sendPrompt(id, "claude", "do the thing");
		expect(result.ok).toBe(false);
		expect(target.paste).not.toHaveBeenCalled();
	});

	it("pastes a multi-line body as one paste call, then submits once, when bracketed paste is on", () => {
		const bracketedTarget: HarnessInputTarget = {
			paste: vi.fn(() => calls.push("paste")),
			bracketedPaste: () => true,
			submit: vi.fn(() => calls.push("submit")),
		};
		harnessInput.register(id, bracketedTarget);
		const body = Array.from({ length: 30 }, (_, i) => `line ${i}`).join("\n");

		const result = sendPrompt(id, "claude", body);
		vi.advanceTimersByTime(SUBMIT_GAP_MS);

		expect(result).toEqual({ ok: true });
		expect(bracketedTarget.paste).toHaveBeenCalledTimes(1);
		expect(bracketedTarget.paste).toHaveBeenCalledWith(body);
		expect(bracketedTarget.submit).toHaveBeenCalledTimes(1);
		expect(calls).toEqual(["paste", "submit"]);
	});
});
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
			submit: vi.fn(() => calls.push("submit")),
		};
		// Bring a real store entry to a state canInsertText allows —
		// mid-turn (`running`), authoritative, heard-from, injected.
		harnessActivity.spawned(id);
		harnessActivity.attachAuthoritativeSource(id);
		harnessActivity.adapterDelivered(id);
		harnessActivity.setRunningFromAdapter(id, "test");
		harnessActivity.setInjected(id, true);
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
		harnessActivity.exited(id, 0);

		const result = insertText(id, "claude", "/tmp/dropped.txt ");
		expect(result.ok).toBe(false);
		expect(target.paste).not.toHaveBeenCalled();
	});
});

// #386: `useMailDelivery.ts`'s seam-registered trigger — near the other
// `register` tests since this is the same call firing a second effect.
describe("harnessInput.subscribeRegistered (#386)", () => {
	it("fires with the harness id once register() has set the target", () => {
		const id = nextId();
		const target: HarnessInputTarget = {
			paste: vi.fn(),
			bracketedPaste: () => false,
			submit: vi.fn(),
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
			submit: vi.fn(),
		};
		const seen: string[] = [];
		const unsubscribe = harnessInput.subscribeRegistered((registeredId) => seen.push(registeredId));
		unsubscribe();

		harnessInput.register(id, target);

		expect(seen).toEqual([]);
	});
});
