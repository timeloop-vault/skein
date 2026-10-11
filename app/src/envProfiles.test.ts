import { describe, expect, it } from "vitest";
import {
	activeOnSave,
	addProfile,
	deleteProfile,
	normalizeProfiles,
	profileNames,
	renameProfile,
	validateProfiles,
	withActiveProfile,
} from "./envProfiles.ts";
import type { EnvProfile } from "./types.ts";

const p = (name: string): EnvProfile => ({ name, vars: [] });
const base = { envProfiles: [p("a"), p("b")], activeEnvProfile: "a" as string | null };

describe("envProfiles", () => {
	it("lists names, tolerating absent profiles", () => {
		expect(profileNames(base)).toEqual(["a", "b"]);
		expect(profileNames({})).toEqual([]);
	});

	it("activates only known names", () => {
		expect(withActiveProfile(base, "b").activeEnvProfile).toBe("b");
		expect(withActiveProfile(base, null).activeEnvProfile).toBeNull();
		expect(withActiveProfile(base, "zzz").activeEnvProfile).toBeNull();
	});

	it("renaming the active profile moves the selection", () => {
		const r = renameProfile(base, 0, "x");
		expect(profileNames(r)).toEqual(["x", "b"]);
		expect(r.activeEnvProfile).toBe("x");
	});

	it("renaming another profile keeps the selection", () => {
		expect(renameProfile(base, 1, "x").activeEnvProfile).toBe("a");
	});

	it("deleting the active profile clears it, others keep it", () => {
		expect(deleteProfile(base, 0).activeEnvProfile).toBeNull();
		const d = deleteProfile(base, 1);
		expect(d.activeEnvProfile).toBe("a");
		expect(profileNames(d)).toEqual(["a"]);
	});

	it("adds profiles with unused names", () => {
		const s = addProfile({ envProfiles: [p("Profile 2")], activeEnvProfile: null });
		expect(profileNames(s)).toEqual(["Profile 2", "Profile 3"]);
		expect(profileNames(addProfile({}))).toEqual(["Profile 1"]);
	});

	it("validates names", () => {
		expect(validateProfiles([p("a"), p("b")])).toBeNull();
		expect(validateProfiles([p("  ")])).toMatch(/needs a name/);
		expect(validateProfiles([p("a"), p(" a ")])).toMatch(/"a"/);
	});

	it("normalizes names and drops keyless rows", () => {
		const n = normalizeProfiles({
			envProfiles: [
				{
					name: " a ",
					vars: [
						{ key: " K ", value: "v", unset: false },
						{ key: " ", value: "", unset: false },
					],
				},
			],
			activeEnvProfile: " a ",
		});
		expect(n.envProfiles[0]?.vars).toEqual([{ key: "K", value: "v", unset: false }]);
		expect(n.activeEnvProfile).toBe("a");
		const m = normalizeProfiles({ envProfiles: [p(" a ")], activeEnvProfile: " a " });
		expect(m.activeEnvProfile).toBe("a");
	});
});

describe("activeOnSave", () => {
	it("takes the latest store value when the draft did not change it", () => {
		expect(activeOnSave("A", "A", "B")).toBe("B");
		expect(activeOnSave(undefined, null, "B")).toBe("B");
	});
	it("keeps the draft when the user changed it", () => {
		expect(activeOnSave("C", "A", "B")).toBe("C");
		expect(activeOnSave(null, "A", "B")).toBeNull();
	});
});
