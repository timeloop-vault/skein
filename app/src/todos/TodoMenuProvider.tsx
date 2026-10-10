// Hosts the ONE context menu the tabs share (#335). A tab calls
// `useTodoMenu()(event, roomId, harnessId?)` from its onContextMenu; the
// provider owns the menu state, so nothing is drilled through the strip or
// the harness column.

import {
	createContext,
	type MouseEvent as ReactMouseEvent,
	type ReactNode,
	useCallback,
	useContext,
	useLayoutEffect,
	useRef,
} from "react";
import { ContextMenu, useContextMenu } from "../ContextMenu.tsx";
import type { AddTodoWithFeedback } from "./addFeedback.ts";

type OpenTodoMenu = (e: ReactMouseEvent, roomId: string, harnessId?: string) => void;

const TodoMenuContext = createContext<OpenTodoMenu>((e) => e.preventDefault());

export const useTodoMenu = (): OpenTodoMenu => useContext(TodoMenuContext);

export const TodoMenuProvider = ({
	addTodo,
	children,
}: {
	addTodo: AddTodoWithFeedback;
	children: ReactNode;
}) => {
	const { menu, open, close } = useContextMenu();
	// #594: the context value must stay referentially stable, or every tab
	// consumer in every room re-renders on each App render. `addTodo` is read
	// through a latest-ref instead of being a dependency.
	const addTodoRef = useRef(addTodo);
	useLayoutEffect(() => {
		addTodoRef.current = addTodo;
	});
	const openTodoMenu = useCallback<OpenTodoMenu>(
		(e, roomId, harnessId) =>
			open(e, [
				{
					id: "room",
					label: "Add to room todos",
					onSelect: () => addTodoRef.current("room", roomId, harnessId),
				},
				{
					id: "global",
					label: "Add to global todos",
					onSelect: () => addTodoRef.current("global", roomId, harnessId),
				},
			]),
		[open],
	);
	return (
		<TodoMenuContext.Provider value={openTodoMenu}>
			{children}
			{menu && <ContextMenu at={menu.at} items={menu.items} onClose={close} />}
		</TodoMenuContext.Provider>
	);
};
