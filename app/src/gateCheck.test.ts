import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

import { checkGate } from "./gateCheck.ts";

beforeEach(() => {
	invokeMock.mockReset();
});

describe("checkGate", () => {
	it("passes the arguments through and maps ok", async () => {
		invokeMock.mockResolvedValue({ ok: true });
		expect(await checkGate("h", "hi", "send")).toEqual({ ok: true });
		expect(invokeMock).toHaveBeenCalledWith("harness_gate_check", {
			harnessId: "h",
			body: "hi",
			mode: "send",
		});
	});

	it("maps a refusal with its code", async () => {
		invokeMock.mockResolvedValue({ ok: false, code: "busy", reason: "typing" });
		expect(await checkGate("h", "hi", "send")).toEqual({
			ok: false,
			reason: "typing",
			code: "busy",
		});
	});

	it("a failed send check reads as allowed (display hint only)", async () => {
		invokeMock.mockImplementation(() => Promise.reject(new Error("boom")));
		expect(await checkGate("h", "hi", "send")).toEqual({ ok: true });
	});

	it("a failed insert check refuses with a plain reason", async () => {
		invokeMock.mockImplementation(() => Promise.reject(new Error("boom")));
		const r = await checkGate("h", null, "insert");
		expect(r.ok).toBe(false);
		if (!r.ok) expect(r.reason.length).toBeGreaterThan(0);
	});
});
