import { describe, expect, it } from "vitest";
import { createHeldMirror } from "./mailHeldMirror.ts";

const setup = () => {
	const calls: Array<[string, boolean]> = [];
	const mirror = createHeldMirror((id, held) => calls.push([id, held]));
	return { calls, mirror };
};

describe("createHeldMirror", () => {
	it("seeds from the snapshot", () => {
		const { calls, mirror } = setup();
		mirror.snapshot(["a", "b"]);
		expect(calls).toEqual([
			["a", true],
			["b", true],
		]);
	});

	it("applies events after the snapshot and skips no-ops", () => {
		const { calls, mirror } = setup();
		mirror.snapshot(["a"]);
		calls.length = 0;
		mirror.event({ harnessId: "a", held: true });
		mirror.event({ harnessId: "b", held: true });
		mirror.event({ harnessId: "a", held: false });
		expect(calls).toEqual([
			["b", true],
			["a", false],
		]);
	});

	it("buffers events that arrive before the snapshot and replays them on top of it", () => {
		const { calls, mirror } = setup();
		mirror.event({ harnessId: "a", held: false });
		mirror.event({ harnessId: "c", held: true });
		expect(calls).toEqual([]);
		mirror.snapshot(["a", "b"]);
		expect(mirror.heldIds().sort()).toEqual(["b", "c"]);
	});

	it("a later snapshot releases what it no longer lists", () => {
		const { calls, mirror } = setup();
		mirror.snapshot(["a", "b"]);
		calls.length = 0;
		mirror.snapshot(["b"]);
		expect(calls).toEqual([["a", false]]);
	});

	it("forget drops a harness without calling apply", () => {
		const { calls, mirror } = setup();
		mirror.snapshot(["a"]);
		calls.length = 0;
		mirror.forget("a");
		expect(calls).toEqual([]);
		expect(mirror.heldIds()).toEqual([]);
	});
});
