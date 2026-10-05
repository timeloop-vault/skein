// The manual todo list (#335), one hook mounted once in the main window.
// Room todos live on `Room.todos` and persist through the rooms autosave;
// global ones through useGlobalTodos. The visible list is recomputed on
// every change (no timer), which is also when done-retention pruning runs.

import { type Dispatch, type SetStateAction, useCallback, useMemo, useRef } from "react";
import type { Room } from "../types.ts";
import {
	addTodo,
	makeTodo,
	pruneDone,
	removeTodo,
	setDone as setDoneIn,
	setNote as setNoteIn,
	type Todo,
	type TodoScope,
	type VisibleTodo,
	visibleTodos,
} from "./model.ts";
import { useGlobalTodos } from "./useGlobalTodos.ts";

/** Mutations a Control Center view can ask for. `roomId` locates a room-scope todo. */
export interface TodoActions {
	setDone: (scope: TodoScope, id: string, done: boolean, roomId?: string) => void;
	setNote: (scope: TodoScope, id: string, note: string, roomId?: string) => void;
	remove: (scope: TodoScope, id: string, roomId?: string) => void;
}

export interface TodosApi extends TodoActions {
	visible: VisibleTodo[];
	/** Adds a todo for the room (+ harness). False when an open one already exists. */
	add: (scope: TodoScope, roomId: string, harnessId?: string) => boolean;
}

export function useTodos(rooms: Room[], setRooms: Dispatch<SetStateAction<Room[]>>): TodosApi {
	const { todos: globalTodos, setTodos: setGlobal } = useGlobalTodos();
	const roomsRef = useRef(rooms);
	roomsRef.current = rooms;
	const globalRef = useRef(globalTodos);
	globalRef.current = globalTodos;

	const visible = useMemo(() => visibleTodos(rooms, globalTodos, Date.now()), [rooms, globalTodos]);

	const mutate = useCallback(
		(scope: TodoScope, roomId: string | undefined, fn: (list: Todo[]) => Todo[]) => {
			// Every write prunes done todos past their retention.
			const apply = (l: Todo[]) => pruneDone(fn(l), Date.now());
			if (scope === "global") {
				setGlobal(apply);
				return;
			}
			setRooms((rs) =>
				rs.map((r) =>
					roomId !== undefined && r.id !== roomId ? r : { ...r, todos: apply(r.todos ?? []) },
				),
			);
		},
		[setGlobal, setRooms],
	);

	const add = useCallback(
		(scope: TodoScope, roomId: string, harnessId?: string): boolean => {
			const room = roomsRef.current.find((r) => r.id === roomId);
			if (!room) return false;
			const harness =
				harnessId === undefined ? undefined : room.harnesses.find((h) => h.id === harnessId);
			const todo = makeTodo({ room, harness, now: Date.now(), id: crypto.randomUUID() });
			const current = scope === "global" ? globalRef.current : (room.todos ?? []);
			const { list, added } = addTodo(current, todo);
			if (!added) return false;
			// Keep the refs current so a second add in the same tick sees this one.
			if (scope === "global") globalRef.current = list;
			else
				roomsRef.current = roomsRef.current.map((r) =>
					r.id === roomId ? { ...r, todos: list } : r,
				);
			// Apply against the latest state (addTodo dedupes again) so same-tick
			// setDone/setNote/remove are not clobbered.
			mutate(scope, roomId, (l) => addTodo(l, todo).list);
			return true;
		},
		[mutate],
	);

	const setDone = useCallback(
		(scope: TodoScope, id: string, done: boolean, roomId?: string) =>
			mutate(scope, roomId, (l) => setDoneIn(l, id, done, Date.now())),
		[mutate],
	);
	const setNote = useCallback(
		(scope: TodoScope, id: string, note: string, roomId?: string) =>
			mutate(scope, roomId, (l) => setNoteIn(l, id, note)),
		[mutate],
	);
	const remove = useCallback(
		(scope: TodoScope, id: string, roomId?: string) =>
			mutate(scope, roomId, (l) => removeTodo(l, id)),
		[mutate],
	);

	return { visible, add, setDone, setNote, remove };
}
