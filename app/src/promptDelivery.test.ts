import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import { deliverMailNow, sendPrompt } from "./harnessInput.ts";
import { mapDeliveryError, mapDeliveryResult, UNKNOWN_REFUSAL } from "./promptDelivery.ts";

describe("mapDeliveryResult", () => {
	it("maps ok", () => {
		expect(mapDeliveryResult({ ok: true })).toEqual({ ok: true });
	});

	it("maps a refusal to its reason, dropping the code", () => {
		expect(mapDeliveryResult({ ok: false, code: "not_ready", reason: "busy" })).toEqual({
			ok: false,
			reason: "busy",
		});
	});

	it.each([[null], [undefined], ["ok"], [{}], [{ ok: false }], [{ ok: false, reason: "" }]])(
		"refuses on an unusable answer %j",
		(raw) => {
			expect(mapDeliveryResult(raw)).toEqual({ ok: false, reason: UNKNOWN_REFUSAL });
		},
	);
});

describe("mapDeliveryError", () => {
	it("turns a rejection into a refusal", () => {
		expect(mapDeliveryError("daemon gone")).toEqual({ ok: false, reason: "daemon gone" });
		expect(mapDeliveryError(new Error("boom"))).toEqual({ ok: false, reason: "boom" });
	});
});

describe("sendPrompt / deliverMailNow", () => {
	beforeEach(() => {
		invoke.mockReset();
	});

	it("sendPrompt invokes harness_send_prompt and maps the answer", async () => {
		invoke.mockResolvedValue({ ok: false, code: "gate", reason: "turn running" });
		const r = await sendPrompt("h1", "do it");
		expect(invoke).toHaveBeenCalledWith("harness_send_prompt", { harnessId: "h1", body: "do it" });
		expect(r).toEqual({ ok: false, reason: "turn running" });
	});

	it("sendPrompt maps a rejected invoke to a refusal", async () => {
		invoke.mockImplementation(() => Promise.reject(new Error("no such command")));
		expect(await sendPrompt("h1", "x")).toEqual({ ok: false, reason: "no such command" });
	});

	it("deliverMailNow invokes mail_deliver_now", async () => {
		invoke.mockResolvedValue({ ok: true });
		expect(await deliverMailNow("h2")).toEqual({ ok: true });
		expect(invoke).toHaveBeenCalledWith("mail_deliver_now", { harnessId: "h2" });
	});
});
