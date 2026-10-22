import { describe, expect, it } from "vitest";
import {
	consentNeeded,
	entryOf,
	fallbackListing,
	type HermesProfileInfo,
	orderProfiles,
	profileLabel,
	profileTag,
	type RootNote,
	rootNoteText,
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
	it("asks again for a present entry whose hooks are not approved", () => {
		expect(consentNeeded("work", "present", true)).toBe("ask");
		expect(consentNeeded("skein", "present", true)).toBe("none");
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

describe("rootNoteText", () => {
	const l = (rootNote?: RootNote | null) => ({
		root: "/h",
		profiles: [],
		error: null,
		...(rootNote === undefined ? {} : { rootNote }),
	});
	it("is null with no note", () => {
		expect(rootNoteText(l())).toBeNull();
		expect(rootNoteText(l(null))).toBeNull();
	});
	it("suggests the root for a profiles folder", () => {
		expect(rootNoteText(l({ kind: "looksLikeProfilesDir", suggestedRoot: "/h/up" }))).toBe(
			"HERMES_HOME points at a profiles folder. Skein adds profiles/ itself — set HERMES_HOME to /h/up in Settings → Shell & environment.",
		);
		expect(rootNoteText(l({ kind: "looksLikeProfilesDir", suggestedRoot: null }))).toContain(
			"the folder above it",
		);
	});
	it("words missing and noProfilesDir", () => {
		expect(rootNoteText(l({ kind: "missing", suggestedRoot: null }))).toBe(
			"No Hermes folder at this path. If Hermes lives elsewhere, set HERMES_HOME in Settings → Shell & environment.",
		);
		expect(rootNoteText(l({ kind: "noProfilesDir", suggestedRoot: null }))).toBe(
			"No profiles/ folder here, so only Skein's profile and the default are listed.",
		);
	});
});
