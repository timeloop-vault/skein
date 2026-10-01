// Pieces shared by the room strip's tabs (#76): the rename/drag types, the
// stable-rooms hook and `LiveRoomTab`, used by `GroupTab`, `GroupRow` and
// `RoomStrip`.

import { type PointerEvent as ReactPointerEvent, useMemo, useRef } from "react";
import { RoomTab } from "./components.tsx";
import { useRoomActivity } from "./harnessActivity.ts";
import type { TabDragInfo } from "./tabDrag.ts";
import type { Room } from "./types.ts";
import type { TabDropTarget } from "./useTabDrag.ts";

/// #241: which room is mid inline-rename, and which tab currently hosts
/// the input. A group's main room can be shown by TWO tabs at once — the
/// top-row `GroupTab` (via `groupDisplayName`) and, when that room is
/// active, its own pinned lead tab in the second row — so `roomId` alone
/// can't say which one should mount the (single) `RoomNameInput`.
/// `"group"` only ever applies to a `GroupTab`; every other room tab
/// (plain top-level, a group's lead, a group member) is `"tab"`.
export interface RenameTarget {
	roomId: string;
	host: "group" | "tab";
}

/// `rooms`, but the SAME array reference across renders as long as its
/// content hasn't changed — membership (room ids, in order) and each
/// member's own `harnesses` array reference. Every call site below
/// builds `rooms` fresh each render, which would otherwise invalidate
/// `useMemo`/`useRoomActivity` downstream on every render regardless of
/// whether anything the tab actually shows moved.

export function useStableRooms(rooms: readonly Room[]): readonly Room[] {
	const ref = useRef(rooms);
	const prev = ref.current;
	const unchanged =
		prev.length === rooms.length &&
		prev.every((r, i) => r.id === rooms[i]?.id && r.harnesses === rooms[i]?.harnesses);
	if (!unchanged) {
		ref.current = rooms;
	}
	return ref.current;
}

/** The slice of `useTabDrag`'s return value every draggable tab in the
 *  strip needs — passed through unchanged to each one. */
export interface RoomDragWiring {
	drag: TabDragInfo | null;
	dropTarget: TabDropTarget;
	startDrag: (e: ReactPointerEvent<HTMLDivElement>, info: TabDragInfo) => void;
	dragHandlers: {
		onPointerMove: (e: ReactPointerEvent<HTMLDivElement>) => void;
		onPointerUp: (e: ReactPointerEvent<HTMLDivElement>) => void;
		onPointerCancel: (e: ReactPointerEvent<HTMLDivElement>) => void;
		onLostPointerCapture: (e: ReactPointerEvent<HTMLDivElement>) => void;
	};
	suppressClick: () => boolean;
}

/// One room tab (plain top-level tab, a group's lead in the second
/// row, or a group member) with its OWN live status/badge — the same
/// per-room `useRoomActivity` fold every room tab has always used.
/// `dragWiring` omitted entirely renders a pinned tab with no drag
/// attributes at all: that's how a group's lead stays undraggable and
/// an invalid drop target (#76 point 6) without `resolveRowDrop` having
/// to refuse it at the DOM layer too.
export const LiveRoomTab = ({
	room,
	active,
	onClick,
	onClose,
	dragWiring,
	dragSegId,
	dragRole,
	renaming,
	onStartRename,
	onRename,
	onRenameEnd,
}: {
	room: Room;
	active: boolean;
	onClick: () => void;
	onClose: () => void;
	dragWiring?: RoomDragWiring;
	dragSegId?: string;
	dragRole?: "segment" | "member";
	/** #241: threaded straight through to `RoomTab` — see its own doc. */
	renaming?: boolean | undefined;
	onStartRename?: (() => void) | undefined;
	onRename?: ((name: string) => void) | undefined;
	onRenameEnd?: (() => void) | undefined;
}) => {
	const stableRooms = useStableRooms([room]);
	const harnessRefs = useMemo(
		() =>
			stableRooms.flatMap((r) =>
				r.harnesses.map((h) => ({ id: h.id, pendingNotifications: h.pendingNotifications })),
			),
		[stableRooms],
	);
	const status = useRoomActivity(harnessRefs);
	const badge = room.harnesses.reduce((acc, h) => acc + (h.pendingNotifications ?? 0), 0);
	const derived: Room = { ...room, status, badge };

	if (!dragWiring) {
		return (
			<RoomTab
				r={derived}
				active={active}
				onClick={onClick}
				onClose={onClose}
				renaming={renaming}
				onStartRename={onStartRename}
				onRename={onRename}
				onRenameEnd={onRenameEnd}
			/>
		);
	}

	const { drag, dropTarget, startDrag, dragHandlers, suppressClick } = dragWiring;
	const isDragged = drag?.kind === "room" && drag.id === room.id;
	const dropSide =
		dropTarget?.kind === "room" && dropTarget.id === room.id ? dropTarget.side : null;
	const segId = dragSegId ?? room.id;
	const role = dragRole ?? "segment";

	return (
		<RoomTab
			r={derived}
			active={active}
			onClick={onClick}
			onClose={onClose}
			dragging={isDragged}
			dropSide={dropSide}
			dragKind="room"
			dragId={room.id}
			dragSegId={segId}
			dragRole={role}
			onPointerDown={(e) => startDrag(e, { kind: "room", id: room.id, segId, role })}
			onPointerMove={dragHandlers.onPointerMove}
			onPointerUp={dragHandlers.onPointerUp}
			onPointerCancel={dragHandlers.onPointerCancel}
			onLostPointerCapture={dragHandlers.onLostPointerCapture}
			suppressClick={suppressClick}
			renaming={renaming}
			onStartRename={onStartRename}
			onRename={onRename}
			onRenameEnd={onRenameEnd}
		/>
	);
};
