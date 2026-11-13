import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

// #363 — `sendPrompt` (the #238/#327/#330 seam: nudges, mail delivery,
// create_room's first prompt) must tell the daemon a prompt was submitted
// the way a typed Enter does, since none of those callers ever touch
// xterm's `onKey`. The daemon's phase machine arms the #259 watchdog on it
// (#732); here the webview side is checked: the input is forwarded once,
// after the #380 gap, and a harness stuck on a dialog is refused.

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

import type { HarnessInputTarget } from "./harnessInput.ts";
import { harnessInput, sendPrompt } from "./harnessInput.ts";
import { LAUNCH_QUIET_MS } from "./launchReady.ts";
import { mirrorSet } from "./phaseTestSupport.ts";
import { SUBMIT_GAP_MS } from "./submitRetry.ts";

beforeAll(() => {
	vi.useFakeTimers();
});
afterAll(() => {
	vi.useRealTimers();
});
beforeEach(() => {
	h.calls.length = 0;
});

const nextId = (() => {
	let n = 0;
	return () => `si_${++n}`;
})();

const forwarded = () => h.calls.filter((c) => c.cmd === "harness_phase_input").map((c) => c.args);

/// A registered, sendable harness whose only proof of life is the launch
/// signal (#273): `adapterHeard` stays false.
const sendableHarness = (): { id: string; target: HarnessInputTarget } => {
	const id = nextId();
	const target: HarnessInputTarget = {
		paste: vi.fn(),
		bracketedPaste: () => false,
		submit: vi.fn(),
	};
	mirrorSet(id, "waiting", {
		authoritative: true,
		launchSignalAt: Date.now(),
		injected: true,
	});
	harnessInput.register(id, target);
	return { id, target };
};

describe("sendPrompt tells the daemon about the submit (#363)", () => {
	it("forwards promptSubmitted once, with the submit that lands after the #380 gap", () => {
		const { id, target } = sendableHarness();
		vi.advanceTimersByTime(LAUNCH_QUIET_MS);

		expect(sendPrompt(id, "claude", "do the thing")).toEqual({ ok: true });
		expect(forwarded()).toEqual([]);

		vi.advanceTimersByTime(SUBMIT_GAP_MS);
		expect(target.submit).toHaveBeenCalledTimes(1);
		expect(forwarded()).toEqual([{ harnessId: id, input: { kind: "promptSubmitted" } }]);
	});

	it("refuses — and never pastes or submits — a harness stuck on a permission dialog", () => {
		const { id, target } = sendableHarness();
		mirrorSet(id, "permission", {
			authoritative: true,
			launchSignalAt: Date.now(),
			injected: true,
			permissionTool: "Bash",
			permissionAgentType: "agent-1",
		});

		const result = sendPrompt(id, "claude", "anything");
		expect(result.ok).toBe(false);
		expect(target.paste).not.toHaveBeenCalled();
		expect(target.submit).not.toHaveBeenCalled();
		expect(forwarded()).toEqual([]);
	});
});
