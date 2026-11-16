import { describe, expect, it, vi } from "vitest";
import { mailHold } from "./mailHold.ts";

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

	it("forget drops the entry", () => {
		mailHold.set("h2", true);
		mailHold.forget("h2");
		expect(mailHold.get("h2").held).toBe(false);
	});

	it("subscribeHeld fires only when the held flag turns true or false", () => {
		const seen: string[] = [];
		const un = mailHold.subscribeHeld((id, held) => seen.push(`${id}:${held}`));
		mailHold.set("s1", true);
		mailHold.set("s1", true);
		mailHold.setReleaseRefusal("s1", "no");
		mailHold.set("s1", false);
		mailHold.set("s1", false);
		mailHold.set("s2", true);
		mailHold.forget("s2");
		mailHold.setReleaseRefusal("s3", "no");
		mailHold.forget("s3");
		un();
		mailHold.set("s4", true);
		expect(seen).toEqual(["s1:true", "s1:false", "s2:true", "s2:false"]);
	});
});
