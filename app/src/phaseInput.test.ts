import { beforeEach, describe, expect, it, vi } from "vitest";

// #732: client-only phase inputs are forwarded to the daemon, exactly once.

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
import { sendPhaseInput } from "./phaseInput.ts";
import { resetPhaseMirror } from "./phaseMirror.ts";
import { mirrorForget, mirrorSet } from "./phaseTestSupport.ts";
import { setScreenPhaseReader, tickScreenReadings } from "./screenPhaseTick.ts";

const forwarded = () => h.calls.filter((c) => c.cmd === "harness_phase_input").map((c) => c.args);

beforeEach(() => {
	h.calls.length = 0;
	resetPhaseMirror();
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
		["recoverUnheardSilence", "recoverUnheardSilence"],
		["noteLaunchSignal", "launchSignal"],
	] as const)("%s", (method, kind) => {
		harnessActivity[method]("b");
		expect(forwarded()).toEqual([{ harnessId: "b", input: { kind } }]);
	});

	it.each([
		[{ kind: "startFailed" }],
		[{ kind: "restartRequested", source: "user-restart" }],
		[{ kind: "restartRequested", source: "version-auto-restart" }],
	] as const)("sendPhaseInput passes %j through unchanged", (input) => {
		sendPhaseInput("e", input);
		expect(forwarded()).toEqual([{ harnessId: "e", input }]);
	});

	it("sessionSwitched", () => {
		harnessActivity.sessionSwitched("d", "fork");
		expect(forwarded()).toEqual([
			{ harnessId: "d", input: { kind: "sessionSwitched", switch: "fork" } },
		]);
	});
});

describe("screen reading feed", () => {
	it("forwards on change only, for running or waiting non-authoritative harnesses, and null on forget", () => {
		mirrorSet("s", "running");
		mirrorSet("auth", "running", { authoritative: true });
		mirrorSet("idle", "idle");
		let reading: "waiting" | "running" | "unknown" = "running";
		setScreenPhaseReader(() => reading);
		h.calls.length = 0;

		tickScreenReadings();
		tickScreenReadings();
		reading = "unknown";
		tickScreenReadings();
		reading = "running";
		tickScreenReadings();
		mirrorForget("s");
		tickScreenReadings();
		tickScreenReadings();
		setScreenPhaseReader(null);

		expect(forwarded().every((a) => a.harnessId === "s")).toBe(true);
		expect(forwarded().map((a) => (a.input as { reading: unknown }).reading)).toEqual([
			"running",
			"unknown",
			"running",
			null,
		]);
	});
});
