import { describe, expect, it } from "vitest";
import { decideOpen, linkNoticeToast, type OpenTarget } from "./openRequest.ts";
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

describe("decideOpen jump", () => {
	const withHarness = (id: string, harnessId: string, archived = false): Room => ({
		...room(id),
		...(archived ? { archived: Date.now() } : {}),
		harnesses: [{ id: harnessId } as Room["harnesses"][number]],
	});
	const jump = (roomId: string, harnessId: string | null): OpenTarget => ({
		kind: "jump",
		roomId,
		harnessId,
	});

	it("jumps to a known room and harness with no notice", () => {
		expect(decideOpen(jump("r", "h"), [withHarness("r", "h")], null)).toEqual({
			kind: "jump",
			roomId: "r",
			harnessId: "h",
			notice: null,
		});
	});

	it("jumps to a known room when no harness is named", () => {
		expect(decideOpen(jump("r", null), [withHarness("r", "h")], null)).toMatchObject({
			kind: "jump",
			notice: null,
		});
	});

	it("reports an unknown room", () => {
		expect(decideOpen(jump("gone", "h"), [withHarness("r", "h")], null)).toMatchObject({
			notice: "roomGone",
		});
	});

	it("reports a known room whose harness is gone", () => {
		expect(decideOpen(jump("r", "old"), [withHarness("r", "h")], null)).toMatchObject({
			roomId: "r",
			notice: "harnessGone",
		});
	});

	it("jumps into an archived room (the jump reopens it)", () => {
		expect(decideOpen(jump("r", "h"), [withHarness("r", "h", true)], null)).toMatchObject({
			kind: "jump",
			notice: null,
		});
	});

	it("words the toasts", () => {
		expect(linkNoticeToast("roomGone", undefined, 1, 0.5).message).toBe(
			"Skein link: that room no longer exists",
		);
		expect(linkNoticeToast("harnessGone", room("r"), 1, 0.5)).toMatchObject({
			roomId: "r",
			state: "info",
			message: "Skein link: that harness no longer exists",
		});
	});
});
