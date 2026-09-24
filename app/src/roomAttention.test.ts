import { describe, expect, it } from "vitest";
import { clearAttention } from "./roomAttention.ts";
import type { Room } from "./types.ts";

function room(id: string, opts: Partial<Room> = {}): Room {
	return {
		id,
		name: id,
		task: "",
		status: "idle",
		badge: 0,
		harnesses: [],
		activeHarnessId: "",
		...opts,
	};
}

describe("clearAttention", () => {
	it("drops the attention mark on the active room", () => {
		const rooms = [room("a", { attention: true }), room("b")];
		const next = clearAttention(rooms, "a");
		expect(next[0]?.attention).toBeUndefined();
		expect("attention" in (next[0] ?? {})).toBe(false);
		expect(next[1]).toBe(rooms[1]);
	});

	it("leaves other rooms' attention marks untouched", () => {
		const rooms = [room("a", { attention: true }), room("b", { attention: true })];
		const next = clearAttention(rooms, "a");
		expect(next[0]?.attention).toBeUndefined();
		expect(next[1]?.attention).toBe(true);
	});

	it("returns the same array reference when activeId is null", () => {
		const rooms = [room("a", { attention: true })];
		expect(clearAttention(rooms, null)).toBe(rooms);
	});

	it("returns the same array reference when the active room is not found", () => {
		const rooms = [room("a", { attention: true })];
		expect(clearAttention(rooms, "missing")).toBe(rooms);
	});

	it("returns the same array reference when the active room has no mark", () => {
		const rooms = [room("a"), room("b", { attention: true })];
		expect(clearAttention(rooms, "a")).toBe(rooms);
	});
});
