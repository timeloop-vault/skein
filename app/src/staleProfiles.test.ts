import { describe, expect, it } from "vitest";
import { stateFromSettings } from "./envProfileStore.ts";
import { countStaleHarnesses, staleTitle, switchToastMessage } from "./staleProfiles.ts";
import type { Room } from "./types.ts";

const room = (hs: { id: string; envProfile?: string }[]): Room =>
	({ id: "r", harnesses: hs }) as unknown as Room;

describe("countStaleHarnesses", () => {
	const spawned = new Map<string, string | null>([
		["a", null],
		["b", "proxy"],
		["c", null],
	]);
	const rooms = [room([{ id: "a" }, { id: "b" }, { id: "c", envProfile: "proxy" }, { id: "d" }])];
	it("counts running harnesses that would spawn differently", () => {
		// active=proxy: a (null->proxy) stale, b ok, c override proxy stale, d never spawned
		expect(countStaleHarnesses(rooms, "proxy", ["proxy"], (id) => spawned.get(id))).toBe(2);
	});
	it("follows the new selection", () => {
		expect(countStaleHarnesses(rooms, null, ["proxy"], (id) => spawned.get(id))).toBe(2);
	});
	it("is zero when nothing differs", () => {
		expect(countStaleHarnesses([room([{ id: "a" }])], null, [], (id) => spawned.get(id))).toBe(0);
	});
});

describe("switchToastMessage", () => {
	it("omits the second clause at zero", () => {
		expect(switchToastMessage("proxy", 0)).toBe(
			'Env profile "proxy" applies to harnesses started from now on.',
		);
	});
	it("names the stale count", () => {
		expect(switchToastMessage("proxy", 3)).toContain("3 running harnesses keep their current env");
		expect(switchToastMessage(null, 1)).toContain("1 running harness keeps");
	});
});

describe("staleTitle", () => {
	it("is null when not stale", () => {
		expect(staleTitle("x", "x")).toBeNull();
		expect(staleTitle(undefined, "x")).toBeNull();
	});
	it("explains the difference", () => {
		expect(staleTitle(null, "proxy")).toBe(
			'This harness started with no profile — restart to apply "proxy"',
		);
	});
});

describe("stateFromSettings", () => {
	it("is loaded once settings were published, even with no profiles", () => {
		expect(stateFromSettings({ envProfiles: [] }).loaded).toBe(true);
	});
	it("drops an active name that does not exist", () => {
		expect(
			stateFromSettings({ envProfiles: [{ name: "a", vars: [] }], activeEnvProfile: "zz" }),
		).toEqual({ names: ["a"], active: null, loaded: true });
		expect(stateFromSettings(null)).toEqual({ names: [], active: null, loaded: false });
	});
});
