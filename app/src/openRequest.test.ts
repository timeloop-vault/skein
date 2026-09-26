import { describe, expect, it } from "vitest";
import { type OpenTarget, decideOpen } from "./openRequest.ts";
import type { Room } from "./types.ts";

const room = (id: string): Room => ({
	id,
	name: id,
	task: "",
	status: "idle",
	badge: 0,
	harnesses: [],
	activeHarnessId: "",
});

const at = (roomIds: string[], archived = false): OpenTarget => ({
	kind: "room",
	roomIds,
	archived,
	folder: "/code/repo",
});

describe("decideOpen", () => {
	it("focuses the best-ranked room", () => {
		const rooms = [room("a"), room("b")];
		expect(decideOpen(at(["b", "a"]), rooms, null)).toEqual({ kind: "focus", roomId: "b" });
	});

	it("keeps you in the room you are in when it shares the folder", () => {
		const rooms = [room("a"), room("b")];
		expect(decideOpen(at(["a", "b"]), rooms, "b")).toEqual({ kind: "focus", roomId: "b" });
	});

	it("does not prefer the current room when it is not one of the owners", () => {
		const rooms = [room("a"), room("elsewhere")];
		expect(decideOpen(at(["a"]), rooms, "elsewhere")).toEqual({ kind: "focus", roomId: "a" });
	});

	it("skips owners deleted since Rust resolved, then falls back to New room", () => {
		const rooms = [room("b")];
		expect(decideOpen(at(["gone", "b"]), rooms, null)).toEqual({ kind: "focus", roomId: "b" });
		expect(decideOpen(at(["gone"]), rooms, null)).toEqual({
			kind: "newRoom",
			folder: "/code/repo",
		});
	});

	it("passes New room and missing targets straight through", () => {
		expect(decideOpen({ kind: "newRoom", folder: "/x" }, [], null)).toEqual({
			kind: "newRoom",
			folder: "/x",
		});
		expect(decideOpen({ kind: "missing", path: "/nope" }, [], null)).toEqual({
			kind: "missing",
			path: "/nope",
		});
	});
});
