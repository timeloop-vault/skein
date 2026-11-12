import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
const drainRoomsBounded = vi.fn();
vi.mock("./roomsFlush.ts", () => ({ drainRoomsBounded: () => drainRoomsBounded() }));

import {
	defaultEnsureDeps,
	detachHarness,
	ENSURE_ATTEMPTS,
	ENSURE_BACKOFF_MS,
	type EnsureArgs,
	type EnsureDeps,
	type EnsureResult,
	ensureRunning,
} from "./harnessEnsure.ts";
import { resetMounts, restartFor } from "./harnessMounts.ts";

const args = {
	roomId: "r1",
	harnessId: "h1",
	rows: 24,
	cols: 80,
	envProfile: undefined,
	restart: true,
	mountKey: "k1",
	expectCmd: ["claude", "--x"],
	onEvent: {},
} as unknown as EnsureArgs;

const ok = { started: true, id: "p1" } as unknown as EnsureResult;

function deps(over: Partial<EnsureDeps> = {}) {
	return {
		drain: vi.fn(async () => {}),
		call: vi.fn(async () => ok),
		sleep: vi.fn(async (_ms: number) => {}),
		isCancelled: vi.fn(() => false),
		...over,
	};
}

beforeEach(() => {
	invoke.mockReset();
	drainRoomsBounded.mockReset();
});

describe("ensureRunning", () => {
	it("drains, then calls with expectCmd and a null envProfile", async () => {
		const order: string[] = [];
		const d = deps({
			drain: vi.fn(async () => void order.push("drain")),
			call: vi.fn(async () => {
				order.push("call");
				return ok;
			}),
		});
		await expect(ensureRunning(args, d)).resolves.toBe(ok);
		expect(order).toEqual(["drain", "call"]);
		expect(d.call).toHaveBeenCalledWith({
			roomId: "r1",
			harnessId: "h1",
			rows: 24,
			cols: 80,
			envProfile: null,
			restart: true,
			expectCmd: ["claude", "--x"],
			onEvent: args.onEvent,
		});
	});

	it.each(["stale_record", "unknown_harness"])("retries on %s then succeeds", async (code) => {
		const call = vi
			.fn()
			.mockRejectedValueOnce(`${code}: nope`)
			.mockRejectedValueOnce(new Error(`${code}: nope`))
			.mockResolvedValue(ok);
		const d = deps({ call });
		await expect(ensureRunning(args, d)).resolves.toBe(ok);
		expect(call).toHaveBeenCalledTimes(3);
		expect(d.drain).toHaveBeenCalledTimes(3);
		expect(vi.mocked(d.sleep).mock.calls).toEqual([[ENSURE_BACKOFF_MS], [ENSURE_BACKOFF_MS * 2]]);
	});

	it("gives up after 4 attempts and throws the last error", async () => {
		const call = vi.fn().mockRejectedValue("stale_record: still stale");
		const d = deps({ call });
		await expect(ensureRunning(args, d)).rejects.toThrow("stale_record: still stale");
		expect(ENSURE_ATTEMPTS).toBe(4);
		expect(call).toHaveBeenCalledTimes(4);
		expect(d.drain).toHaveBeenCalledTimes(4);
		expect(d.sleep).toHaveBeenCalledTimes(3);
	});

	it("throws other errors at once, without retry", async () => {
		const call = vi.fn().mockRejectedValue("spawn_failed: boom");
		const d = deps({ call });
		await expect(ensureRunning(args, d)).rejects.toThrow("spawn_failed: boom");
		expect(call).toHaveBeenCalledTimes(1);
		expect(d.sleep).not.toHaveBeenCalled();
	});

	it("does not retry a code that merely ends in a retryable one", async () => {
		const call = vi.fn().mockRejectedValue("not_stale_record: x");
		await expect(ensureRunning(args, deps({ call }))).rejects.toThrow();
		expect(call).toHaveBeenCalledTimes(1);
	});

	it("returns null without calling when cancelled after the drain", async () => {
		const d = deps({ isCancelled: vi.fn(() => true) });
		await expect(ensureRunning(args, d)).resolves.toBeNull();
		expect(d.call).not.toHaveBeenCalled();
	});

	it("commits the mount key only once the daemon has acted on the call", async () => {
		resetMounts();
		const cancelled = deps({ isCancelled: vi.fn(() => true) });
		await ensureRunning(args, cancelled);
		expect(restartFor("h1", "k2")).toBe(false); // nothing was committed
		const order: string[] = [];
		const d = deps({
			call: vi.fn(async () => {
				order.push(restartFor("h1", "k2") ? "committed" : "not-committed");
				return ok;
			}),
		});
		await ensureRunning(args, d);
		expect(order).toEqual(["not-committed"]); // not yet, during the invoke
		expect(restartFor("h1", "k2")).toBe(true); // k1 committed after success
		expect(restartFor("h1", "k1")).toBe(false);
	});

	it("leaves the key uncommitted when every attempt is refused with stale_record", async () => {
		resetMounts();
		await ensureRunning(args, deps()); // commit "k0"-like baseline: k1
		const again = { ...args, mountKey: "k2", restart: true } as EnsureArgs;
		const call = vi.fn().mockRejectedValue("stale_record: x");
		await expect(ensureRunning(again, deps({ call }))).rejects.toThrow();
		expect(restartFor("h1", "k2")).toBe(true); // still asks for a restart
	});

	it("commits the key on spawn_failed", async () => {
		resetMounts();
		await ensureRunning(args, deps());
		const again = { ...args, mountKey: "k2" } as EnsureArgs;
		const call = vi.fn().mockRejectedValue("spawn_failed: boom");
		await expect(ensureRunning(again, deps({ call }))).rejects.toThrow();
		expect(restartFor("h1", "k2")).toBe(false);
	});

	it("commits the key when the result arrives after the effect was cancelled", async () => {
		resetMounts();
		await ensureRunning(args, deps());
		const again = { ...args, mountKey: "k2" } as EnsureArgs;
		let cancelled = false;
		const call = vi.fn(async () => {
			cancelled = true;
			return ok;
		});
		await ensureRunning(again, deps({ call, isCancelled: () => cancelled }));
		expect(restartFor("h1", "k2")).toBe(false);
	});

	it("returns null without calling again when cancelled during the backoff", async () => {
		let cancelled = false;
		const call = vi.fn().mockRejectedValue("stale_record: x");
		const d = deps({
			call,
			isCancelled: () => cancelled,
			sleep: vi.fn(async () => {
				cancelled = true;
			}),
		});
		await expect(ensureRunning(args, d)).resolves.toBeNull();
		expect(call).toHaveBeenCalledTimes(1);
	});
});

