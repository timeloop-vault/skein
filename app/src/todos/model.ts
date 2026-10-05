// The manual todo list (#335): pure model, no React, no Tauri.
//
// A todo is "come back to this" on a room or one of its harnesses. Room
// todos live on `Room.todos`; global ones in their own store. Both use
// this shape, so everything here takes plain lists.

import type { Harness, Room } from "../types.ts";

export interface Todo {
	id: string;
	createdMs: number;
	title: string;
	note?: string;
	roomId?: string;
	harnessId?: string;
	doneMs?: number;
}

export type TodoScope = "room" | "global";

/** A done todo is dropped this long after it was ticked off. */
export const DONE_RETENTION_MS = 24 * 60 * 60 * 1000;

/** One row of the Control Center list. Plain JSON: it crosses a Tauri event. */
export interface VisibleTodo {
	scope: TodoScope;
	todo: Todo;
	/** Name of the linked room; null when unlinked, missing or archived. */
	roomName: string | null;
	/** True when `harnessId` no longer exists in the room: a click falls back to the room. */
	harnessGone: boolean;
}

export function makeTodo(args: {
	room: Pick<Room, "id" | "name">;
	harness?: Pick<Harness, "id" | "name"> | undefined;
	now: number;
	id: string;
}): Todo {
	const { room, harness, now, id } = args;
	const todo: Todo = {
		id,
		createdMs: now,
		title: harness ? `${room.name} · ${harness.name}` : room.name,
		roomId: room.id,
	};
	if (harness) todo.harnessId = harness.id;
	return todo;
}

/** Adds `todo` unless an open todo for the same room + harness exists. */
export function addTodo(list: Todo[], todo: Todo): { list: Todo[]; added: boolean } {
	const dup = list.some(
		(t) => t.doneMs === undefined && t.roomId === todo.roomId && t.harnessId === todo.harnessId,
	);
	return dup ? { list, added: false } : { list: [...list, todo], added: true };
}

export function setDone(list: Todo[], id: string, done: boolean, now: number): Todo[] {
	return list.map((t) => {
		if (t.id !== id) return t;
		const { doneMs: _drop, ...rest } = t;
		return done ? { ...rest, doneMs: now } : rest;
	});
}

/** An empty note clears it. */
export function setNote(list: Todo[], id: string, note: string): Todo[] {
	return list.map((t) => {
		if (t.id !== id) return t;
		const { note: _drop, ...rest } = t;
		return note === "" ? rest : { ...rest, note };
	});
}

export function removeTodo(list: Todo[], id: string): Todo[] {
	return list.filter((t) => t.id !== id);
}

/** Drops todos done more than `DONE_RETENTION_MS` ago (exactly 24h is kept). */
export function pruneDone(list: Todo[], now: number): Todo[] {
	return list.filter((t) => t.doneMs === undefined || now - t.doneMs <= DONE_RETENTION_MS);
}

/** Stored rows first, then anything added locally before the load resolved. */
export function mergeLoaded(rows: Todo[], current: Todo[]): Todo[] {
	return [...rows, ...current.filter((c) => !rows.some((r) => r.id === c.id))];
}

function resolve(scope: TodoScope, todo: Todo, roomsById: Map<string, Room>): VisibleTodo {
	const room = todo.roomId === undefined ? undefined : roomsById.get(todo.roomId);
	const live = room && room.archived === undefined ? room : undefined;
	const harnessGone =
		todo.harnessId !== undefined &&
		live !== undefined &&
		!live.harnesses.some((h) => h.id === todo.harnessId);
	return { scope, todo, roomName: live ? live.name : null, harnessGone };
}

/** Open first (newest first), then done (most recently done first). */
export function visibleTodos(rooms: Room[], globalTodos: Todo[], now: number): VisibleTodo[] {
	const roomsById = new Map(rooms.map((r) => [r.id, r]));
	const out: VisibleTodo[] = [];
	for (const room of rooms) {
		if (room.archived !== undefined) continue;
		for (const t of pruneDone(room.todos ?? [], now)) out.push(resolve("room", t, roomsById));
	}
	for (const t of pruneDone(globalTodos, now)) out.push(resolve("global", t, roomsById));
	const open = out.filter((v) => v.todo.doneMs === undefined);
	const done = out.filter((v) => v.todo.doneMs !== undefined);
	open.sort((a, b) => b.todo.createdMs - a.todo.createdMs);
	done.sort((a, b) => (b.todo.doneMs ?? 0) - (a.todo.doneMs ?? 0));
	return [...open, ...done];
}
