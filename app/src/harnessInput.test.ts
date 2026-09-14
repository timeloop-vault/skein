import { beforeEach, describe, expect, it, vi } from "vitest";
import { HARNESS_KINDS } from "./data.tsx";
import { harnessActivity } from "./harnessActivity.ts";
import type { HarnessActivity } from "./harnessActivity.ts";
import { canSendPrompt, harnessInput, sendPrompt } from "./harnessInput.ts";
import type { CanSendPromptInput, HarnessInputTarget } from "./harnessInput.ts";

// Pure `canSendPrompt` tests build the input by hand — no store, no
// DOM, no xterm — exactly what the gate is supposed to allow.

const nextId = (() => {
	let n = 0;
	return () => `h_${++n}`;
})();

const activity = (over: Partial<HarnessActivity> = {}): HarnessActivity => ({
	phase: "waiting",
	lastOutputAt: null,
	exitCode: null,
	spawnedAt: 0,
	hasUserInput: false,
	authoritative: true,
	tail: "",
	permissionTool: null,
	adapterHeard: true,
	promptSubmittedAt: null,
	adapterSilent: false,
	injected: true,
	...over,
});

const baseInput = (over: Partial<CanSendPromptInput> = {}): CanSendPromptInput => ({
	capabilities: HARNESS_KINDS.claude.capabilities,
	activity: activity(),
	registered: true,
	bracketedPasteOn: false,
	body: "nudge",
	...over,
});

describe("canSendPrompt", () => {
	it("allows a single-line prompt to a waiting, verified, injected, registered harness", () => {
		expect(canSendPrompt(baseInput())).toEqual({ ok: true });
	});

	it("refuses a kind with no terminal", () => {
		const r = canSendPrompt(baseInput({ capabilities: HARNESS_KINDS.files.capabilities }));
		expect(r).toEqual({ ok: false, reason: expect.stringContaining("no terminal") });
	});

	it("refuses a harness that hasn't registered a live terminal", () => {
		const r = canSendPrompt(baseInput({ registered: false }));
		expect(r.ok).toBe(false);
	});

	it("refuses when there's no activity record at all", () => {
		const r = canSendPrompt(baseInput({ activity: null }));
		expect(r.ok).toBe(false);
	});

	it("gives permission its own reason rather than a generic 'not waiting'", () => {
		const r = canSendPrompt(baseInput({ activity: activity({ phase: "permission" }) }));
		expect(r).toEqual({ ok: false, reason: expect.stringContaining("permission dialog") });
	});

	it("refuses any other non-waiting phase", () => {
		const r = canSendPrompt(baseInput({ activity: activity({ phase: "running" }) }));
		expect(r.ok).toBe(false);
		if (!r.ok) expect(r.reason).toContain("running");
	});

	it("refuses without a source that has proven it's watching the harness", () => {
		expect(canSendPrompt(baseInput({ activity: activity({ authoritative: false }) })).ok).toBe(
			false,
		);
		expect(canSendPrompt(baseInput({ activity: activity({ adapterHeard: false }) })).ok).toBe(
			false,
		);
		expect(canSendPrompt(baseInput({ activity: activity({ adapterSilent: true }) })).ok).toBe(
			false,
		);
	});

	it("refuses a harness that wasn't spawned with config injection", () => {
		const r = canSendPrompt(baseInput({ activity: activity({ injected: false }) }));
		expect(r).toEqual({ ok: false, reason: expect.stringContaining("review tools") });
	});

	it("refuses a multi-line body when bracketed paste is off", () => {
		const r = canSendPrompt(baseInput({ body: "line one\nline two", bracketedPasteOn: false }));
		expect(r).toEqual({ ok: false, reason: expect.stringContaining("paste mode") });
	});

	it("allows a multi-line body when bracketed paste is on", () => {
		const r = canSendPrompt(baseInput({ body: "line one\nline two", bracketedPasteOn: true }));
		expect(r).toEqual({ ok: true });
	});
});

describe("sendPrompt", () => {
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
		// Bring a real store entry to the one state the gate allows:
		// waiting, authoritative, heard-from, injected.
		harnessActivity.spawned(id);
		harnessActivity.attachAuthoritativeSource(id);
		harnessActivity.adapterDelivered(id);
		harnessActivity.setWaitingFromAdapter(id, "test");
		harnessActivity.setInjected(id, true);
	});

	it("pastes then submits, in that order, when the gate allows it", () => {
		harnessInput.register(id, target);

		const result = sendPrompt(id, "claude", "do the thing");

		expect(result).toEqual({ ok: true });
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
});
