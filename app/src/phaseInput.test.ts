import { beforeEach, describe, expect, it, vi } from "vitest";

// #732: client-only phase inputs are forwarded to the daemon's shadow machine.

const h = vi.hoisted(() => ({
	calls: [] as Array<{ cmd: string; args: Record<string, unknown> }>,
}));

vi.mock("@tauri-apps/api/core", () => ({
	Channel: class {
		onmessage: (e: unknown) => void = () => {};
	},
	invoke: (cmd: string, args: Record<string, unknown>) => {
		h.calls.push({ cmd, args });
		return Promise.resolve();
	},
}));
vi.mock("./frontendLog.ts", () => ({ logBoth: vi.fn(), logToRust: vi.fn() }));

import { harnessActivity } from "./harnessActivity.ts";
import type { HarnessActivity } from "./harnessActivityTypes.ts";
import { noteViewAttached } from "./phaseInput.ts";
import { forgetScreenPhase, setScreenPhaseReader, tickScreenPhase } from "./screenPhaseTick.ts";

const forwarded = () => h.calls.filter((c) => c.cmd === "harness_phase_input").map((c) => c.args);

beforeEach(() => {
	h.calls.length = 0;
});

describe("store methods forward their phase input", () => {
	it("recordInput", () => {
		harnessActivity.recordInput("a", "x", { emptyComposer: true });
		harnessActivity.recordInput("a", "y");
		expect(forwarded()).toEqual([
			{ harnessId: "a", input: { kind: "input", data: "x", emptyComposer: true } },
			{ harnessId: "a", input: { kind: "input", data: "y", emptyComposer: false } },
		]);
	});

	it.each([
		["notePromptSubmitted", "promptSubmitted"],
		["muteInducedOutput", "muteInducedOutput"],
		["reattached", "reattached"],
		["recoverUnheardSilence", "recoverUnheardSilence"],
	] as const)("%s", (method, kind) => {
		harnessActivity[method]("b");
		expect(forwarded()).toEqual([{ harnessId: "b", input: { kind } }]);
	});

	it("forget sends viewReset after the local forget", () => {
		harnessActivity.spawned("f");
		h.calls.length = 0;
		harnessActivity.forget("f");
		expect(harnessActivity.get("f")).toBeNull();
		expect(forwarded()).toEqual([{ harnessId: "f", input: { kind: "viewReset" } }]);
	});

	it("noteLaunchSignal through the store does not forward", () => {
		harnessActivity.noteLaunchSignal("c");
		expect(forwarded()).toEqual([]);
	});
});

describe("noteViewAttached", () => {
	it("sends viewReset only for a process this page did not start", () => {
		noteViewAttached("v", true);
		expect(forwarded()).toEqual([]);
		noteViewAttached("v", false);
		expect(forwarded()).toEqual([{ harnessId: "v", input: { kind: "viewReset" } }]);
	});
});

describe("screen reading", () => {
	const act = (): HarnessActivity => harnessActivity.get("s") as HarnessActivity;

	it("forwards on change only, and null on forget", () => {
		harnessActivity.spawned("s");
		// A running phase so the tick reads the screen.
		harnessActivity.setRunningFromAdapter("s", "test" as never);
		let reading: "waiting" | "running" | "unknown" = "running";
		setScreenPhaseReader(() => reading);
		const set = vi.fn();
		const get = () => harnessActivity.get("s") ?? undefined;
		const tick = () => tickScreenPhase("s", act(), set, get);
		h.calls.length = 0;

		tick();
		tick();
		reading = "unknown";
		tick();
		reading = "running";
		tick();
		forgetScreenPhase("s");
		forgetScreenPhase("s");

		expect(forwarded().map((a) => (a.input as { reading: unknown }).reading)).toEqual([
			"running",
			"unknown",
			"running",
			null,
		]);
		setScreenPhaseReader(null);
		harnessActivity.forget("s");
	});
});
