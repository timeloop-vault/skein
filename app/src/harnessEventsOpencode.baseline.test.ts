import { beforeEach, describe, expect, it, vi } from "vitest";

// #175 — the assumed-idle baseline an opencode (re)connect emits is a
// quiet `waiting`, not an observed end of turn.

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

import { harnessActivity, TRANSITION_SOURCE } from "./harnessActivity.ts";
import type { ActivityPhase } from "./harnessActivityTypes.ts";
import { attachOpencodeEvents } from "./harnessEvents.ts";
import { classifyTransition, isNotifiable } from "./harnessNotifyLogic.ts";

let n = 0;
const fresh = (): string => {
	const id = `bl_${++n}`;
	harnessActivity.spawned(id);
	attachOpencodeEvents(id, "room", "/cwd", 1234, undefined, undefined, () => undefined, undefined);
	return id;
};
const send = (event: unknown) => h.channels[h.channels.length - 1]?.onmessage(event);
const phase = (id: string) => harnessActivity.get(id)?.phase;

beforeEach(() => {
	h.channels.length = 0;
});

describe("session_baseline_idle", () => {
	it("moves running to waiting under the baseline source", () => {
		const id = fresh();
		harnessActivity.setRunningFromAdapter(id, TRANSITION_SOURCE.L2c2OpencodeBusy);
		const seen: string[] = [];
		const unsub = harnessActivity.subscribeTransitions((hid, _from, to, source) => {
			if (hid === id) seen.push(`${to}:${source}`);
		});
		send({ kind: "session_baseline_idle" });
		unsub();
		expect(phase(id)).toBe("waiting");
		expect(seen).toEqual([`waiting:${TRANSITION_SOURCE.L2c2OpencodeBaseline}`]);
	});

	it("moves permission to waiting quietly (the reconnect forgot its ids)", () => {
		const id = fresh();
		harnessActivity.setPermissionFromAdapter(id, TRANSITION_SOURCE.L2c2OpencodePermission, null);
		const seen: Array<{ from: ActivityPhase; to: ActivityPhase; source: string }> = [];
		const unsub = harnessActivity.subscribeTransitions((hid, from, to, source) => {
			if (hid === id) seen.push({ from, to, source });
		});
		send({ kind: "connected" });
		send({ kind: "session_baseline_idle" });
		unsub();
		expect(phase(id)).toBe("waiting");
		expect(seen).toEqual([
			{ from: "permission", to: "waiting", source: TRANSITION_SOURCE.L2c2OpencodeBaseline },
		]);
		expect(isNotifiable(classifyTransition("permission", "waiting"), seen[0]?.source)).toBe(false);
	});

	it("does not leave exited", () => {
		const id = fresh();
		harnessActivity.exited(id, 0);
		send({ kind: "session_baseline_idle" });
		expect(phase(id)).toBe("exited");
	});

	it("a real session_idle still reports its own source", () => {
		const id = fresh();
		harnessActivity.setRunningFromAdapter(id, TRANSITION_SOURCE.L2c2OpencodeBusy);
		const seen: string[] = [];
		const unsub = harnessActivity.subscribeTransitions((hid, _from, _to, source) => {
			if (hid === id) seen.push(source);
		});
		send({ kind: "session_idle" });
		unsub();
		expect(seen).toEqual([TRANSITION_SOURCE.L2c2OpencodeIdle]);
	});
});
