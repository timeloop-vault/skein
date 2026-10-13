import { describe, expect, it } from "vitest";
import {
	activeOnSave,
	addProfile,
	deleteProfile,
	dropRename,
	finalRenames,
	normalizeProfiles,
	profileNames,
	recordRename,
	renameHarnessProfiles,
	renameProfile,
	resolveRenamed,
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

describe("profile renames (#619)", () => {
	it("composes repeated renames and drops a rename back to the origin", () => {
		const ab = recordRename({}, "A", "B");
		expect(recordRename(ab, "B", "C")).toEqual({ A: "C" });
		expect(recordRename(ab, "B", "A")).toEqual({});
		expect(recordRename(ab, "X", "X")).toEqual(ab);
	});

	it("deleting a profile drops its entry", () => {
		expect(dropRename({ A: "C", D: "E" }, "C")).toEqual({ D: "E" });
	});

	it("final renames trim and ignore no-ops and vanished targets", () => {
		expect(finalRenames({ A: " B ", C: "C ", D: "gone" }, ["B", "C"])).toEqual({ A: "B" });
	});

	it("maps an old name to the renamed one, unless it still exists", () => {
		expect(resolveRenamed({ A: "B" }, "A")).toBe("B");
		expect(resolveRenamed({ A: "B" }, "A", ["A", "B"])).toBe("A");
		expect(resolveRenamed({ A: "B" }, "A", ["B"])).toBe("B");
		expect(resolveRenamed({ A: "B" }, null)).toBeNull();
		expect(resolveRenamed({}, "toString")).toBe("toString");
	});

	it("a chip switch to the old name during a rename lands on the new name", () => {
		// Draft renamed A -> A2 (draft active followed it); chip switched to A.
		expect(activeOnSave("A2", "A", "A", { A: "A2" })).toBe("A2");
		// Untouched draft active: the latest switch wins, mapped through renames.
		expect(activeOnSave("A2", "A", "A", { A: "A2" })).toBe("A2");
		// Active A renamed to A2 in the draft, then the chip switched to B:
		// the draft's A2 is the rename, not an edit, so B wins.
		expect(activeOnSave("A2", "A", "B", { A: "A2" })).toBe("B");
	});

	it("updates only harness overrides naming a renamed profile", () => {
		const rooms = [
			{
				id: "r1",
				harnesses: [{ id: "h1", envProfile: "A" }, { id: "h2", envProfile: "B" }, { id: "h3" }],
			},
			{ id: "r2", harnesses: [{ id: "h4" }] },
		];
		const out = renameHarnessProfiles(rooms, { A: "A2" });
		expect(out[0]?.harnesses.map((h) => h.envProfile)).toEqual(["A2", "B", undefined]);
		expect(out[1]).toBe(rooms[1]);
		expect(renameHarnessProfiles(rooms, {})).toBe(rooms);
	});
});
