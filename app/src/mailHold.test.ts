import { describe, expect, it, vi } from "vitest";
import { mailHold } from "./mailHold.ts";
import { automaticGate, isDraftHold, mailHeld, releaseRefusalReason } from "./mailNudge.ts";

const DRAFT = automaticGate({ ok: true }, { kind: "typed", chars: 3 });
const OTHER = { ok: false, reason: "not ready" } as const;

describe("isDraftHold / mailHeld", () => {
	it("marks only the draft guard's refusal", () => {
		expect(isDraftHold(DRAFT)).toBe(true);
		expect(isDraftHold(OTHER)).toBe(false);
		expect(isDraftHold({ ok: true })).toBe(false);
		expect(isDraftHold(null)).toBe(false);
	});

	it("is held only with undelivered mail and a draft refusal", () => {
		expect(mailHeld({ unread: 2, lastNudged: 0, refusal: DRAFT })).toBe(true);
		expect(mailHeld({ unread: 2, lastNudged: 1, refusal: DRAFT })).toBe(true);
		expect(mailHeld({ unread: 1, lastNudged: 1, refusal: DRAFT })).toBe(false);
		expect(mailHeld({ unread: 0, lastNudged: 0, refusal: DRAFT })).toBe(false);
		expect(mailHeld({ unread: 2, lastNudged: 0, refusal: OTHER })).toBe(false);
		expect(mailHeld({ unread: 2, lastNudged: 0, refusal: null })).toBe(false);
	});
});

describe("releaseRefusalReason", () => {
	it("prefers the gate's own reason, then explains the rest", () => {
		const base = { atStoppingPoint: true, unread: 1, lastNudged: 0 };
		expect(releaseRefusalReason({ ...base, gate: OTHER })).toBe("not ready");
		expect(releaseRefusalReason({ ...base, unread: 0, gate: { ok: true } })).toMatch(
			/no undelivered/,
		);
		expect(releaseRefusalReason({ ...base, atStoppingPoint: false, gate: { ok: true } })).toMatch(
			/stopping point/,
		);
	});
});

describe("mailHold store", () => {
	it("sets, clears and notifies once per change", () => {
		const cb = vi.fn();
		const un = mailHold.subscribe("h1", cb);
		mailHold.set("h1", true);
		mailHold.set("h1", true);
		expect(mailHold.get("h1").held).toBe(true);
		mailHold.setReleaseRefusal("h1", "nope");
		expect(mailHold.get("h1")).toEqual({ held: true, releaseRefusal: "nope" });
		mailHold.set("h1", false);
		expect(mailHold.get("h1")).toEqual({ held: false });
		expect(cb).toHaveBeenCalledTimes(3);
		un();
	});

	it("keeps a held entry when a deliver-now refusal is recorded (no set(false) on that path)", () => {
		mailHold.set("h3", true);
		mailHold.setReleaseRefusal("h3", "not ready");
		expect(mailHold.get("h3")).toEqual({ held: true, releaseRefusal: "not ready" });
	});

	it("forget drops the entry and release reaches listeners", () => {
		mailHold.set("h2", true);
		mailHold.forget("h2");
		expect(mailHold.get("h2").held).toBe(false);
		const cb = vi.fn();
		const off = mailHold.onRelease(cb);
		mailHold.release("h2");
		off();
		mailHold.release("h2");
		expect(cb).toHaveBeenCalledTimes(1);
		expect(cb).toHaveBeenCalledWith("h2");
	});
});
