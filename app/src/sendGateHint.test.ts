import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { GateResult } from "./harnessInputGate.ts";
import { createSendGateHint, MULTI_PROBE, SINGLE_PROBE } from "./sendGateHint.ts";

const flush = async () => {
	for (let i = 0; i < 5; i++) await Promise.resolve();
};

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("createSendGateHint", () => {
	it("reads ok before the first answer", () => {
		const hint = createSendGateHint(() => new Promise(() => {}));
		hint.refresh();
		expect(hint.get("a")).toEqual({ ok: true });
		expect(hint.get("a\nb")).toEqual({ ok: true });
	});

	it("picks the single- or multi-line answer by newline", async () => {
		const ask = vi.fn(
			async (body: string): Promise<GateResult> =>
				body === MULTI_PROBE
					? { ok: false, reason: "paste mode", code: "needsPasteMode" }
					: { ok: true },
		);
		const hint = createSendGateHint(ask);
		hint.refresh();
		await flush();
		expect(ask).toHaveBeenCalledWith(SINGLE_PROBE);
		expect(hint.get("one line")).toEqual({ ok: true });
		expect(hint.get("two\nlines").ok).toBe(false);
	});

	it("drops an out-of-order answer", async () => {
		const resolvers: Array<(r: GateResult) => void> = [];
		const hint = createSendGateHint(
			() => new Promise<GateResult>((resolve) => resolvers.push(resolve)),
		);
		hint.refresh(); // asks 0 (single), 1 (multi)
		hint.refresh(); // asks 2, 3
		const stale: GateResult = { ok: false, reason: "stale", code: "notReady" };
		resolvers[2]?.({ ok: true });
		resolvers[3]?.({ ok: true });
		await flush();
		resolvers[0]?.(stale);
		resolvers[1]?.(stale);
		await flush();
		expect(hint.get("a")).toEqual({ ok: true });
	});

	it("re-polls every 2 s while the answer is settling, then stops", async () => {
		let code = "settling";
		const ask = vi.fn(
			async (): Promise<GateResult> =>
				code === "ok" ? { ok: true } : { ok: false, reason: "wait", code },
		);
		const hint = createSendGateHint(ask);
		hint.refresh();
		await flush();
		expect(ask).toHaveBeenCalledTimes(2);
		await vi.advanceTimersByTimeAsync(2000);
		expect(ask).toHaveBeenCalledTimes(4);
		code = "ok";
		await vi.advanceTimersByTimeAsync(2000);
		expect(ask).toHaveBeenCalledTimes(6);
		await vi.advanceTimersByTimeAsync(10000);
		expect(ask).toHaveBeenCalledTimes(6);
		hint.dispose();
	});

	it("re-polls every 5 s for any other refusal", async () => {
		const ask = vi.fn(
			async (): Promise<GateResult> => ({ ok: false, reason: "no", code: "notAtStoppingPoint" }),
		);
		const hint = createSendGateHint(ask);
		hint.refresh();
		await flush();
		await vi.advanceTimersByTimeAsync(4900);
		expect(ask).toHaveBeenCalledTimes(2);
		await vi.advanceTimersByTimeAsync(100);
		expect(ask).toHaveBeenCalledTimes(4);
		hint.dispose();
		await vi.advanceTimersByTimeAsync(20000);
		expect(ask).toHaveBeenCalledTimes(4);
	});

	it("sets no timer while the answer is ok", async () => {
		const ask = vi.fn(async (): Promise<GateResult> => ({ ok: true }));
		const hint = createSendGateHint(ask);
		hint.refresh();
		await flush();
		expect(vi.getTimerCount()).toBe(0);
		hint.dispose();
	});

	it("stops asking and notifying after dispose", async () => {
		const ask = vi.fn(async (): Promise<GateResult> => ({ ok: false, reason: "w", code: "busy" }));
		const hint = createSendGateHint(ask);
		const seen = vi.fn();
		hint.subscribe(seen);
		hint.refresh();
		await flush();
		expect(seen).toHaveBeenCalledTimes(1);
		hint.dispose();
		expect(vi.getTimerCount()).toBe(0);
		await vi.advanceTimersByTimeAsync(10000);
		hint.refresh();
		expect(ask).toHaveBeenCalledTimes(2);
	});
});
