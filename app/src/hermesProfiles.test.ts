import { describe, expect, it } from "vitest";
import {
	consentNeeded,
	entryOf,
	fallbackListing,
	type HermesProfileInfo,
	orderProfiles,
	profileLabel,
	profileTag,
} from "./hermesProfiles.ts";

const p = (
	name: string,
	skeinEntry: HermesProfileInfo["skeinEntry"] = "absent",
): HermesProfileInfo => ({
	name,
	exists: true,
	skeinEntry,
	reason: null,
});

describe("orderProfiles", () => {
	it("puts skein, then default, then the rest alphabetically", () => {
		const out = orderProfiles([p("zed"), p("default"), p("alpha"), p("skein", "present")]);
		expect(out.map((x) => x.name)).toEqual(["skein", "default", "alpha", "zed"]);
	});
	it("always offers skein and default, even from nothing", () => {
		expect(orderProfiles([]).map((x) => x.name)).toEqual(["skein", "default"]);
		expect(fallbackListing("boom").profiles.map((x) => x.name)).toEqual(["skein", "default"]);
		expect(fallbackListing("boom").error).toBe("boom");
	});
	it("keeps the listing's own row when present", () => {
		expect(orderProfiles([p("default", "refused")])[1]?.skeinEntry).toBe("refused");
	});
});

describe("consentNeeded", () => {
	it("never asks for skein's own profile", () => {
		expect(consentNeeded("skein", "absent")).toBe("none");
	});
	it("asks for an absent entry, goes manual for a refused one", () => {
		expect(consentNeeded("work", "absent")).toBe("ask");
		expect(consentNeeded("work", "refused")).toBe("manual");
	});
	it("runs directly when present, no config, or unknown", () => {
		expect(consentNeeded("work", "present")).toBe("none");
		expect(consentNeeded("work", "noConfig")).toBe("none");
		expect(consentNeeded("work", undefined)).toBe("none");
	});
});

describe("labels", () => {
	it("tags rows by entry state", () => {
		expect(profileTag(p("skein"))).toBeNull();
		expect(profileTag(p("a", "present"))).toBe("review tools on");
		expect(profileTag(p("a", "absent"))).toBe("no review tools yet");
		expect(profileTag(p("a", "refused"))).toBe("review tools: manual");
		expect(profileTag(p("a", "noConfig"))).toBeNull();
		expect(profileLabel("x")).toBe("x");
	});
	it("finds an entry", () => {
		const l = { root: null, profiles: [p("a")], error: null };
		expect(entryOf(l, "a")?.name).toBe("a");
		expect(entryOf(null, "a")).toBeUndefined();
	});
});
