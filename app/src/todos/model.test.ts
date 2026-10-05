import { describe, expect, it } from "vitest";
import type { Harness, Room } from "../types.ts";
import {
	addTodo,
	DONE_RETENTION_MS,
	makeTodo,
	mergeLoaded,
	pruneDone,
	removeTodo,
	setDone,
	setNote,
	type Todo,
	visibleTodos,
} from "./model.ts";

const harness = (id: string, name = id): Harness => ({
	id,
	kind: "claude",
	name,
	status: "idle",
	model: "",
	tokens: "0",
});
const room = (id: string, extra: Partial<Room> = {}): Room => ({
	id,
	name: `room-${id}`,
	task: "",
	status: "idle",
	badge: 0,
	harnesses: [harness("h1", "claude 1")],
	activeHarnessId: "h1",
	...extra,
});
const todo = (id: string, extra: Partial<Todo> = {}): Todo => ({
	id,
	createdMs: 1000,
	title: id,
	...extra,
});

describe("makeTodo", () => {
	it("titles with room and harness label", () => {
		const t = makeTodo({ room: room("r1"), harness: harness("h1", "claude 1"), now: 5, id: "t" });
		expect(t).toEqual({
			id: "t",
			createdMs: 5,
			title: "room-r1 · claude 1",
			roomId: "r1",
			harnessId: "h1",
		});
	});
	it("uses just the room name without a harness", () => {
		const t = makeTodo({ room: room("r1"), now: 5, id: "t" });
		expect(t.title).toBe("room-r1");
		expect("harnessId" in t).toBe(false);
	});
});

describe("addTodo", () => {
	it("refuses a duplicate open todo for the same room + harness", () => {
		const a = todo("a", { roomId: "r1", harnessId: "h1" });
		const res = addTodo([a], todo("b", { roomId: "r1", harnessId: "h1" }));
		expect(res.added).toBe(false);
		expect(res.list).toEqual([a]);
	});
	it("allows it when the existing one is done, or the target differs", () => {
		const done = todo("a", { roomId: "r1", harnessId: "h1", doneMs: 9 });
		expect(addTodo([done], todo("b", { roomId: "r1", harnessId: "h1" })).added).toBe(true);
		const open = todo("a", { roomId: "r1", harnessId: "h1" });
		expect(addTodo([open], todo("b", { roomId: "r1" })).added).toBe(true);
	});
});

describe("setDone / setNote / removeTodo", () => {
	it("marks done and undone", () => {
		const done = setDone([todo("a")], "a", true, 50);
		expect(done[0]?.doneMs).toBe(50);
		const undone = setDone(done, "a", false, 60);
		expect("doneMs" in (undone[0] as Todo)).toBe(false);
	});
	it("sets and clears a note", () => {
		const withNote = setNote([todo("a")], "a", "hi");
		expect(withNote[0]?.note).toBe("hi");
		const cleared = setNote(withNote, "a", "");
		expect("note" in (cleared[0] as Todo)).toBe(false);
	});
	it("removes by id", () => {
		expect(removeTodo([todo("a"), todo("b")], "a").map((t) => t.id)).toEqual(["b"]);
	});
});

describe("pruneDone", () => {
	it("keeps exactly 24h, drops older, keeps open", () => {
		const now = 10 * DONE_RETENTION_MS;
		const list = [
			todo("edge", { doneMs: now - DONE_RETENTION_MS }),
			todo("old", { doneMs: now - DONE_RETENTION_MS - 1 }),
			todo("open"),
		];
		expect(pruneDone(list, now).map((t) => t.id)).toEqual(["edge", "open"]);
	});
});

describe("visibleTodos", () => {
	const now = 1_000_000;
	it("hides todos of archived rooms", () => {
		const rooms = [
			room("r1", { todos: [todo("a", { roomId: "r1" })] }),
			room("r2", { archived: 5, todos: [todo("b", { roomId: "r2" })] }),
		];
		expect(visibleTodos(rooms, [], now).map((v) => v.todo.id)).toEqual(["a"]);
	});
	it("flags harnessGone but keeps the room", () => {
		const rooms = [room("r1", { todos: [todo("a", { roomId: "r1", harnessId: "zz" })] })];
		const [v] = visibleTodos(rooms, [], now);
		expect(v).toMatchObject({ scope: "room", roomName: "room-r1", harnessGone: true });
	});
	it("shows a global todo with a missing or archived room, flagged", () => {
		const rooms = [room("r2", { archived: 5 })];
		const g = [
			todo("m", { roomId: "nope", harnessId: "h1" }),
			todo("n", { roomId: "r2", harnessId: "h1" }),
			todo("free"),
		];
		const vs = visibleTodos(rooms, g, now);
		expect(vs).toHaveLength(3);
		for (const v of vs) {
			expect(v.scope).toBe("global");
			expect(v.roomName).toBeNull();
			expect(v.harnessGone).toBe(false);
		}
	});
	it("orders open newest first, then done most recently done first", () => {
		const rooms = [
			room("r1", {
				todos: [todo("old", { createdMs: 1 }), todo("d1", { createdMs: 9, doneMs: now - 100 })],
			}),
		];
		const g = [todo("new", { createdMs: 5 }), todo("d2", { createdMs: 2, doneMs: now - 10 })];
		expect(visibleTodos(rooms, g, now).map((v) => v.todo.id)).toEqual(["new", "old", "d2", "d1"]);
	});
	it("drops long-done todos", () => {
		const g = [todo("x", { doneMs: now - DONE_RETENTION_MS - 1 })];
		expect(visibleTodos([], g, now)).toEqual([]);
	});
});

describe("mergeLoaded", () => {
	const t = (id: string): Todo => ({ id, createdMs: 1, title: id });
	it("keeps rows first, then locally added todos not in rows", () => {
		expect(mergeLoaded([t("a")], [t("b")]).map((x) => x.id)).toEqual(["a", "b"]);
	});
	it("does not duplicate a todo present in both", () => {
		expect(mergeLoaded([t("a")], [t("a"), t("b")]).map((x) => x.id)).toEqual(["a", "b"]);
	});
});
