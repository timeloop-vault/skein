// App.tsx wiring for the manual todo list (#335): the todo store, the
// add-with-toast handler the shortcuts and tab menus share, and the Control
// Center overlay that renders the list (#492).

import { useControlCenter } from "../controlCenter/useControlCenter.ts";
import type { StripSegment } from "../roomGroups.ts";
import type { NewToast } from "../toastStack.ts";
import type { useRoomsStore } from "../useRoomsStore.ts";
import { makeAddTodoWithFeedback } from "./addFeedback.ts";
import { useTodos } from "./useTodos.ts";

export function useTodoWiring(
	store: Pick<
		ReturnType<typeof useRoomsStore>,
		| "rooms"
		| "setRooms"
		| "roomsRef"
		| "activeRooms"
		| "activeRoomId"
		| "setActiveRoomId"
		| "roomSelectSeq"
	>,
	pushToast: (entry: NewToast) => void,
	segments: StripSegment[],
	selectHarness: (roomId: string, harnessId: string) => void,
) {
	const todos = useTodos(store.rooms, store.setRooms);
	const addTodo = makeAddTodoWithFeedback(store.roomsRef, todos.add, pushToast);
	const cc = useControlCenter(
		store.activeRooms,
		segments,
		store.activeRoomId,
		store.setActiveRoomId,
		store.roomSelectSeq,
		selectHarness,
		todos,
	);
	return { addTodo, cc };
}
