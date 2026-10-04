// Control Center host state (#492): one open flag toggled by the strip
// button, Mod+0, and the palette. The workspace stays mounted
// underneath (PTYs live there); App only hides it.

import {
	type Dispatch,
	type SetStateAction,
	useCallback,
	useEffect,
	useRef,
	useState,
} from "react";
import type { StripSegment } from "../roomGroups.ts";
import type { Room } from "../types.ts";
import type { ControlCenterProps } from "./ControlCenter.tsx";

export function useControlCenter(
	rooms: Room[],
	segments: StripSegment[],
	activeRoomId: string,
	setActiveRoomId: Dispatch<SetStateAction<string>>,
	switchHarnessInRoom: (roomId: string, harnessId: string) => void,
) {
	const [open, setOpen] = useState(false);
	const toggle = useCallback(() => setOpen((o) => !o), []);
	// Mirrors for the shortcut listener, which stays bound across renders.
	const toggleRef = useRef(toggle);
	toggleRef.current = toggle;

	// Any room change (tab click, Mod+1..9, nav, palette) leaves the
	// overview; so the room active at open time is still active on a
	// plain toggle close, and a closed-meanwhile room just keeps
	// whatever is active now.
	// biome-ignore lint/correctness/useExhaustiveDependencies: only a room change should close
	useEffect(() => setOpen(false), [activeRoomId]);

	const onFocus = useCallback(
		(roomId: string, harnessId?: string) => {
			setActiveRoomId(roomId);
			if (harnessId) switchHarnessInRoom(roomId, harnessId);
			setOpen(false);
		},
		[setActiveRoomId, switchHarnessInRoom],
	);

	const props: ControlCenterProps = {
		rooms,
		segments,
		activeRoomId,
		visible: open,
		onFocus,
	};
	return { open, toggle, toggleRef, props };
}