describe("defaultEnsureDeps", () => {
	it("calls harness_ensure_running through invoke and drains via drainRoomsBounded", async () => {
		invoke.mockResolvedValue(ok);
		drainRoomsBounded.mockResolvedValue(undefined);
		const d = defaultEnsureDeps(() => false);
		await d.drain();
		expect(drainRoomsBounded).toHaveBeenCalledTimes(1);
		await expect(d.call({ a: 1 })).resolves.toBe(ok);
		expect(invoke).toHaveBeenCalledWith("harness_ensure_running", { a: 1 });
	});
});

describe("detachHarness", () => {
	it("invokes harness_detach with the id and attach id", () => {
		invoke.mockResolvedValue(undefined);
		detachHarness("h1", 7);
		expect(invoke).toHaveBeenCalledTimes(1);
		expect(invoke).toHaveBeenCalledWith("harness_detach", { harnessId: "h1", attachId: 7 });
	});

	it("sends null for a missing attach id", () => {
		invoke.mockResolvedValue(undefined);
		detachHarness("h1", undefined);
		expect(invoke).toHaveBeenCalledWith("harness_detach", { harnessId: "h1", attachId: null });
	});

	it("swallows a rejection with a warning", async () => {
		const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
		invoke.mockRejectedValue(new Error("gone"));
		detachHarness("h1", 1);
		await new Promise((r) => setTimeout(r, 0));
		expect(warn).toHaveBeenCalled();
		warn.mockRestore();
	});
});
