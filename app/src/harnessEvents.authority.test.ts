import { beforeEach, describe, expect, it, vi } from "vitest";

// #422 — attach-generation-aware detach, and authority restored when a
// live adapter speaks after its own `session_end`.

const h = vi.hoisted(() => {
	const channels: Array<{ onmessage: (e: unknown) => void }> = [];
	const invokes: Array<{
		cmd: string;
		resolve: () => void;
		reject: (e: unknown) => void;
	}> = [];
	return { channels, invokes, logBoth: vi.fn(), logToRust: vi.fn() };
});

vi.mock("@tauri-apps/api/core", () => ({
	Channel: class {
		onmessage: (e: unknown) => void = () => {};
		constructor() {
			h.channels.push(this);
		}
	},
	invoke: (cmd: string) =>
		new Promise<void>((resolve, reject) => {
			h.invokes.push({ cmd, resolve, reject });
		}),
}));
vi.mock("./frontendLog.ts", () => ({ logBoth: h.logBoth, logToRust: h.logToRust }));

import { harnessActivity } from "./harnessActivity.ts";
import { store } from "./harnessActivityCore.ts";
import { attachClaudeEvents, attachOpencodeEvents } from "./harnessEvents.ts";

let n = 0;
const fresh = (): string => {
	const id = `ev_${++n}`;
	harnessActivity.spawned(id);
	return id;
};
const auth = (id: string) => harnessActivity.get(id)?.authoritative;
const send = (i: number, event: unknown) => h.channels[i]?.onmessage(event);
const claude = (id: string) => attachClaudeEvents(id, "room", "sid", "/cwd");
const opencode = (id: string) =>
	attachOpencodeEvents(id, "room", "/cwd", 1234, undefined, undefined, () => undefined, undefined);
const attachInvoke = (cmdPrefix: string) => h.invokes.find((i) => i.cmd === cmdPrefix);

beforeEach(() => {
	h.channels.length = 0;
	h.invokes.length = 0;
	h.logBoth.mockClear();
});

describe("claude attach generations", () => {
	it("a late attach rejection of a replaced attach leaves the new one authoritative", async () => {
		const id = fresh();
		const cleanupA = claude(id);
		const rejectA = attachInvoke("claude_events_attach");
		cleanupA();
		claude(id);
		rejectA?.reject(new Error("boom"));
		await Promise.resolve();
		await Promise.resolve();
		expect(auth(id)).toBe(true);
	});

	it("a stale cleanup does not detach the newer attach", () => {
		const id = fresh();
		const cleanupA = claude(id);
		claude(id);
		cleanupA();
		expect(auth(id)).toBe(true);
	});

	it("a session_end on a closed attach's channel does nothing to the new attach", () => {
		const id = fresh();
		const cleanupA = claude(id);
		cleanupA();
		claude(id);
		const phase = harnessActivity.get(id)?.phase;
		send(0, { kind: "session_end" });
		expect(auth(id)).toBe(true);
		expect(harnessActivity.get(id)?.phase).toBe(phase);
	});

	it("a live adapter speaking after session_end takes authority back", () => {
		const id = fresh();
		claude(id);
		send(0, { kind: "session_end" });
		expect(auth(id)).toBe(false);
		send(0, { kind: "awaiting_prompt" });
		expect(auth(id)).toBe(true);
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		const restored = h.logBoth.mock.calls.filter((c) => String(c[2]).includes("#422"));
		expect(restored).toHaveLength(1);
	});
});

describe("claude attach rejection after its own cleanup", () => {
	it("does not throw or touch a harness with no attach", async () => {
		const id = fresh();
		const cleanup = claude(id);
		const attach = attachInvoke("claude_events_attach");
		cleanup();
		attach?.reject(new Error("late"));
		await Promise.resolve();
		await Promise.resolve();
		expect(auth(id)).toBe(false);
	});
});

describe("adapterDelivered guards", () => {
	it("does not re-grant authority once exited", () => {
		const id = fresh();
		claude(id);
		send(0, { kind: "session_end" });
		harnessActivity.exited(id, 0);
		send(0, { kind: "awaiting_prompt" });
		expect(auth(id)).toBe(false);
		expect(h.logBoth.mock.calls.filter((c) => String(c[2]).includes("#422"))).toHaveLength(0);
	});

	it("an adapterSilent harness takes the old recovery branch", () => {
		const id = fresh();
		claude(id);
		const cur = harnessActivity.get(id);
		if (!cur) throw new Error("no entry");
		store.set(id, {
			...cur,
			adapterSilent: true,
			authoritative: false,
			degradedBy: "adapter-silent",
		});
		send(0, { kind: "awaiting_prompt" });
		expect(auth(id)).toBe(true);
		const lines = h.logBoth.mock.calls.map((c) => String(c[2]));
		expect(lines.filter((l) => l.includes("adapter recovered"))).toHaveLength(1);
		expect(lines.filter((l) => l.includes("#422"))).toHaveLength(0);
	});
});

describe("opencode attach generations", () => {
	it("session_end then connected re-arms without a #422 line", () => {
		const id = fresh();
		opencode(id);
		send(0, { kind: "connected" });
		send(0, { kind: "session_end" });
		expect(auth(id)).toBe(false);
		send(0, { kind: "connected" });
		expect(auth(id)).toBe(true);
		expect(h.logBoth.mock.calls.filter((c) => String(c[2]).includes("#422"))).toHaveLength(0);
	});

	it("a stale cleanup does not detach the newer attach", () => {
		const id = fresh();
		const cleanupA = opencode(id);
		opencode(id);
		cleanupA();
		expect(auth(id)).toBe(true);
	});

	it("a session_end from a replaced attach's channel is ignored", () => {
		const id = fresh();
		opencode(id);
		opencode(id);
		send(0, { kind: "session_end" });
		expect(auth(id)).toBe(true);
	});

	it("a session_end on a closed attach's channel is ignored", () => {
		const id = fresh();
		const cleanupA = opencode(id);
		cleanupA();
		opencode(id);
		send(0, { kind: "session_end" });
		expect(auth(id)).toBe(true);
	});
});
