// Control Center host state (#492): one open flag toggled by the strip
// button, Mod+0, and the palette. The workspace stays mounted
// underneath (PTYs live there); App only hides it.

import {
	type Dispatch,
	type SetStateAction,
	useCallback,
	useEffect,
	useMemo,
	useRef,
	useState,
} from "react";
import type { StripSegment } from "../roomGroups.ts";
import type { TodosApi } from "../todos/useTodos.ts";
import type { Room } from "../types.ts";
import type { ControlCenterProps } from "./ControlCenter.tsx";
import { toggleAction } from "./popoutSync.ts";
import { useControlCenterPopout } from "./useControlCenterPopout.ts";

export function useControlCenter(
	rooms: Room[],
	segments: StripSegment[],
	activeRoomId: string,
	setActiveRoomId: Dispatch<SetStateAction<string>>,
	switchHarnessInRoom: (roomId: string, harnessId: string) => void,
	todos: TodosApi,
) {
	const [open, setOpen] = useState(false);
	const onFocus = useCallback(
		(roomId: string, harnessId?: string) => {
			setActiveRoomId(roomId);
			if (harnessId) switchHarnessInRoom(roomId, harnessId);
			setOpen(false);
		},
		[setActiveRoomId, switchHarnessInRoom],
	);
	// #493: the pop-out window, when there is one, is what every entry point
	// (strip button, Mod+0, palette) brings forward instead of the in-app view.
	// "Dock back" opens the in-app view; the pop-out closes itself.
	const onDock = useCallback(() => setOpen(true), []);
	const { visible: visibleTodos, setDone, setNote, remove } = todos;
	const todoActions = useMemo(() => ({ setDone, setNote, remove }), [setDone, setNote, remove]);
	const popout = useControlCenterPopout(
		rooms,
		segments,
		activeRoomId,
		onFocus,
		onDock,
		visibleTodos,
		todoActions,
	);
	const { poppedOut, poppedOutRef, raise, open: openPopout } = popout;
	// Never run the in-app view under a live pop-out (both run the data hook).
	useEffect(() => {
		if (poppedOut) setOpen(false);
	}, [poppedOut]);
	const toggle = useCallback(() => {
		if (toggleAction(poppedOutRef.current) === "raise") void raise();
		else setOpen((o) => !o);
	}, [poppedOutRef, raise]);
	const onPopOut = useCallback(() => {
		void openPopout();
		setOpen(false);
	}, [openPopout]);
	// Mirrors for the shortcut listener, which stays bound across renders.
	const toggleRef = useRef(toggle);
	toggleRef.current = toggle;

	// Any room change (tab click, Mod+1..9, nav, palette) leaves the
	// overview; so the room active at open time is still active on a
	// plain toggle close, and a closed-meanwhile room just keeps
	// whatever is active now.
	// biome-ignore lint/correctness/useExhaustiveDependencies: only a room change should close
	useEffect(() => setOpen(false), [activeRoomId]);

	const props: ControlCenterProps = {
		rooms,
		segments,
		activeRoomId,
		visible: open,
		onFocus,
		todos: visibleTodos,
		todoActions,
		onPopOut,
	};
	return { open, toggle, toggleRef, props };
}
