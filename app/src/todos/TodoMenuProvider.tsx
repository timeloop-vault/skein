// Hosts the ONE context menu the tabs share (#335). A tab calls
// `useTodoMenu()(event, roomId, harnessId?)` from its onContextMenu; the
// provider owns the menu state, so nothing is drilled through the strip or
// the harness column.

import {
	createContext,
	type MouseEvent as ReactMouseEvent,
	type ReactNode,
	useContext,
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
	const openTodoMenu: OpenTodoMenu = (e, roomId, harnessId) =>
		open(e, [
			{
				id: "room",
				label: "Add to room todos",
				onSelect: () => addTodo("room", roomId, harnessId),
			},
			{
				id: "global",
				label: "Add to global todos",
				onSelect: () => addTodo("global", roomId, harnessId),
			},
		]);
	return (
		<TodoMenuContext.Provider value={openTodoMenu}>
			{children}
			{menu && <ContextMenu at={menu.at} items={menu.items} onClose={close} />}
		</TodoMenuContext.Provider>
	);
};
