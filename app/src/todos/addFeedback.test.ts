import { describe, expect, it } from "vitest";
import type { Room } from "../types.ts";
import { activeTodoTarget, todoAddMessage } from "./addFeedback.ts";

describe("todoAddMessage", () => {
	it("words added vs duplicate per scope", () => {
		expect(todoAddMessage("room", true)).toBe("Added to room todos");
		expect(todoAddMessage("global", true)).toBe("Added to global todos");
		expect(todoAddMessage("room", false)).toBe("Already in room todos");
	});
});

describe("activeTodoTarget", () => {
	const room = (harnesses: { id: string }[], activeHarnessId: string) =>
		({ id: "r1", harnesses, activeHarnessId }) as unknown as Room;
	it("targets the active harness, or the room alone without one", () => {
		expect(activeTodoTarget([room([{ id: "h1" }], "h1")], "r1")).toEqual({
			roomId: "r1",
			harnessId: "h1",
		});
		expect(activeTodoTarget([room([], "")], "r1")).toEqual({ roomId: "r1" });
		expect(activeTodoTarget([], "r1")).toBeNull();
	});
});
